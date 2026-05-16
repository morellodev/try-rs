//! Interactive workspace selector.
//!
//! The selector is split into two layers:
//!
//! - A pure state machine ([`State`] + [`apply_key`]) that owns the query
//!   buffer and the navigation cursor. No I/O, fully unit-tested.
//! - A thin I/O loop ([`run`]) that drives the state machine with crossterm
//!   events, recomputes the fuzzy view each frame, and re-renders.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::action::Action;
use crate::clock::Clock;
use crate::fuzzy::{Candidate, Matcher};
use crate::naming::normalize;
use crate::workspace::{ProjectsRoot, Workspace, WorkspaceRoot};

use super::dialog;
use super::input::{Event, Key, KeySource};
use super::render::{self, Row, Screen};

/// Result of feeding one key into [`apply_key`].
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Stay in the loop; redraw on next iteration.
    Continue,
    /// User chose an action; the caller should exit and emit the script.
    Selected(Action),
    /// User cancelled (Esc / Ctrl-C). Caller should exit non-zero.
    Cancelled,
    /// Open the batch-delete confirmation dialog.
    OpenDeleteConfirm,
    /// Open the rename dialog for the currently selected workspace.
    OpenRename,
    /// Open the graduate dialog for the currently selected workspace.
    OpenGraduate,
}

/// Mutable selector state.
///
/// `query_cursor` is a byte index into `query` and is always on a UTF-8 char
/// boundary. `cursor` and `scroll` index into the combined "rows + create_new"
/// item list as seen by the user.
#[derive(Debug, Default, Clone)]
pub struct State {
    pub query: String,
    pub query_cursor: usize,
    pub cursor: usize,
    pub scroll: usize,
    /// Canonical paths of workspaces the user has toggled for deletion.
    pub marked: Vec<PathBuf>,
}

impl State {
    /// Delete mode is on whenever any item is marked.
    pub fn delete_mode(&self) -> bool {
        !self.marked.is_empty()
    }
}

impl State {
    pub fn with_query(query: impl Into<String>) -> Self {
        let query = query.into();
        let query_cursor = query.len();
        Self {
            query,
            query_cursor,
            cursor: 0,
            scroll: 0,
            marked: Vec::new(),
        }
    }
}

/// What the state machine needs that isn't its own state.
pub struct Context<'a> {
    pub rows: &'a [Row<'a>],
    /// `Some(label)` means the "Create new: <label>" row is offered.
    pub create_new: Option<&'a str>,
    pub root: &'a WorkspaceRoot,
    pub clock: &'a dyn Clock,
}

impl std::fmt::Debug for Context<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Context")
            .field("rows", &self.rows.len())
            .field("create_new", &self.create_new)
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Context<'_> {
    fn total_items(&self) -> usize {
        self.rows.len() + usize::from(self.create_new.is_some())
    }
}

/// Apply one key event to `state`. Pure; safe to unit-test without a terminal.
pub fn apply_key(state: &mut State, key: Key, ctx: &Context<'_>) -> Outcome {
    let total = ctx.total_items();

    match key {
        Key::Enter => {
            if state.delete_mode() {
                return Outcome::OpenDeleteConfirm;
            }
            select(state, ctx)
        }

        // --- delete / rename / graduate ---
        Key::Ctrl('d') => {
            if state.cursor < ctx.rows.len() {
                let path = ctx.rows[state.cursor].workspace.path.clone();
                if let Some(idx) = state.marked.iter().position(|p| p == &path) {
                    state.marked.remove(idx);
                } else {
                    state.marked.push(path);
                }
            }
            Outcome::Continue
        }
        Key::Ctrl('r') => {
            if state.cursor < ctx.rows.len() {
                return Outcome::OpenRename;
            }
            Outcome::Continue
        }
        Key::Ctrl('g') => {
            if state.cursor < ctx.rows.len() {
                return Outcome::OpenGraduate;
            }
            Outcome::Continue
        }

        // --- navigation ---
        Key::Up | Key::Ctrl('p') => {
            state.cursor = state.cursor.saturating_sub(1);
            Outcome::Continue
        }
        Key::Down | Key::Ctrl('n') => {
            if total > 0 && state.cursor + 1 < total {
                state.cursor += 1;
            }
            Outcome::Continue
        }

        // --- line editing ---
        Key::Backspace => {
            if state.query_cursor > 0 {
                let new = prev_boundary(&state.query, state.query_cursor);
                state.query.replace_range(new..state.query_cursor, "");
                state.query_cursor = new;
                state.cursor = 0;
                state.scroll = 0;
            }
            Outcome::Continue
        }
        Key::Ctrl('a') => {
            state.query_cursor = 0;
            Outcome::Continue
        }
        Key::Ctrl('e') => {
            state.query_cursor = state.query.len();
            Outcome::Continue
        }
        Key::Ctrl('b') => {
            state.query_cursor = prev_boundary(&state.query, state.query_cursor);
            Outcome::Continue
        }
        Key::Ctrl('f') => {
            state.query_cursor = next_boundary(&state.query, state.query_cursor);
            Outcome::Continue
        }
        Key::Ctrl('k') => {
            state.query.truncate(state.query_cursor);
            Outcome::Continue
        }
        Key::Ctrl('w') => {
            if state.query_cursor > 0 {
                let new = word_boundary_backward(&state.query, state.query_cursor);
                state.query.replace_range(new..state.query_cursor, "");
                state.query_cursor = new;
                state.cursor = 0;
                state.scroll = 0;
            }
            Outcome::Continue
        }

        Key::Ctrl('t') => create_new(state, ctx),

        Key::Char(c) if is_query_char(c) => {
            state.query.insert(state.query_cursor, c);
            state.query_cursor += c.len_utf8();
            state.cursor = 0;
            state.scroll = 0;
            Outcome::Continue
        }

        Key::Escape | Key::Ctrl('c') => {
            // Esc in delete mode clears marks instead of exiting; Esc with no
            // marks cancels the whole selector.
            if state.delete_mode() {
                state.marked.clear();
                Outcome::Continue
            } else {
                Outcome::Cancelled
            }
        }

        _ => Outcome::Continue,
    }
}

fn select(state: &State, ctx: &Context<'_>) -> Outcome {
    if state.cursor < ctx.rows.len() {
        return Outcome::Selected(Action::Cd {
            path: ctx.rows[state.cursor].workspace.path.clone(),
        });
    }
    if ctx.create_new.is_some() {
        return create_new(state, ctx);
    }
    Outcome::Continue
}

fn create_new(state: &State, ctx: &Context<'_>) -> Outcome {
    if state.query.trim().is_empty() {
        return Outcome::Continue;
    }
    let dir = format!("{}-{}", ctx.clock.today(), normalize(state.query.trim()));
    Outcome::Selected(Action::Mkdir {
        path: ctx.root.join(dir),
    })
}

/// Clamp `state.scroll` so the cursor is visible within `body_rows`.
pub fn adjust_scroll(state: &mut State, body_rows: usize, total_items: usize) {
    if total_items == 0 || body_rows == 0 {
        state.scroll = 0;
        return;
    }
    if state.cursor < state.scroll {
        state.scroll = state.cursor;
    } else if state.cursor >= state.scroll + body_rows {
        state.scroll = state.cursor + 1 - body_rows;
    }
}

/// Run the selector. Returns `Ok(Some(action))` on selection, `Ok(None)` on
/// cancel. The caller is responsible for setting up / tearing down the
/// terminal (typically via [`super::terminal::Guard`]).
#[allow(clippy::too_many_arguments)]
pub fn run<W: Write>(
    out: &mut W,
    keys: &mut dyn KeySource,
    root: &WorkspaceRoot,
    projects: &ProjectsRoot,
    clock: &dyn Clock,
    workspaces: &[Workspace],
    initial_query: &str,
    confirm_override: Option<&str>,
) -> io::Result<Option<Action>> {
    let candidates: Vec<Candidate> = workspaces
        .iter()
        .map(|w| Candidate::new(&w.name, w.base_score))
        .collect();
    let matcher = Matcher::new(&candidates);

    let mut state = State::with_query(normalize(initial_query));

    loop {
        let hits = matcher.query(&state.query, 0);
        let rows: Vec<Row<'_>> = hits
            .iter()
            .map(|h| Row {
                workspace: &workspaces[h.index],
                positions: h.positions.as_slice(),
            })
            .collect();

        let create_new_label = (!state.query.trim().is_empty())
            .then(|| format!("{}-{}", clock.today(), normalize(state.query.trim())));
        let create_new = create_new_label.as_deref();

        let ctx = Context {
            rows: &rows,
            create_new,
            root,
            clock,
        };
        let total = ctx.total_items();
        if state.cursor >= total {
            state.cursor = total.saturating_sub(1);
        }
        adjust_scroll(&mut state, render::body_capacity().max(1), total);

        let selected_workspace = (state.cursor < rows.len()).then(|| rows[state.cursor].workspace);

        let screen = Screen {
            rows: &rows,
            query: &state.query,
            query_cursor: state.query_cursor,
            cursor: state.cursor,
            scroll: state.scroll,
            create_new,
            marked: &state.marked,
            delete_mode: state.delete_mode(),
            now: clock.now(),
        };
        render::draw(out, &screen)?;

        match keys.read_event(Duration::from_millis(200))? {
            None => continue,
            Some(Event::Resize) => continue, // top of loop redraws
            Some(Event::Key(key)) => match apply_key(&mut state, key, &ctx) {
                Outcome::Continue => continue,
                Outcome::Selected(action) => return Ok(Some(action)),
                Outcome::Cancelled => return Ok(None),
                Outcome::OpenDeleteConfirm => {
                    let action = dialog::delete_confirm(
                        out,
                        keys,
                        &state.marked,
                        workspaces,
                        root,
                        confirm_override,
                    )?;
                    // Either way (confirmed or cancelled), clear marks so the
                    // selector doesn't keep a stale "delete mode" alive.
                    state.marked.clear();
                    if let Some(a) = action {
                        return Ok(Some(a));
                    }
                }
                Outcome::OpenRename => {
                    if let Some(ws) = selected_workspace
                        && let Some(a) = dialog::rename(out, keys, ws, root)?
                    {
                        return Ok(Some(a));
                    }
                }
                Outcome::OpenGraduate => {
                    if let Some(ws) = selected_workspace
                        && let Some(a) = dialog::graduate(out, keys, ws, root, projects)?
                    {
                        return Ok(Some(a));
                    }
                }
            },
        }
    }
}

fn is_query_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ')
}

fn prev_boundary(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    s.floor_char_boundary(cursor - 1)
}

fn next_boundary(s: &str, cursor: usize) -> usize {
    if cursor >= s.len() {
        return s.len();
    }
    s.ceil_char_boundary(cursor + 1)
}

fn word_boundary_backward(buf: &str, cursor: usize) -> usize {
    // Mirrors the upstream Ruby: skip non-word chars backward, then skip word
    // chars backward; the result is the start of the previous "word".
    let mut pos = cursor;
    while pos > 0 {
        let prev = prev_boundary(buf, pos);
        let ch = buf[prev..].chars().next().unwrap_or(' ');
        if ch.is_alphanumeric() {
            break;
        }
        pos = prev;
    }
    while pos > 0 {
        let prev = prev_boundary(buf, pos);
        let ch = buf[prev..].chars().next().unwrap_or(' ');
        if !ch.is_alphanumeric() {
            break;
        }
        pos = prev;
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;
    use std::path::PathBuf;
    use std::time::SystemTime;

    struct FixedClock;
    impl Clock for FixedClock {
        fn today(&self) -> String {
            "2026-05-16".to_string()
        }
        fn now(&self) -> SystemTime {
            SystemTime::UNIX_EPOCH
        }
    }

    fn ws(name: &str) -> Workspace {
        Workspace {
            name: name.to_string(),
            path: PathBuf::from("/tries").join(name),
            is_symlink: false,
            modified: SystemTime::UNIX_EPOCH,
            created: SystemTime::UNIX_EPOCH,
            base_score: 0.0,
        }
    }

    fn root() -> WorkspaceRoot {
        WorkspaceRoot::new(PathBuf::from("/tries")).unwrap()
    }

    fn rows_of<'a>(workspaces: &'a [Workspace]) -> Vec<Row<'a>> {
        workspaces
            .iter()
            .map(|w| Row {
                workspace: w,
                positions: &[],
            })
            .collect()
    }

    fn ctx<'a>(
        rows: &'a [Row<'a>],
        create_new: Option<&'a str>,
        root: &'a WorkspaceRoot,
        clock: &'a dyn Clock,
    ) -> Context<'a> {
        Context {
            rows,
            create_new,
            root,
            clock,
        }
    }

    // --- navigation ---

    #[test]
    fn down_advances_cursor() {
        let workspaces = vec![ws("a"), ws("b"), ws("c")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State::default();
        let c = ctx(&rows, None, &root, &clock);
        assert_eq!(apply_key(&mut state, Key::Down, &c), Outcome::Continue);
        assert_eq!(state.cursor, 1);
        assert_eq!(apply_key(&mut state, Key::Ctrl('n'), &c), Outcome::Continue);
        assert_eq!(state.cursor, 2);
        // At the end, Down is a no-op.
        assert_eq!(apply_key(&mut state, Key::Down, &c), Outcome::Continue);
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn up_at_zero_stays() {
        let workspaces = vec![ws("a")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State::default();
        apply_key(&mut state, Key::Up, &ctx(&rows, None, &root, &clock));
        assert_eq!(state.cursor, 0);
    }

    // --- input ---

    #[test]
    fn typing_inserts_and_resets_cursor() {
        let mut state = State {
            query: "redi".into(),
            query_cursor: 4,
            cursor: 5, // would be invalid; typing must reset to 0
            scroll: 2,
            marked: vec![],
        };
        let root = root();
        let clock = FixedClock;
        let c = ctx(&[], None, &root, &clock);
        apply_key(&mut state, Key::Char('s'), &c);
        assert_eq!(state.query, "redis");
        assert_eq!(state.query_cursor, 5);
        assert_eq!(state.cursor, 0);
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn backspace_removes_char_before_cursor() {
        let mut state = State {
            query: "redis".into(),
            query_cursor: 5,
            cursor: 0,
            scroll: 0,
            marked: vec![],
        };
        let root = root();
        let clock = FixedClock;
        let c = ctx(&[], None, &root, &clock);
        apply_key(&mut state, Key::Backspace, &c);
        assert_eq!(state.query, "redi");
        assert_eq!(state.query_cursor, 4);
    }

    #[test]
    fn ctrl_a_and_e_jump_to_edges() {
        let mut state = State {
            query: "hello".into(),
            query_cursor: 3,
            cursor: 0,
            scroll: 0,
            marked: vec![],
        };
        let root = root();
        let clock = FixedClock;
        let c = ctx(&[], None, &root, &clock);
        apply_key(&mut state, Key::Ctrl('a'), &c);
        assert_eq!(state.query_cursor, 0);
        apply_key(&mut state, Key::Ctrl('e'), &c);
        assert_eq!(state.query_cursor, 5);
    }

    #[test]
    fn ctrl_w_deletes_previous_word() {
        let mut state = State {
            query: "foo-bar baz".into(),
            query_cursor: 11,
            cursor: 0,
            scroll: 0,
            marked: vec![],
        };
        let root = root();
        let clock = FixedClock;
        apply_key(&mut state, Key::Ctrl('w'), &ctx(&[], None, &root, &clock));
        assert_eq!(state.query, "foo-bar ");
        assert_eq!(state.query_cursor, 8);
    }

    // --- selection ---

    #[test]
    fn enter_on_workspace_returns_cd_action() {
        let workspaces = vec![ws("alpha"), ws("beta")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State {
            cursor: 1,
            ..State::default()
        };
        let outcome = apply_key(&mut state, Key::Enter, &ctx(&rows, None, &root, &clock));
        match outcome {
            Outcome::Selected(Action::Cd { path }) => {
                assert_eq!(path, PathBuf::from("/tries/beta"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn enter_on_create_new_returns_mkdir_with_dated_name() {
        let root = root();
        let clock = FixedClock;
        let label = "2026-05-16-redis-pool";
        let mut state = State {
            query: "redis pool".into(),
            query_cursor: 10,
            cursor: 0, // on the create-new row (no workspace rows)
            scroll: 0,
            marked: vec![],
        };
        let outcome = apply_key(&mut state, Key::Enter, &ctx(&[], Some(label), &root, &clock));
        match outcome {
            Outcome::Selected(Action::Mkdir { path }) => {
                assert_eq!(path, PathBuf::from("/tries/2026-05-16-redis-pool"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn ctrl_t_creates_new_directly() {
        let root = root();
        let clock = FixedClock;
        let mut state = State {
            query: "new-thing".into(),
            query_cursor: 9,
            cursor: 0,
            scroll: 0,
            marked: vec![],
        };
        let outcome = apply_key(&mut state, Key::Ctrl('t'), &ctx(&[], None, &root, &clock));
        match outcome {
            Outcome::Selected(Action::Mkdir { path }) => {
                assert_eq!(path, PathBuf::from("/tries/2026-05-16-new-thing"));
            }
            other => panic!("unexpected outcome: {other:?}"),
        }
    }

    #[test]
    fn escape_returns_cancelled() {
        let mut state = State::default();
        let root = root();
        let clock = FixedClock;
        assert_eq!(
            apply_key(&mut state, Key::Escape, &ctx(&[], None, &root, &clock)),
            Outcome::Cancelled,
        );
        assert_eq!(
            apply_key(&mut state, Key::Ctrl('c'), &ctx(&[], None, &root, &clock)),
            Outcome::Cancelled,
        );
    }

    // --- marks / dialog triggers ---

    #[test]
    fn ctrl_d_toggles_mark_on_current_workspace() {
        let workspaces = vec![ws("alpha"), ws("beta")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State::default();

        apply_key(&mut state, Key::Ctrl('d'), &ctx(&rows, None, &root, &clock));
        assert_eq!(state.marked, vec![workspaces[0].path.clone()]);
        assert!(state.delete_mode());

        // Same workspace again → unmarks.
        apply_key(&mut state, Key::Ctrl('d'), &ctx(&rows, None, &root, &clock));
        assert!(state.marked.is_empty());
        assert!(!state.delete_mode());
    }

    #[test]
    fn enter_in_delete_mode_opens_confirm() {
        let workspaces = vec![ws("a")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State::default();
        state.marked.push(workspaces[0].path.clone());

        let outcome = apply_key(&mut state, Key::Enter, &ctx(&rows, None, &root, &clock));
        assert_eq!(outcome, Outcome::OpenDeleteConfirm);
    }

    #[test]
    fn esc_in_delete_mode_clears_marks_without_cancelling() {
        let workspaces = vec![ws("a")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State::default();
        state.marked.push(workspaces[0].path.clone());

        let outcome = apply_key(&mut state, Key::Escape, &ctx(&rows, None, &root, &clock));
        assert_eq!(outcome, Outcome::Continue);
        assert!(state.marked.is_empty());
    }

    #[test]
    fn ctrl_r_opens_rename_only_when_on_workspace() {
        let workspaces = vec![ws("a")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State::default();

        let outcome = apply_key(&mut state, Key::Ctrl('r'), &ctx(&rows, None, &root, &clock));
        assert_eq!(outcome, Outcome::OpenRename);

        // Now move cursor past the workspaces (onto a hypothetical create-new row).
        state.cursor = 1;
        let outcome = apply_key(
            &mut state,
            Key::Ctrl('r'),
            &ctx(&rows, Some("2026-05-16-foo"), &root, &clock),
        );
        assert_eq!(outcome, Outcome::Continue);
    }

    #[test]
    fn ctrl_g_opens_graduate_only_when_on_workspace() {
        let workspaces = vec![ws("a")];
        let rows = rows_of(&workspaces);
        let root = root();
        let clock = FixedClock;
        let mut state = State::default();

        let outcome = apply_key(&mut state, Key::Ctrl('g'), &ctx(&rows, None, &root, &clock));
        assert_eq!(outcome, Outcome::OpenGraduate);
    }

    // --- scroll ---

    #[test]
    fn adjust_scroll_keeps_cursor_visible() {
        let mut state = State {
            cursor: 9,
            scroll: 0,
            ..State::default()
        };
        adjust_scroll(&mut state, 5, 20);
        // cursor 9 must be in [scroll, scroll+5) → scroll = 5.
        assert_eq!(state.scroll, 5);
    }

    #[test]
    fn adjust_scroll_pulls_back_when_cursor_above() {
        let mut state = State {
            cursor: 1,
            scroll: 5,
            ..State::default()
        };
        adjust_scroll(&mut state, 5, 20);
        assert_eq!(state.scroll, 1);
    }

    #[test]
    fn adjust_scroll_zeroes_for_empty() {
        let mut state = State {
            scroll: 7,
            cursor: 0,
            ..State::default()
        };
        adjust_scroll(&mut state, 5, 0);
        assert_eq!(state.scroll, 0);
    }
}
