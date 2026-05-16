//! Time abstractions.
//!
//! Production code uses [`SystemClock`], which delegates to `jiff` for local-date
//! formatting and `std::time` for relative-duration math. Tests inject a fake
//! [`Clock`] to make scoring deterministic.

use std::time::SystemTime;

use jiff::Zoned;

pub trait Clock: Send + Sync {
    /// Current local date in `YYYY-MM-DD` form.
    fn today(&self) -> String;

    /// Current wall-clock instant, comparable to `std::fs::Metadata::modified()`.
    fn now(&self) -> SystemTime;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn today(&self) -> String {
        Zoned::now().strftime("%Y-%m-%d").to_string()
    }

    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// Hours elapsed between two instants, clamped to `0` when `then` is in the future.
#[must_use]
pub fn hours_since(now: SystemTime, then: SystemTime) -> f64 {
    now.duration_since(then)
        .map_or(0.0, |d| d.as_secs_f64() / 3600.0)
}

/// Format a duration like the upstream TUI: `"just now"`, `"5m ago"`,
/// `"2h ago"`, `"3d ago"`, `"1w ago"`.
#[must_use]
pub fn format_relative(now: SystemTime, then: SystemTime) -> String {
    let secs = now.duration_since(then).map_or(0, |d| d.as_secs());
    let mins = secs / 60;
    let hours = mins / 60;
    let days = hours / 24;

    if secs < 60 {
        "just now".to_string()
    } else if mins < 60 {
        format!("{mins}m ago")
    } else if hours < 24 {
        format!("{hours}h ago")
    } else if days < 7 {
        format!("{days}d ago")
    } else {
        format!("{}w ago", days / 7)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn hours_since_is_zero_when_then_is_after_now() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(200);
        assert_eq!(hours_since(now, then), 0.0);
    }

    #[test]
    fn hours_since_computes_difference() {
        let then = SystemTime::UNIX_EPOCH;
        let now = then + Duration::from_secs(3600);
        assert!((hours_since(now, then) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn system_clock_today_is_iso_date() {
        let s = SystemClock.today();
        assert_eq!(s.len(), 10, "got {s}");
        let bytes = s.as_bytes();
        assert!(bytes[4] == b'-' && bytes[7] == b'-');
    }

    #[test]
    fn format_relative_buckets() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000_000);
        let sub = |secs| base - Duration::from_secs(secs);
        assert_eq!(format_relative(base, base), "just now");
        assert_eq!(format_relative(base, sub(30)), "just now");
        assert_eq!(format_relative(base, sub(60)), "1m ago");
        assert_eq!(format_relative(base, sub(5 * 60)), "5m ago");
        assert_eq!(format_relative(base, sub(60 * 60)), "1h ago");
        assert_eq!(format_relative(base, sub(2 * 3600)), "2h ago");
        assert_eq!(format_relative(base, sub(24 * 3600)), "1d ago");
        assert_eq!(format_relative(base, sub(3 * 86_400)), "3d ago");
        assert_eq!(format_relative(base, sub(7 * 86_400)), "1w ago");
        assert_eq!(format_relative(base, sub(14 * 86_400)), "2w ago");
    }

    #[test]
    fn format_relative_clamps_future_to_just_now() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let future = base + Duration::from_secs(60);
        assert_eq!(format_relative(base, future), "just now");
    }
}
