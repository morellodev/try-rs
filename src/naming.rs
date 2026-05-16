//! Naming rules for workspace directories.
//!
//! Three concerns live here:
//!
//! 1. Detecting the `YYYY-MM-DD-` prefix that triggers the date-bonus in scoring.
//! 2. Normalizing user-supplied names (whitespace → `-`, no other changes).
//! 3. Picking a unique directory name when a same-day collision exists, with
//!    digit-suffix bumping so e.g. `foo3` becomes `foo4`.

use std::path::Path;

const DATE_PREFIX_LEN: usize = 11; // "YYYY-MM-DD-"

/// Returns `true` when `name` starts with `YYYY-MM-DD-` (digits + hyphens, no
/// further validation of month/day ranges — matches the upstream regex
/// `^\d{4}-\d{2}-\d{2}-`).
#[must_use]
pub fn has_date_prefix(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.len() < DATE_PREFIX_LEN {
        return false;
    }
    bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b'-'
}

/// Split a name on its date prefix, returning `(date, rest)`. Returns `None`
/// if the name does not start with a date prefix.
#[must_use]
pub fn split_date_prefix(name: &str) -> Option<(&str, &str)> {
    if has_date_prefix(name) {
        Some((&name[..10], &name[DATE_PREFIX_LEN..]))
    } else {
        None
    }
}

/// Collapse whitespace runs in `input` to single `-` characters. No other
/// normalization is applied; existing dashes are preserved.
#[must_use]
pub fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut prev_dash = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(ch);
            prev_dash = ch == '-';
        }
    }
    out
}

/// Resolve a base name that does not collide with an existing
/// `<date>-<base>` directory under `root`.
///
/// If `base` ends in ASCII digits, those digits are bumped (`foo3` → `foo4`).
/// Otherwise `-N` is appended starting at `N = 2`.
#[must_use]
pub fn resolve_unique_base(root: &Path, date: &str, base: &str) -> String {
    let collides = |cand: &str| root.join(format!("{date}-{cand}")).exists();

    if !collides(base) {
        return base.to_string();
    }

    let digit_start = base
        .bytes()
        .rposition(|b| !b.is_ascii_digit())
        .map_or(0, |i| i + 1);

    if digit_start < base.len() {
        let stem = &base[..digit_start];
        let n: u64 = base[digit_start..].parse().unwrap_or(0);
        (n + 1..)
            .map(|k| format!("{stem}{k}"))
            .find(|cand| !collides(cand))
            .expect("u64 exhausted searching for unique candidate")
    } else {
        (2u64..)
            .map(|k| format!("{base}-{k}"))
            .find(|cand| !collides(cand))
            .expect("u64 exhausted searching for unique candidate")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn date_prefix_detection() {
        assert!(has_date_prefix("2025-01-02-foo"));
        assert!(has_date_prefix("9999-99-99-anything")); // matches regex, not calendar
        assert!(!has_date_prefix("2025-01-02"));        // no trailing hyphen
        assert!(!has_date_prefix("2025-1-02-foo"));     // single-digit month
        assert!(!has_date_prefix("foo-2025-01-02-bar"));
        assert!(!has_date_prefix("short"));
    }

    #[test]
    fn split_date_prefix_extracts_date_and_rest() {
        assert_eq!(
            split_date_prefix("2025-01-02-hello-world"),
            Some(("2025-01-02", "hello-world"))
        );
        assert_eq!(split_date_prefix("not-a-date"), None);
    }

    #[test]
    fn normalize_collapses_whitespace_runs() {
        assert_eq!(normalize("foo bar"), "foo-bar");
        assert_eq!(normalize("foo   bar  baz"), "foo-bar-baz");
        assert_eq!(normalize("already-fine"), "already-fine");
        assert_eq!(normalize(" leading"), "-leading");
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn unique_base_returns_input_when_no_collision() {
        let dir = TempDir::new().unwrap();
        let got = resolve_unique_base(dir.path(), "2025-01-02", "fresh");
        assert_eq!(got, "fresh");
    }

    #[test]
    fn unique_base_bumps_trailing_digits() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("2025-01-02-redis3")).unwrap();
        fs::create_dir(dir.path().join("2025-01-02-redis4")).unwrap();
        let got = resolve_unique_base(dir.path(), "2025-01-02", "redis3");
        assert_eq!(got, "redis5");
    }

    #[test]
    fn unique_base_appends_dash_n_when_no_digits() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("2025-01-02-redis")).unwrap();
        let got = resolve_unique_base(dir.path(), "2025-01-02", "redis");
        assert_eq!(got, "redis-2");

        fs::create_dir(dir.path().join("2025-01-02-redis-2")).unwrap();
        let got = resolve_unique_base(dir.path(), "2025-01-02", "redis");
        assert_eq!(got, "redis-3");
    }
}
