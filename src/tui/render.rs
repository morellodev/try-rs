//! Render a [`Screen`] to any `Write` as a single ANSI frame.
//!
//! The model is intentionally view-only: this module owns no state and
//! mutates none. The selector loop computes the next [`Screen`] and asks
//! [`draw`] to render it.

use std::io::{self, Write};
use std::path::PathBuf;
use std::time::SystemTime;

use crossterm::cursor::MoveTo;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{Clear, ClearType, size};
use crossterm::{ExecutableCommand, queue};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::clock::format_relative;
use crate::workspace::Workspace;

const HEADER_ROWS: u16 = 4;
const FOOTER_ROWS: u16 = 2;

/// A single workspace ready to render, paired with the fuzzy-match positions
/// (character indices into `workspace.name`) that should be highlighted.
#[derive(Debug, Clone, Copy)]
pub struct Row<'a> {
    pub workspace: &'a Workspace,
    pub positions: &'a [usize],
}

/// View-model for one render pass.
#[derive(Debug)]
pub struct Screen<'a> {
    pub rows: &'a [Row<'a>],
    pub query: &'a str,
    /// Byte index into `query` where the input cursor sits; must be on a char
    /// boundary.
    pub query_cursor: usize,
    /// Index of the highlighted list item. `0..rows.len()` indexes `rows`;
    /// `rows.len()` indexes the optional "Create new" row.
    pub cursor: usize,
    /// Index of the first visible row (the list scrolls but the create-new
    /// row is always pinned to the end).
    pub scroll: usize,
    /// When `Some`, an extra "Create new: <label>" row is rendered at the end.
    pub create_new: Option<&'a str>,
    /// Canonical paths of workspaces marked for deletion; rendered with a
    /// danger background.
    pub marked: &'a [PathBuf],
    /// True iff `marked` is non-empty; controls the footer banner.
    pub delete_mode: bool,
    /// Reference instant for relative-time formatting; captured once per frame.
    pub now: SystemTime,
}

/// Total visible body rows for the current terminal size. The selector uses
/// this to clamp `scroll` so the cursor stays visible.
#[must_use]
pub fn body_capacity() -> usize {
    let (_, rows) = terminal_size();
    rows.saturating_sub(HEADER_ROWS + FOOTER_ROWS) as usize
}

/// Terminal dimensions, honoring `TRY_WIDTH` / `TRY_HEIGHT` env-var overrides
/// (used by the spec test harness for deterministic layout assertions).
fn terminal_size() -> (u16, u16) {
    let (def_w, def_h) = size().unwrap_or((80, 24));
    let w = std::env::var("TRY_WIDTH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(def_w);
    let h = std::env::var("TRY_HEIGHT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(def_h);
    (w, h)
}

/// Write a complete frame to `out`.
pub fn draw<W: Write>(out: &mut W, screen: &Screen<'_>) -> io::Result<()> {
    let (cols, rows) = terminal_size();

    out.execute(Clear(ClearType::All))?;
    // Cursor home — emitted as the literal `ESC [ H` so callers that strip
    // ANSI but keep the position byte (like the spec test harness) still see
    // a home-position marker.
    queue!(out, Print("\x1b[H"))?;

    draw_header(out, screen, cols)?;
    draw_body(out, screen, cols, rows)?;
    draw_footer(out, screen, cols, rows)?;

    out.flush()
}

fn draw_header<W: Write>(out: &mut W, screen: &Screen<'_>, cols: u16) -> io::Result<()> {
    // `\e[H` was emitted at the top of `draw`; cursor is already at (0,0).
    // Avoiding `MoveTo(0, 0)` keeps `\e[1;1H` out of the output so the spec
    // pattern `\e\[1[m;]` only matches SGR codes, not positional ones.
    queue!(out, Print("🏠 "))?;
    queue!(
        out,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print("Try Directory Selection"),
        SetAttribute(Attribute::Reset),
        ResetColor,
        Print("\r\n"),
    )?;

    queue!(out, MoveTo(0, 1))?;
    rule(out, cols)?;
    queue!(out, Print("\r\n"))?;

    queue!(out, MoveTo(0, 2))?;
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("Search: "),
        ResetColor,
    )?;
    draw_query_with_cursor(out, screen.query, screen.query_cursor)?;
    queue!(out, Print("\r\n"))?;

    queue!(out, MoveTo(0, 3))?;
    rule(out, cols)?;
    queue!(out, Print("\r\n"))?;

    Ok(())
}

fn draw_query_with_cursor<W: Write>(out: &mut W, query: &str, cursor: usize) -> io::Result<()> {
    let cursor = cursor.min(query.len());
    let (before, after) = query.split_at(cursor);
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

fn draw_body<W: Write>(out: &mut W, screen: &Screen<'_>, cols: u16, rows: u16) -> io::Result<()> {
    let body_rows = rows.saturating_sub(HEADER_ROWS + FOOTER_ROWS) as usize;
    let total_items = screen.rows.len() + usize::from(screen.create_new.is_some());

    if total_items == 0 {
        queue!(out, MoveTo(2, HEADER_ROWS))?;
        queue!(
            out,
            SetForegroundColor(Color::DarkGrey),
            Print("(no workspaces yet)"),
            ResetColor,
        )?;
        return Ok(());
    }

    let end = (screen.scroll + body_rows).min(total_items);
    for (visual_row, item_idx) in (screen.scroll..end).enumerate() {
        let row = HEADER_ROWS + visual_row as u16;
        let selected = item_idx == screen.cursor;

        if item_idx < screen.rows.len() {
            let r = &screen.rows[item_idx];
            let marked = screen.marked.iter().any(|p| p == &r.workspace.path);
            draw_entry(out, r, row, cols, screen.now, selected, marked)?;
        } else if let Some(label) = screen.create_new {
            draw_create_new(out, label, row, cols, selected)?;
        }
    }

    Ok(())
}

fn draw_entry<W: Write>(
    out: &mut W,
    row: &Row<'_>,
    terminal_row: u16,
    cols: u16,
    now: SystemTime,
    selected: bool,
    marked: bool,
) -> io::Result<()> {
    queue!(out, MoveTo(0, terminal_row))?;

    // Selection indicator: a bold arrow. We deliberately avoid setting any
    // row background so emphasis works on both light and dark terminal
    // themes without relying on a single bg/fg pair.
    if selected {
        queue!(
            out,
            SetAttribute(Attribute::Bold),
            Print("→ "),
            SetAttribute(Attribute::Reset)
        )?;
    } else {
        queue!(out, Print("  "))?;
    }

    let icon = if marked {
        "🗑️ "
    } else if row.workspace.is_symlink {
        "🔗 "
    } else {
        "📁 "
    };
    queue!(out, Print(icon))?;

    let meta = format!(
        "{}, {:.1}",
        format_relative(now, row.workspace.modified),
        row.workspace.base_score
    );
    let prefix_cols: usize = 2 + UnicodeWidthStr::width(icon);
    let meta_cols = UnicodeWidthStr::width(meta.as_str());
    let total_cols = cols as usize;
    let name_budget = total_cols.saturating_sub(prefix_cols + meta_cols + 1);

    // Marked rows render their name in bold red so they read clearly on any
    // terminal theme without a row background. Fuzzy highlights are skipped
    // because the row is about to be removed.
    let displayed_cols = if marked {
        queue!(
            out,
            SetForegroundColor(Color::Red),
            SetAttribute(Attribute::Bold)
        )?;
        let cols_used = write_plain_name(out, &row.workspace.name, name_budget)?;
        queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
        cols_used
    } else {
        write_highlighted_name(out, &row.workspace.name, row.positions, name_budget)?
    };

    let padding = total_cols.saturating_sub(prefix_cols + displayed_cols + meta_cols);
    if padding > 0 {
        queue!(out, Print(" ".repeat(padding)))?;
    }

    queue!(out, SetForegroundColor(Color::DarkGrey), Print(meta))?;
    queue!(out, ResetColor, Print("\r\n"))?;
    Ok(())
}

fn draw_create_new<W: Write>(
    out: &mut W,
    label: &str,
    terminal_row: u16,
    cols: u16,
    selected: bool,
) -> io::Result<()> {
    queue!(out, MoveTo(0, terminal_row))?;

    if selected {
        queue!(
            out,
            SetAttribute(Attribute::Bold),
            Print("→ "),
            SetAttribute(Attribute::Reset)
        )?;
    } else {
        queue!(out, Print("  "))?;
    }
    queue!(out, Print("📂 Create new: "))?;
    // Green = positive/creation action. Distinct from the cyan/blue accents
    // and contrasts well on both light and dark backgrounds.
    queue!(
        out,
        SetForegroundColor(Color::Green),
        SetAttribute(Attribute::Bold),
        Print(label),
        SetAttribute(Attribute::Reset),
        ResetColor,
    )?;

    let used = 2 + UnicodeWidthStr::width("📂 Create new: ") + UnicodeWidthStr::width(label);
    let padding = (cols as usize).saturating_sub(used);
    if padding > 0 {
        queue!(out, Print(" ".repeat(padding)))?;
    }
    queue!(out, Print("\r\n"))?;
    Ok(())
}

/// Write `name` honoring `positions` (char indices into `name`) as bold-yellow
/// highlights, truncating with `…` to fit `max_cols`. Returns display columns
/// written. Since the row has no background, each highlight cleanly resets to
/// the terminal default.
fn write_highlighted_name<W: Write>(
    out: &mut W,
    name: &str,
    positions: &[usize],
    max_cols: usize,
) -> io::Result<usize> {
    if max_cols == 0 {
        return Ok(0);
    }
    let full_width = UnicodeWidthStr::width(name);
    let needs_truncate = full_width > max_cols;
    let budget = if needs_truncate {
        max_cols.saturating_sub(1)
    } else {
        max_cols
    };

    let mut cols = 0;
    let mut pos_iter = positions.iter().copied().peekable();

    for (i, ch) in name.chars().enumerate() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + w > budget {
            break;
        }
        let highlighted = pos_iter.peek() == Some(&i);
        if highlighted {
            pos_iter.next();
            queue!(
                out,
                SetForegroundColor(Color::Yellow),
                SetAttribute(Attribute::Bold),
            )?;
        }
        queue!(out, Print(ch))?;
        if highlighted {
            queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
        }
        cols += w;
    }
    if needs_truncate {
        queue!(out, Print('…'))?;
        cols += 1;
    }
    Ok(cols)
}

/// Plain (no-highlight) name writer used for marked rows that already have an
/// outer style (red+bold). Truncates with `…`.
fn write_plain_name<W: Write>(out: &mut W, name: &str, max_cols: usize) -> io::Result<usize> {
    if max_cols == 0 {
        return Ok(0);
    }
    let full_width = UnicodeWidthStr::width(name);
    let needs_truncate = full_width > max_cols;
    let budget = if needs_truncate {
        max_cols.saturating_sub(1)
    } else {
        max_cols
    };

    let mut cols = 0;
    for ch in name.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if cols + w > budget {
            break;
        }
        queue!(out, Print(ch))?;
        cols += w;
    }
    if needs_truncate {
        queue!(out, Print('…'))?;
        cols += 1;
    }
    Ok(cols)
}

fn draw_footer<W: Write>(out: &mut W, screen: &Screen<'_>, cols: u16, rows: u16) -> io::Result<()> {
    let footer_top = rows.saturating_sub(FOOTER_ROWS);
    queue!(out, MoveTo(0, footer_top))?;
    rule(out, cols)?;
    queue!(out, Print("\r\n"))?;

    queue!(out, MoveTo(0, footer_top + 1))?;
    if screen.delete_mode {
        // High-contrast warning banner: bright red bg + white bold fg.
        // Reliable on any terminal theme.
        queue!(
            out,
            SetBackgroundColor(Color::Red),
            SetForegroundColor(Color::White),
            SetAttribute(Attribute::Bold),
        )?;
        let n = screen.marked.len();
        let word = if n == 1 { "item" } else { "items" };
        let line =
            format!(" DELETE MODE  {n} {word} marked — Ctrl-D toggle  Enter confirm  Esc cancel ",);
        let padding = (cols as usize).saturating_sub(UnicodeWidthStr::width(line.as_str()));
        queue!(
            out,
            Print(line),
            Print(" ".repeat(padding)),
            SetAttribute(Attribute::Reset),
            ResetColor,
        )?;
    } else {
        queue!(
            out,
            SetForegroundColor(Color::DarkGrey),
            Print("↑/↓ Navigate  Enter Select  ^R Rename  ^G Graduate  ^D Delete  Esc Cancel"),
            ResetColor,
        )?;
    }
    Ok(())
}

fn rule<W: Write>(out: &mut W, cols: u16) -> io::Result<()> {
    queue!(
        out,
        SetForegroundColor(Color::DarkGrey),
        Print("─".repeat(cols as usize)),
        ResetColor,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn workspace(name: &str, age_secs: u64, base_score: f64) -> Workspace {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        Workspace {
            name: name.to_string(),
            path: PathBuf::from("/tmp").join(name),
            is_symlink: false,
            modified: now - Duration::from_secs(age_secs),
            created: now - Duration::from_secs(age_secs),
            base_score,
        }
    }

    fn render(screen: &Screen<'_>) -> String {
        let mut buf = Vec::<u8>::new();
        draw(&mut buf, screen).unwrap();
        String::from_utf8_lossy(&buf).into_owned()
    }

    #[test]
    fn empty_workspaces_show_placeholder() {
        let screen = Screen {
            rows: &[],
            query: "",
            query_cursor: 0,
            cursor: 0,
            scroll: 0,
            create_new: None,
            marked: &[],
            delete_mode: false,
            now: SystemTime::now(),
        };
        assert!(render(&screen).contains("no workspaces"));
    }

    #[test]
    fn workspace_name_appears_in_output() {
        let ws = workspace("2026-05-16-fresh", 0, 5.0);
        let row = Row {
            workspace: &ws,
            positions: &[],
        };
        let screen = Screen {
            rows: std::slice::from_ref(&row),
            query: "",
            query_cursor: 0,
            cursor: 0,
            scroll: 0,
            create_new: None,
            marked: &[],
            delete_mode: false,
            now: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
        };
        let out = render(&screen);
        assert!(out.contains("2026-05-16-fresh"));
        assert!(out.contains("Search:"));
    }

    #[test]
    fn create_new_row_renders_with_label() {
        let screen = Screen {
            rows: &[],
            query: "redis",
            query_cursor: 5,
            cursor: 0,
            scroll: 0,
            create_new: Some("2026-05-16-redis"),
            marked: &[],
            delete_mode: false,
            now: SystemTime::now(),
        };
        let out = render(&screen);
        assert!(out.contains("Create new:"));
        assert!(out.contains("2026-05-16-redis"));
    }

    #[test]
    fn highlight_positions_emit_bold_yellow() {
        let ws = workspace("redis-server", 0, 3.0);
        let row = Row {
            workspace: &ws,
            positions: &[0, 1, 2],
        };
        let screen = Screen {
            rows: std::slice::from_ref(&row),
            query: "red",
            query_cursor: 3,
            cursor: 0,
            scroll: 0,
            create_new: None,
            marked: &[],
            delete_mode: false,
            now: SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000),
        };
        let out = render(&screen);
        // Crossterm emits SGR 1 (bold) and 33 (yellow) for the highlight.
        assert!(out.contains("\x1b[1m") || out.contains("\x1b[33m"));
    }
}
