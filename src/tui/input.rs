//! Input decoding.
//!
//! Crossterm's event enums are flexible but verbose; the selector loop wants a
//! single `match` against a flat `Key` enum. [`read_event`] polls crossterm
//! with a timeout and returns `Option<Event>` so the caller can decide whether
//! to redraw on timeout (e.g. for animated dots or stale-state recovery).

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// What the selector loop reacts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    Resize,
}

/// The selector's key domain — flatter than crossterm's `KeyCode` because most
/// of the bindings we care about are single characters or named keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// `Ctrl-<letter>`. Letter is normalized to lowercase ASCII.
    Ctrl(char),
    Enter,
    Backspace,
    Escape,
    Up,
    Down,
    Left,
    Right,
    /// Anything we don't model — selector ignores these.
    Other,
}

/// Block up to `timeout` waiting for an input event.
///
/// Returns `Ok(None)` on timeout (no event). On Windows we filter to
/// `KeyEventKind::Press` so a single keystroke does not fire twice.
pub fn read_event(timeout: Duration) -> io::Result<Option<Event>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }
    match event::read()? {
        CtEvent::Key(ke) if ke.kind == KeyEventKind::Press => Ok(Some(Event::Key(decode(ke)))),
        CtEvent::Resize(_, _) => Ok(Some(Event::Resize)),
        _ => Ok(None),
    }
}

/// Abstraction over "where do key events come from". The selector and dialogs
/// take a `&mut dyn KeySource` so production runs against crossterm while
/// tests inject scripted key streams.
pub trait KeySource {
    fn read_event(&mut self, timeout: Duration) -> io::Result<Option<Event>>;
}

/// Reads from the real terminal via crossterm.
#[derive(Debug, Default)]
pub struct TerminalKeys;

impl KeySource for TerminalKeys {
    fn read_event(&mut self, timeout: Duration) -> io::Result<Option<Event>> {
        read_event(timeout)
    }
}

/// Plays back a pre-baked queue of keys. When exhausted, emits `Escape`
/// forever so the consumer's loop cancels cleanly instead of spinning on
/// `None` (which would be a "timeout, try again" signal in a real terminal).
#[derive(Debug)]
pub struct ScriptedKeys {
    queue: VecDeque<Key>,
}

impl ScriptedKeys {
    pub fn new(keys: impl IntoIterator<Item = Key>) -> Self {
        Self {
            queue: keys.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl KeySource for ScriptedKeys {
    fn read_event(&mut self, _timeout: Duration) -> io::Result<Option<Event>> {
        let key = self.queue.pop_front().unwrap_or(Key::Escape);
        Ok(Some(Event::Key(key)))
    }
}

/// Parse the upstream `--and-keys` spec language into a sequence of [`Key`]s.
///
/// Two modes are auto-detected:
///
/// - **Token mode** when the spec contains `,` or is composed entirely of
///   ASCII uppercase letters / hyphens. Tokens are comma-separated and may
///   be: `UP`, `DOWN`, `LEFT`, `RIGHT`, `ENTER`, `ESC`, `BACKSPACE`,
///   `CTRL-X` / `CTRLX` (single letter), or `TYPE=<text>` (each character of
///   `<text>` becomes a `Key::Char`).
/// - **Raw mode** otherwise — each character of the spec becomes a
///   `Key::Char`. (This deviates from the Ruby upstream, which also decodes
///   `\x1b[A` sequences in raw mode; we don't, because we operate on `Key`s
///   not raw bytes. Use token mode for arrow keys.)
///
/// Unknown tokens are silently dropped.
#[must_use]
pub fn parse_keys_spec(spec: &str) -> Vec<Key> {
    if spec.is_empty() {
        return Vec::new();
    }

    let token_mode = spec.contains(',')
        || (spec.len() >= 5 && spec[..5].eq_ignore_ascii_case("TYPE="))
        || spec.chars().all(|c| c.is_ascii_uppercase() || c == '-');

    if token_mode {
        spec.split(',')
            .flat_map(|tok| parse_token(tok.trim()))
            .collect()
    } else {
        parse_raw(spec)
    }
}

/// Raw-byte parsing: each byte becomes a [`Key`], with control bytes mapped
/// to their semantic key (CR/LF → Enter, ESC → Escape, 0x7f/0x08 → Backspace,
/// 0x01..0x1a → Ctrl-letter) and `ESC [ A/B/C/D` decoded as arrow keys.
fn parse_raw(spec: &str) -> Vec<Key> {
    let bytes = spec.as_bytes();
    let mut keys = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Arrow escape sequences first.
        if bytes[i] == 0x1b && i + 2 < bytes.len() && bytes[i + 1] == b'[' {
            let arrow = match bytes[i + 2] {
                b'A' => Some(Key::Up),
                b'B' => Some(Key::Down),
                b'C' => Some(Key::Right),
                b'D' => Some(Key::Left),
                _ => None,
            };
            if let Some(k) = arrow {
                keys.push(k);
                i += 3;
                continue;
            }
        }
        keys.push(byte_to_key(bytes[i]));
        i += 1;
    }
    keys
}

fn byte_to_key(byte: u8) -> Key {
    match byte {
        b'\r' | b'\n' => Key::Enter,
        0x1b => Key::Escape,
        0x7f | 0x08 => Key::Backspace,
        0x01..=0x1a => Key::Ctrl((byte + b'a' - 1) as char),
        _ => Key::Char(byte as char),
    }
}

fn parse_token(tok: &str) -> Vec<Key> {
    if tok.is_empty() {
        return Vec::new();
    }
    // TYPE=...: preserve the value's original case
    if tok.len() >= 5 && tok[..5].eq_ignore_ascii_case("TYPE=") {
        return tok[5..].chars().map(Key::Char).collect();
    }

    let upper = tok.to_ascii_uppercase();
    match upper.as_str() {
        "UP" => vec![Key::Up],
        "DOWN" => vec![Key::Down],
        "LEFT" => vec![Key::Left],
        "RIGHT" => vec![Key::Right],
        "ENTER" => vec![Key::Enter],
        "ESC" => vec![Key::Escape],
        "BACKSPACE" => vec![Key::Backspace],
        _ if upper.starts_with("CTRL") => {
            let rest = upper.trim_start_matches("CTRL").trim_start_matches('-');
            rest.chars()
                .next()
                .map(|c| vec![Key::Ctrl(c.to_ascii_lowercase())])
                .unwrap_or_default()
        }
        _ if tok.chars().count() == 1 => vec![Key::Char(tok.chars().next().unwrap())],
        _ => Vec::new(),
    }
}

fn decode(ev: KeyEvent) -> Key {
    let ctrl = ev.modifiers.contains(KeyModifiers::CONTROL);
    match ev.code {
        KeyCode::Char(c) if ctrl => Key::Ctrl(c.to_ascii_lowercase()),
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Esc => Key::Escape,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        _ => Key::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        }
    }

    #[test]
    fn plain_char_decodes_to_char() {
        assert_eq!(
            decode(key(KeyCode::Char('a'), KeyModifiers::NONE)),
            Key::Char('a')
        );
    }

    #[test]
    fn shift_char_keeps_case() {
        // Crossterm reports the shifted character directly.
        assert_eq!(
            decode(key(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            Key::Char('A')
        );
    }

    #[test]
    fn ctrl_char_normalizes_to_lowercase() {
        assert_eq!(
            decode(key(KeyCode::Char('D'), KeyModifiers::CONTROL)),
            Key::Ctrl('d')
        );
        assert_eq!(
            decode(key(KeyCode::Char('p'), KeyModifiers::CONTROL)),
            Key::Ctrl('p')
        );
    }

    #[test]
    fn named_keys_decode() {
        assert_eq!(decode(key(KeyCode::Enter, KeyModifiers::NONE)), Key::Enter);
        assert_eq!(
            decode(key(KeyCode::Backspace, KeyModifiers::NONE)),
            Key::Backspace
        );
        assert_eq!(decode(key(KeyCode::Esc, KeyModifiers::NONE)), Key::Escape);
        assert_eq!(decode(key(KeyCode::Up, KeyModifiers::NONE)), Key::Up);
        assert_eq!(decode(key(KeyCode::Down, KeyModifiers::NONE)), Key::Down);
    }

    #[test]
    fn unknown_keys_collapse_to_other() {
        assert_eq!(decode(key(KeyCode::F(1), KeyModifiers::NONE)), Key::Other);
        assert_eq!(decode(key(KeyCode::Tab, KeyModifiers::NONE)), Key::Other);
    }

    // --- parse_keys_spec ---

    #[test]
    fn parse_keys_spec_empty_returns_empty() {
        assert!(parse_keys_spec("").is_empty());
    }

    #[test]
    fn parse_keys_spec_named_tokens() {
        assert_eq!(
            parse_keys_spec("UP,DOWN,ENTER,ESC,BACKSPACE,LEFT,RIGHT"),
            vec![
                Key::Up,
                Key::Down,
                Key::Enter,
                Key::Escape,
                Key::Backspace,
                Key::Left,
                Key::Right,
            ],
        );
    }

    #[test]
    fn parse_keys_spec_ctrl_variants() {
        assert_eq!(parse_keys_spec("CTRL-D"), vec![Key::Ctrl('d')]);
        assert_eq!(parse_keys_spec("CTRLD"), vec![Key::Ctrl('d')]);
        assert_eq!(
            parse_keys_spec("CTRL-A,CTRL-R"),
            vec![Key::Ctrl('a'), Key::Ctrl('r')]
        );
    }

    #[test]
    fn parse_keys_spec_type_emits_each_char() {
        assert_eq!(
            parse_keys_spec("TYPE=ab"),
            vec![Key::Char('a'), Key::Char('b')],
        );
    }

    #[test]
    fn parse_keys_spec_type_preserves_case() {
        assert_eq!(
            parse_keys_spec("TYPE=YES"),
            vec![Key::Char('Y'), Key::Char('E'), Key::Char('S')],
        );
    }

    #[test]
    fn parse_keys_spec_mixed_grammar() {
        assert_eq!(
            parse_keys_spec("TYPE=redis,DOWN,ENTER"),
            vec![
                Key::Char('r'),
                Key::Char('e'),
                Key::Char('d'),
                Key::Char('i'),
                Key::Char('s'),
                Key::Down,
                Key::Enter,
            ],
        );
    }

    #[test]
    fn parse_keys_spec_raw_mode_each_char() {
        // No commas and not all-uppercase → raw mode.
        assert_eq!(
            parse_keys_spec("abc"),
            vec![Key::Char('a'), Key::Char('b'), Key::Char('c')],
        );
    }

    #[test]
    fn parse_keys_spec_raw_mode_control_bytes() {
        assert_eq!(parse_keys_spec("\r"), vec![Key::Enter]);
        assert_eq!(parse_keys_spec("\n"), vec![Key::Enter]);
        assert_eq!(parse_keys_spec("\x1b"), vec![Key::Escape]);
        assert_eq!(parse_keys_spec("\x7f"), vec![Key::Backspace]);
        assert_eq!(parse_keys_spec("\x04"), vec![Key::Ctrl('d')]);
        assert_eq!(parse_keys_spec("\x01"), vec![Key::Ctrl('a')]);
    }

    #[test]
    fn parse_keys_spec_raw_mode_arrows() {
        assert_eq!(parse_keys_spec("\x1b[A"), vec![Key::Up]);
        assert_eq!(parse_keys_spec("\x1b[B"), vec![Key::Down]);
        assert_eq!(parse_keys_spec("\x1b[C"), vec![Key::Right]);
        assert_eq!(parse_keys_spec("\x1b[D"), vec![Key::Left]);
    }

    #[test]
    fn parse_keys_spec_raw_mode_mixed() {
        // "down arrow + j + enter"
        assert_eq!(
            parse_keys_spec("\x1b[Bj\r"),
            vec![Key::Down, Key::Char('j'), Key::Enter],
        );
    }

    #[test]
    fn parse_keys_spec_unknown_tokens_dropped() {
        assert_eq!(
            parse_keys_spec("UP,WHATEVER,ENTER"),
            vec![Key::Up, Key::Enter]
        );
    }

    // --- ScriptedKeys ---

    #[test]
    fn scripted_keys_replays_then_emits_escape() {
        let mut keys = ScriptedKeys::new(vec![Key::Char('a'), Key::Enter]);
        assert!(matches!(
            keys.read_event(Duration::ZERO).unwrap(),
            Some(Event::Key(Key::Char('a')))
        ));
        assert!(matches!(
            keys.read_event(Duration::ZERO).unwrap(),
            Some(Event::Key(Key::Enter))
        ));
        // Exhausted → Escape forever.
        for _ in 0..3 {
            assert!(matches!(
                keys.read_event(Duration::ZERO).unwrap(),
                Some(Event::Key(Key::Escape))
            ));
        }
    }
}
