//! RAII guard for raw mode + alternate screen.
//!
//! Construction enters raw mode, switches to the alt screen on `stderr`, and
//! hides the cursor. [`Drop`] reverses all three in the inverse order. Because
//! Rust unwinds on panic by default, this also restores the terminal when a
//! panic occurs inside the selector loop.

use std::io::{self, Write};

use crossterm::cursor::{Hide, Show};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use crossterm::{execute, queue};

/// Returned from [`Guard::enter`]; dropping it restores the terminal.
#[derive(Debug)]
pub struct Guard {
    _private: (),
}

impl Guard {
    /// Enter raw mode, switch to the alt screen, hide the cursor.
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut err = io::stderr();
        if let Err(e) = execute!(err, EnterAlternateScreen, Hide) {
            // Best-effort cleanup so we don't leave the terminal in raw mode.
            let _ = disable_raw_mode();
            return Err(e);
        }
        Ok(Self { _private: () })
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let mut err = io::stderr();
        let _ = queue!(err, Show, LeaveAlternateScreen);
        let _ = err.flush();
        let _ = disable_raw_mode();
    }
}
