//! Library backing the `try` CLI: ephemeral workspace manager.
//!
//! The crate is split into pure modules (`fuzzy`, `naming`, `git`, `shell::posix`)
//! that contain the bulk of the logic, and thin I/O modules (`clock`, `workspace`)
//! that wrap the system boundary. The binary in `src/main.rs` wires them together.

#![forbid(unsafe_code)]
#![warn(unreachable_pub)]
#![warn(missing_debug_implementations)]

pub mod action;
pub mod clock;
pub mod discover;
pub mod error;
pub mod fuzzy;
pub mod git;
pub mod naming;
pub mod shell;
pub mod tui;
pub mod workspace;

pub use error::{Error, Result};
