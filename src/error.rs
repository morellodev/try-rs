use std::io;
use std::path::PathBuf;

use thiserror::Error;

/// Crate-wide error type.
#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("invalid workspace root: {0}")]
    InvalidRoot(String),

    #[error("invalid name: {0}")]
    InvalidName(String),

    #[error("invalid git URI: {0}")]
    InvalidGitUri(String),
}

pub type Result<T> = std::result::Result<T, Error>;
