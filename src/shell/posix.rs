//! POSIX single-quote escaping.
//!
//! Mirrors the upstream Ruby `q()` byte-for-byte so emitted scripts remain
//! comparable in golden tests.

use std::path::Path;

/// Wrap `s` in single quotes, escaping any embedded `'` as `'"'"'`.
///
/// # Examples
///
/// ```
/// use try_rs::shell::posix::quote;
///
/// assert_eq!(quote("foo"), "'foo'");
/// assert_eq!(quote("it's"), r#"'it'"'"'s'"#);
/// assert_eq!(quote(""), "''");
/// ```
#[must_use]
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\"'\"'");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Convenience: quote a [`Path`] via its lossy UTF-8 representation. Non-UTF-8
/// path components on Unix are rare enough that this is acceptable for the
/// shell-script emit path.
#[must_use]
pub fn quote_path(p: &Path) -> String {
    quote(&p.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_pair_of_quotes() {
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn alphanumeric_just_wraps() {
        assert_eq!(quote("foo-bar_42"), "'foo-bar_42'");
    }

    #[test]
    fn single_quote_is_escaped() {
        assert_eq!(quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn multiple_single_quotes() {
        assert_eq!(quote("a'b'c"), "'a'\"'\"'b'\"'\"'c'");
    }

    #[test]
    fn whitespace_and_specials_pass_through() {
        assert_eq!(quote("a b $c"), "'a b $c'");
        assert_eq!(quote("rm -rf /"), "'rm -rf /'");
    }
}
