//! Domain types for workspaces (a.k.a. "tries") and their roots.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::error::{Error, Result};

/// Absolute path to the directory holding all workspaces.
///
/// Invariants: the path is absolute. Construction does not require the
/// directory to exist; call [`WorkspaceRoot::ensure_exists`] to create it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceRoot(PathBuf);

impl WorkspaceRoot {
    pub fn new(path: PathBuf) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::InvalidRoot(format!(
                "not an absolute path: {}",
                path.display()
            )));
        }
        Ok(Self(path))
    }

    pub fn ensure_exists(&self) -> Result<()> {
        fs::create_dir_all(&self.0).map_err(|source| Error::Io {
            path: self.0.clone(),
            source,
        })
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.0.join(name)
    }
}

/// Destination directory for "graduated" workspaces (the parent of the
/// workspace root by default, or `$TRY_PROJECTS` when set).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectsRoot(PathBuf);

impl ProjectsRoot {
    pub fn new(path: PathBuf) -> Self {
        Self(path)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A single workspace discovered under the [`WorkspaceRoot`].
///
/// `base_score` is the time-aware component of fuzzy ranking — recency plus
/// a date-prefix bonus — computed at scan time. It is frozen for the lifetime
/// of this struct: scores do not drift if the user lingers in the TUI.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub name: String,
    pub path: PathBuf,
    pub is_symlink: bool,
    pub modified: SystemTime,
    pub created: SystemTime,
    pub base_score: f64,
}
