//! Actions the TUI / CLI produce, which the shell emitter serializes.
//!
//! Keeping this enum pure data (no I/O, no formatting concerns) is what lets
//! the TUI be tested without a real terminal and lets emission be snapshot-tested
//! without going through the TUI.

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `cd` into an existing workspace.
    Cd { path: PathBuf },

    /// Create a new directory and `cd` into it.
    Mkdir { path: PathBuf },

    /// Clone a git repo into a new directory and `cd` into it.
    Clone { path: PathBuf, uri: String },

    /// Create a detached git worktree (or a plain directory if not inside a repo).
    Worktree {
        path: PathBuf,
        /// `None` means "use the current working directory".
        repo: Option<PathBuf>,
    },

    /// Remove one or more workspaces, then `cd` back to the root.
    Delete {
        targets: Vec<DeleteTarget>,
        base: PathBuf,
    },

    /// Rename a workspace and `cd` into the new path.
    Rename {
        base: PathBuf,
        from: String,
        to: String,
    },

    /// Move a workspace to a permanent home and leave a symlink behind.
    Graduate {
        source: PathBuf,
        dest: PathBuf,
        basename: String,
        base: PathBuf,
        is_worktree: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTarget {
    /// Canonicalized absolute path of the directory to remove.
    pub real_path: PathBuf,
    /// Basename relative to `base`, used as the script-local identifier.
    pub basename: String,
}
