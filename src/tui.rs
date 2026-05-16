//! Terminal user interface.
//!
//! Boundaries are kept tight:
//!
//! - [`terminal`] owns raw-mode / alt-screen state through an RAII guard.
//! - [`input`] decodes raw `crossterm` events into a [`input::Event`] enum
//!   that the selector loop matches on.
//! - [`render`] turns a [`render::Screen`] view-model into ANSI bytes on a
//!   `Write`. No filesystem, no clock — all data is passed in.
//!
//! Output always goes to `io::stderr()`: stdout is reserved for the shell
//! script that the wrapper `eval`s.

pub mod dialog;
pub mod input;
pub mod render;
pub mod selector;
pub mod terminal;
