//! Single-level directory scan of a [`WorkspaceRoot`].
//!
//! Filtering rules (mirroring the upstream `load_all_tries`):
//!
//! - skip entries whose name starts with `.`
//! - skip entries that cannot be stat-ed (missing / permission denied)
//! - skip non-directory entries
//! - skip symlinks whose target is unreachable
//!
//! For each surviving entry, [`scan`] computes a `base_score`:
//!
//! ```text
//! base_score = 3 / sqrt(hours_since_mtime + 1) + (date_prefix ? 2.0 : 0.0)
//! ```
//!
//! Symlinks are followed for both the directory check and for the stored
//! `path` (the [`Workspace::path`] is the symlink target's canonical path),
//! but `is_symlink` records that the workspace was reached via a symlink so
//! the TUI can render it differently.

use std::fs;
use std::io;
use std::time::SystemTime;

use crate::clock::{Clock, hours_since};
use crate::error::{Error, Result};
use crate::naming::has_date_prefix;
use crate::workspace::{Workspace, WorkspaceRoot};

/// Scan `root` and return every workspace that survives the filtering rules.
///
/// A missing `root` directory returns `Ok(vec![])` rather than an error — it
/// is normal for a fresh install (the first `mkdir` happens lazily inside the
/// emitted shell script).
pub fn scan(root: &WorkspaceRoot, clock: &dyn Clock) -> Result<Vec<Workspace>> {
    let now = clock.now();
    let path = root.as_path();

    let entries = match fs::read_dir(path) {
        Ok(it) => it,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    let workspaces = entries
        .filter_map(io::Result::ok)
        .filter_map(|entry| inspect(&entry, now))
        .collect();
    Ok(workspaces)
}

fn inspect(entry: &fs::DirEntry, now: SystemTime) -> Option<Workspace> {
    let name = entry.file_name().into_string().ok()?;
    if name.starts_with('.') {
        return None;
    }

    let entry_path = entry.path();

    // First stat without following symlinks so we can detect them.
    let lstat = fs::symlink_metadata(&entry_path).ok()?;
    let is_symlink = lstat.file_type().is_symlink();

    // Resolve through symlinks to get the target's metadata. A broken symlink
    // errors here and is silently skipped.
    let target_meta = if is_symlink {
        fs::metadata(&entry_path).ok()?
    } else {
        lstat
    };
    if !target_meta.is_dir() {
        return None;
    }

    let modified = target_meta.modified().unwrap_or(now);
    let created = target_meta.created().unwrap_or(modified);

    let path = if is_symlink {
        fs::canonicalize(&entry_path).ok()?
    } else {
        entry_path
    };

    Some(Workspace {
        name: name.clone(),
        base_score: compute_base_score(&name, modified, now),
        path,
        is_symlink,
        modified,
        created,
    })
}

fn compute_base_score(name: &str, modified: SystemTime, now: SystemTime) -> f64 {
    let hours = hours_since(now, modified);
    let recency = 3.0 / (hours + 1.0).sqrt();
    if has_date_prefix(name) {
        recency + 2.0
    } else {
        recency
    }
}

/// Test-only [`Clock`] returning a fixed instant.
#[cfg(test)]
struct TestClock {
    now: SystemTime,
}

#[cfg(test)]
impl Clock for TestClock {
    fn today(&self) -> String {
        "2026-05-16".to_string()
    }
    fn now(&self) -> SystemTime {
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::time::Duration;
    use tempfile::TempDir;

    fn root_from(td: &TempDir) -> WorkspaceRoot {
        WorkspaceRoot::new(td.path().to_path_buf()).unwrap()
    }

    fn clock_at(now: SystemTime) -> TestClock {
        TestClock { now }
    }

    #[test]
    fn empty_root_returns_empty_vec() {
        let td = TempDir::new().unwrap();
        let ws = scan(&root_from(&td), &clock_at(SystemTime::now())).unwrap();
        assert!(ws.is_empty());
    }

    #[test]
    fn missing_root_returns_empty_vec() {
        let td = TempDir::new().unwrap();
        let missing = td.path().join("does-not-exist");
        let root = WorkspaceRoot::new(missing).unwrap();
        let ws = scan(&root, &clock_at(SystemTime::now())).unwrap();
        assert!(ws.is_empty());
    }

    #[test]
    fn dot_entries_are_skipped() {
        let td = TempDir::new().unwrap();
        fs::create_dir(td.path().join(".hidden")).unwrap();
        fs::create_dir(td.path().join("visible")).unwrap();
        let ws = scan(&root_from(&td), &clock_at(SystemTime::now())).unwrap();
        let names: Vec<&str> = ws.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["visible"]);
    }

    #[test]
    fn files_are_skipped() {
        let td = TempDir::new().unwrap();
        File::create(td.path().join("not-a-dir")).unwrap();
        fs::create_dir(td.path().join("a-dir")).unwrap();
        let ws = scan(&root_from(&td), &clock_at(SystemTime::now())).unwrap();
        assert_eq!(ws.len(), 1);
        assert_eq!(ws[0].name, "a-dir");
    }

    #[test]
    fn finds_multiple_dirs_unordered() {
        let td = TempDir::new().unwrap();
        for name in ["alpha", "bravo", "2025-01-02-charlie"] {
            fs::create_dir(td.path().join(name)).unwrap();
        }
        let ws = scan(&root_from(&td), &clock_at(SystemTime::now())).unwrap();
        let mut names: Vec<&str> = ws.iter().map(|w| w.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["2025-01-02-charlie", "alpha", "bravo"]);
    }

    #[test]
    fn date_prefix_adds_two_to_base_score() {
        let td = TempDir::new().unwrap();
        fs::create_dir(td.path().join("plain")).unwrap();
        fs::create_dir(td.path().join("2025-01-02-fancy")).unwrap();
        let ws = scan(&root_from(&td), &clock_at(SystemTime::now())).unwrap();
        let plain = ws.iter().find(|w| w.name == "plain").unwrap();
        let fancy = ws.iter().find(|w| w.name == "2025-01-02-fancy").unwrap();
        // Both were just created, so recency components are within a few ms
        // of each other; the only material difference is the +2.0 date bonus.
        assert!(
            (fancy.base_score - plain.base_score - 2.0).abs() < 0.05,
            "fancy={} plain={}",
            fancy.base_score,
            plain.base_score,
        );
    }

    #[test]
    fn fresh_directory_has_recency_score_near_three() {
        // hours_since(now, now) = 0 -> 3 / sqrt(1) = 3.0
        let td = TempDir::new().unwrap();
        fs::create_dir(td.path().join("plain")).unwrap();
        let ws = scan(&root_from(&td), &clock_at(SystemTime::now())).unwrap();
        assert!(
            (ws[0].base_score - 3.0).abs() < 0.05,
            "got {}",
            ws[0].base_score,
        );
    }

    #[test]
    fn compute_base_score_pure_24h_old() {
        // hours = 24 -> 3 / sqrt(25) = 0.6
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400);
        let then = SystemTime::UNIX_EPOCH;
        let score = compute_base_score("plain", then, now);
        assert!((score - 0.6).abs() < 1e-9, "got {score}");
    }

    #[test]
    fn compute_base_score_pure_with_date_prefix_24h_old() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400);
        let then = SystemTime::UNIX_EPOCH;
        let score = compute_base_score("2025-01-02-foo", then, now);
        assert!((score - 2.6).abs() < 1e-9, "got {score}");
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlinks_are_skipped() {
        use std::os::unix::fs::symlink;
        let td = TempDir::new().unwrap();
        symlink(td.path().join("nowhere"), td.path().join("dangling")).unwrap();
        fs::create_dir(td.path().join("real")).unwrap();
        let ws = scan(&root_from(&td), &clock_at(SystemTime::now())).unwrap();
        let names: Vec<&str> = ws.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["real"]);
    }

    #[cfg(unix)]
    #[test]
    fn valid_symlinks_resolve_to_canonical_target() {
        use std::os::unix::fs::symlink;
        let outer = TempDir::new().unwrap();
        let inner = TempDir::new().unwrap();
        let target = inner.path().join("real-target");
        fs::create_dir(&target).unwrap();
        let link = outer.path().join("link");
        symlink(&target, &link).unwrap();

        let root = WorkspaceRoot::new(outer.path().to_path_buf()).unwrap();
        let ws = scan(&root, &clock_at(SystemTime::now())).unwrap();
        assert_eq!(ws.len(), 1);
        assert!(ws[0].is_symlink);
        assert_eq!(ws[0].name, "link");
        assert_eq!(ws[0].path, fs::canonicalize(&target).unwrap());
    }
}
