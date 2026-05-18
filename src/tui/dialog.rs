//! Modal dialogs spawned from the selector loop.
//!
//! Each public function (`delete_confirm`, `rename`, `graduate`) takes over
//! `out` and the terminal until the user confirms or cancels, then returns
//! `Some(Action)` or `None`. The selector caller re-renders its own frame
//! after the dialog returns.
//!
//! Validation is performed in pure helpers (`validate_rename`, `validate_graduate`)
//! so the success/failure logic can be unit-tested without a terminal.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::cursor::MoveTo;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType, size};
use crossterm::{ExecutableCommand, queue};

use crate::action::{Action, DeleteTarget};
use crate::naming::{has_date_prefix, normalize};
use crate::workspace::{ProjectsRoot, Workspace, WorkspaceRoot};

use super::input::{Event, Key, KeySource};

// ---------------------------------------------------------------------------
// Shared text-input state
// ---------------------------------------------------------------------------

/// Tiny line editor used by every dialog. `cursor` is a byte index into
/// `buffer` and is always on a UTF-8 char boundary.
#[derive(Debug)]
struct TextField {
    buffer: String,
    cursor: usize,
}

impl TextField {
    fn new(initial: impl Into<String>) -> Self {
        let buffer = initial.into();
        let cursor = buffer.len();
        Self { buffer, cursor }
    }

    /// Apply one key. Returns whether the caller should confirm, cancel, or
    /// keep looping.
    fn apply(&mut self, key: Key) -> TextOutcome {
        match key {
            Key::Enter => TextOutcome::Confirm,
            Key::Escape | Key::Ctrl('c') => TextOutcome::Cancel,
            Key::Backspace => {
                if self.cursor > 0 {
                    let new = prev_boundary(&self.buffer, self.cursor);
                    self.buffer.replace_range(new..self.cursor, "");
                    self.cursor = new;
                }
                TextOutcome::Continue
            }
            Key::Ctrl('a') => {
                self.cursor = 0;
                TextOutcome::Continue
            }
            Key::Ctrl('e') => {
                self.cursor = self.buffer.len();
                TextOutcome::Continue
            }
            Key::Ctrl('b') => {
                self.cursor = prev_boundary(&self.buffer, self.cursor);
                TextOutcome::Continue
            }
            Key::Ctrl('f') => {
                self.cursor = next_boundary(&self.buffer, self.cursor);
                TextOutcome::Continue
            }
            Key::Ctrl('k') => {
                self.buffer.truncate(self.cursor);
                TextOutcome::Continue
            }
            Key::Ctrl('w') => {
                if self.cursor > 0 {
                    let new = word_boundary_backward(&self.buffer, self.cursor);
                    self.buffer.replace_range(new..self.cursor, "");
                    self.cursor = new;
                }
                TextOutcome::Continue
            }
            Key::Char(c) if is_text_char(c) => {
                self.buffer.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                TextOutcome::Continue
            }
            _ => TextOutcome::Continue,
        }
    }
}

enum TextOutcome {
    Continue,
    Confirm,
    Cancel,
}

fn is_text_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '/' | '~')
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

// ---------------------------------------------------------------------------
// Delete confirmation
// ---------------------------------------------------------------------------

/// Show the list of marked workspaces and require the user to type `YES`
/// to confirm. Anything else (Esc, Ctrl-C, wrong text) cancels.
///
/// `confirm_override` short-circuits the interactive loop: if `Some("YES")`,
/// the action is built immediately; if `Some(other)`, returns `None`.
pub fn delete_confirm<W: Write>(
    out: &mut W,
    keys: &mut dyn KeySource,
    marked: &[PathBuf],
    workspaces: &[Workspace],
    root: &WorkspaceRoot,
    confirm_override: Option<&str>,
) -> io::Result<Option<Action>> {
    // Resolve marked paths back to workspaces in the snapshot, dropping
    // anything no longer present (defensive; shouldn't happen in practice).
    let targets: Vec<&Workspace> = marked
        .iter()
        .filter_map(|p| workspaces.iter().find(|w| &w.path == p))
        .collect();
    if targets.is_empty() {
        return Ok(None);
    }

    let build_action = |targets: &[&Workspace]| Action::Delete {
        targets: targets
            .iter()
            .map(|w| DeleteTarget {
                real_path: w.path.clone(),
                basename: w.name.clone(),
            })
            .collect(),
        base: root.as_path().to_path_buf(),
    };

    if let Some(text) = confirm_override {
        return Ok((text == "YES").then(|| build_action(&targets)));
    }

    let mut field = TextField::new("");
    loop {
        render_delete(out, &targets, &field)?;
        match read_text_event(keys, &mut field)? {
            TextEvent::Continue => continue,
            TextEvent::Cancel => return Ok(None),
            TextEvent::Confirm => {
                if field.buffer != "YES" {
                    // Spec is strict: anything other than exactly "YES" cancels.
                    return Ok(None);
                }
                return Ok(Some(build_action(&targets)));
            }
        }
    }
}

fn render_delete<W: Write>(
    out: &mut W,
    targets: &[&Workspace],
    field: &TextField,
) -> io::Result<()> {
    let (cols, _) = size().unwrap_or((80, 24));
    out.execute(Clear(ClearType::All))?;
    title(out, "🗑️  Confirm deletion", cols)?;

    let n = targets.len();
    let word = if n == 1 { "directory" } else { "directories" };
    queue!(out, MoveTo(0, 2))?;
    queue!(
        out,
        SetForegroundColor(Color::Red),
        Print(format!("Delete {n} {word}? This cannot be undone.")),
        ResetColor,
    )?;

    for (i, ws) in targets.iter().enumerate() {
        queue!(out, MoveTo(2, 4 + i as u16))?;
        queue!(
            out,
            SetBackgroundColor(Color::DarkRed),
            Print(format!(" 🗑️  {} ", ws.name)),
            ResetColor,
        )?;
    }

    let prompt_row = 5 + targets.len() as u16;
    queue!(out, MoveTo(0, prompt_row))?;
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("Type "),
        SetForegroundColor(Color::Yellow),
        SetAttribute(Attribute::Bold),
        Print("YES"),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::DarkGrey),
        Print(" to confirm: "),
        ResetColor,
    )?;
    draw_field(out, field)?;

    out.flush()
}

// ---------------------------------------------------------------------------
// Rename
// ---------------------------------------------------------------------------

/// Open the rename dialog seeded with `ws.name`. Returns `Some(Rename)` on
/// confirm, `None` on cancel or no-op rename.
pub fn rename<W: Write>(
    out: &mut W,
    keys: &mut dyn KeySource,
    ws: &Workspace,
    root: &WorkspaceRoot,
) -> io::Result<Option<Action>> {
    let mut field = TextField::new(&ws.name);
    let mut error: Option<String> = None;

    loop {
        render_rename(out, &ws.name, &field, error.as_deref())?;
        match read_text_event(keys, &mut field)? {
            TextEvent::Continue => {
                error = None;
                continue;
            }
            TextEvent::Cancel => return Ok(None),
            TextEvent::Confirm => match validate_rename(&field.buffer, &ws.name, root.as_path()) {
                Ok(None) => return Ok(None), // unchanged
                Ok(Some(new)) => {
                    return Ok(Some(Action::Rename {
                        base: root.as_path().to_path_buf(),
                        from: ws.name.clone(),
                        to: new,
                    }));
                }
                Err(msg) => error = Some(msg),
            },
        }
    }
}

/// Returns `Ok(None)` for an unchanged name, `Ok(Some(new))` for a valid
/// rename, or `Err(msg)` for a validation failure.
pub fn validate_rename(input: &str, current: &str, root: &Path) -> Result<Option<String>, String> {
    // Match upstream: whitespace runs collapse to `-`. We do this *before*
    // the empty check so a buffer of pure whitespace is rejected.
    let normalized = normalize(input.trim());
    if normalized.is_empty() {
        return Err("Name cannot be empty".into());
    }
    if normalized.contains('/') {
        return Err("Name cannot contain '/'".into());
    }
    if normalized == current {
        return Ok(None);
    }
    if root.join(&normalized).exists() {
        return Err(format!("Directory exists: {normalized}"));
    }
    Ok(Some(normalized))
}

fn render_rename<W: Write>(
    out: &mut W,
    current: &str,
    field: &TextField,
    error: Option<&str>,
) -> io::Result<()> {
    let (cols, rows) = size().unwrap_or((80, 24));
    out.execute(Clear(ClearType::All))?;
    title(out, "✏️  Rename directory", cols)?;

    queue!(out, MoveTo(2, 3))?;
    queue!(out, Print("📁 "), Print(current))?;

    queue!(out, MoveTo(2, 5))?;
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("New name: "),
        ResetColor,
    )?;
    draw_field(out, field)?;

    if let Some(msg) = error {
        queue!(out, MoveTo(2, 7))?;
        queue!(out, SetForegroundColor(Color::Red), Print(msg), ResetColor)?;
    }

    confirm_hint(out, cols, rows)?;
    out.flush()
}

// ---------------------------------------------------------------------------
// Graduate
// ---------------------------------------------------------------------------

/// Open the graduate dialog seeded with `<projects>/<basename-without-date>`.
pub fn graduate<W: Write>(
    out: &mut W,
    keys: &mut dyn KeySource,
    ws: &Workspace,
    root: &WorkspaceRoot,
    projects: &ProjectsRoot,
) -> io::Result<Option<Action>> {
    let stem = strip_date_prefix(&ws.name);
    let default_dest = projects.as_path().join(stem);
    let mut field = TextField::new(default_dest.to_string_lossy().into_owned());
    let mut error: Option<String> = None;

    loop {
        render_graduate(out, ws, &field, error.as_deref(), projects)?;
        match read_text_event(keys, &mut field)? {
            TextEvent::Continue => {
                error = None;
                continue;
            }
            TextEvent::Cancel => return Ok(None),
            TextEvent::Confirm => match validate_graduate(&field.buffer) {
                Ok(dest) => {
                    return Ok(Some(Action::Graduate {
                        source: ws.path.clone(),
                        dest,
                        basename: ws.name.clone(),
                        base: root.as_path().to_path_buf(),
                        is_worktree: is_worktree(&ws.path),
                    }));
                }
                Err(msg) => error = Some(msg),
            },
        }
    }
}

/// Validate the destination path: must be non-empty, must not already exist,
/// and its parent must exist.
pub fn validate_graduate(input: &str) -> Result<PathBuf, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("Destination cannot be empty".into());
    }
    let dest = expand_user(trimmed);
    if dest.exists() {
        return Err(format!("Destination already exists: {}", dest.display()));
    }
    match dest.parent() {
        Some(p) if p.as_os_str().is_empty() => {
            // e.g. "foo" with no parent component — fine, treat as cwd.
        }
        Some(p) if !p.exists() => {
            return Err(format!("Parent directory does not exist: {}", p.display()));
        }
        _ => {}
    }
    Ok(dest)
}

fn render_graduate<W: Write>(
    out: &mut W,
    ws: &Workspace,
    field: &TextField,
    error: Option<&str>,
    projects: &ProjectsRoot,
) -> io::Result<()> {
    let (cols, rows) = size().unwrap_or((80, 24));
    out.execute(Clear(ClearType::All))?;
    title(out, "🎓 Graduate to project", cols)?;

    queue!(out, MoveTo(2, 3))?;
    queue!(out, Print("📁 "), Print(&ws.name))?;

    queue!(out, MoveTo(2, 5))?;
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print(format!(
            "Destination (under {}): ",
            projects.as_path().display()
        )),
        ResetColor,
    )?;
    queue!(out, MoveTo(2, 6))?;
    draw_field(out, field)?;

    queue!(out, MoveTo(2, 8))?;
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("A symlink will be left in the tries directory."),
        ResetColor,
    )?;

    if let Some(msg) = error {
        queue!(out, MoveTo(2, 10))?;
        queue!(out, SetForegroundColor(Color::Red), Print(msg), ResetColor)?;
    }

    confirm_hint(out, cols, rows)?;
    out.flush()
}

fn strip_date_prefix(name: &str) -> &str {
    if has_date_prefix(name) {
        &name[11..]
    } else {
        name
    }
}

fn is_worktree(source: &Path) -> bool {
    // A git worktree's `.git` is a *file* pointing at the main repo's
    // `gitdir`, not a directory. We can't read it without I/O, so we just
    // detect "is `.git` a file".
    source.join(".git").is_file()
}

fn expand_user(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = std::env::home_dir()
    {
        return home.join(rest);
    }
    if s == "~"
        && let Some(home) = std::env::home_dir()
    {
        return home;
    }
    PathBuf::from(s)
}

// ---------------------------------------------------------------------------
// Shared rendering helpers + event reader
// ---------------------------------------------------------------------------

/// Pinned-to-bottom hint shown by rename and graduate dialogs.
fn confirm_hint<W: Write>(out: &mut W, cols: u16, rows: u16) -> io::Result<()> {
    queue!(out, MoveTo(0, rows.saturating_sub(2)))?;
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(cols as usize)),
        Print("\r\n"),
    )?;
    queue!(out, MoveTo(0, rows.saturating_sub(1)))?;
    queue!(
        out,
        Print("Enter: Confirm  Esc: Cancel"),
        ResetColor,
        Print("\r\n"),
    )?;
    Ok(())
}

fn title<W: Write>(out: &mut W, label: &str, cols: u16) -> io::Result<()> {
    queue!(out, MoveTo(0, 0))?;
    queue!(
        out,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print(label),
        SetAttribute(Attribute::Reset),
        ResetColor,
    )?;
    queue!(out, MoveTo(0, 1))?;
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(cols as usize)),
        ResetColor,
    )?;
    Ok(())
}

fn draw_field<W: Write>(out: &mut W, field: &TextField) -> io::Result<()> {
    let cursor = field.cursor.min(field.buffer.len());
    let (before, after) = field.buffer.split_at(cursor);
    queue!(out, Print(before))?;
    queue!(out, SetAttribute(Attribute::Reverse))?;
    if let Some(ch) = after.chars().next() {
        queue!(out, Print(ch))?;
        queue!(out, SetAttribute(Attribute::Reset))?;
        queue!(out, Print(&after[ch.len_utf8()..]))?;
    } else {
        queue!(out, Print(' '))?;
        queue!(out, SetAttribute(Attribute::Reset))?;
    }
    Ok(())
}

enum TextEvent {
    Continue,
    Confirm,
    Cancel,
}

fn read_text_event(keys: &mut dyn KeySource, field: &mut TextField) -> io::Result<TextEvent> {
    match keys.read_event(Duration::from_millis(200))? {
        None | Some(Event::Resize) => Ok(TextEvent::Continue),
        Some(Event::Key(key)) => Ok(match field.apply(key) {
            TextOutcome::Continue => TextEvent::Continue,
            TextOutcome::Confirm => TextEvent::Confirm,
            TextOutcome::Cancel => TextEvent::Cancel,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // --- validate_rename ---

    #[test]
    fn rename_rejects_empty() {
        let td = TempDir::new().unwrap();
        let err = validate_rename("   ", "foo", td.path()).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn rename_rejects_slash() {
        let td = TempDir::new().unwrap();
        let err = validate_rename("a/b", "foo", td.path()).unwrap_err();
        assert!(err.contains("/"));
    }

    #[test]
    fn rename_returns_none_for_unchanged() {
        let td = TempDir::new().unwrap();
        let got = validate_rename("foo", "foo", td.path()).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn rename_rejects_collision() {
        let td = TempDir::new().unwrap();
        fs::create_dir(td.path().join("taken")).unwrap();
        let err = validate_rename("taken", "foo", td.path()).unwrap_err();
        assert!(err.contains("exists"));
    }

    #[test]
    fn rename_accepts_valid_new_name() {
        let td = TempDir::new().unwrap();
        let got = validate_rename("  new-name  ", "old-name", td.path()).unwrap();
        assert_eq!(got, Some("new-name".to_string()));
    }

    // --- validate_graduate ---

    #[test]
    fn graduate_rejects_empty() {
        let err = validate_graduate("").unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn graduate_rejects_existing_path() {
        let td = TempDir::new().unwrap();
        let p = td.path().join("exists");
        fs::create_dir(&p).unwrap();
        let err = validate_graduate(p.to_str().unwrap()).unwrap_err();
        assert!(err.contains("already exists"));
    }

    #[test]
    fn graduate_rejects_missing_parent() {
        let err = validate_graduate("/this/path/does/not/exist/foo").unwrap_err();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn graduate_accepts_valid_dest() {
        let td = TempDir::new().unwrap();
        let dest = td.path().join("new-home");
        let got = validate_graduate(dest.to_str().unwrap()).unwrap();
        assert_eq!(got, dest);
    }

    // --- helpers ---

    #[test]
    fn strip_date_prefix_removes_yyyy_mm_dd() {
        assert_eq!(strip_date_prefix("2026-05-16-foo"), "foo");
        assert_eq!(strip_date_prefix("no-date"), "no-date");
    }

    #[test]
    fn text_field_basic_editing() {
        let mut f = TextField::new("hello");
        assert_eq!(f.cursor, 5);
        f.apply(Key::Backspace);
        assert_eq!(f.buffer, "hell");
        assert_eq!(f.cursor, 4);
        f.apply(Key::Ctrl('a'));
        assert_eq!(f.cursor, 0);
        f.apply(Key::Ctrl('e'));
        assert_eq!(f.cursor, 4);
        f.apply(Key::Char('o'));
        assert_eq!(f.buffer, "hello");
    }

    #[test]
    fn text_field_word_delete() {
        let mut f = TextField::new("foo bar baz");
        f.apply(Key::Ctrl('w'));
        assert_eq!(f.buffer, "foo bar ");
    }
}
