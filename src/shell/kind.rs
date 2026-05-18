//! Shell identification.
//!
//! [`Shell::detect`] inspects environment variables to choose a shell. The
//! pure form [`detect_from`] takes those values as arguments so the logic is
//! unit-testable without mutating the process env.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Pwsh,
}

impl Shell {
    /// Canonical lower-case name (`"bash"`, `"zsh"`, ...).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Zsh => "zsh",
            Self::Fish => "fish",
            Self::Pwsh => "pwsh",
        }
    }

    /// Detect the user's shell from the process environment.
    #[must_use]
    pub fn detect() -> Option<Self> {
        detect_from(
            std::env::var("SHELL").ok().as_deref(),
            std::env::var("PSModulePath").ok().as_deref(),
        )
    }
}

impl fmt::Display for Shell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Shell {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "bash" => Ok(Self::Bash),
            "zsh" => Ok(Self::Zsh),
            "fish" => Ok(Self::Fish),
            "pwsh" | "powershell" => Ok(Self::Pwsh),
            other => Err(format!("unknown shell: {other}")),
        }
    }
}

/// Pure shell-detection logic. `$SHELL` wins when it contains a known marker;
/// otherwise a non-empty `PSModulePath` indicates PowerShell. Returns `None`
/// when no heuristic applies.
#[must_use]
pub fn detect_from(shell_env: Option<&str>, ps_module_path: Option<&str>) -> Option<Shell> {
    if let Some(s) = shell_env {
        if s.contains("fish") {
            return Some(Shell::Fish);
        }
        if s.contains("zsh") {
            return Some(Shell::Zsh);
        }
        if s.contains("bash") {
            return Some(Shell::Bash);
        }
    }
    if ps_module_path.is_some_and(|p| !p.is_empty()) {
        return Some(Shell::Pwsh);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_picks_fish_first() {
        assert_eq!(
            detect_from(Some("/usr/local/bin/fish"), None),
            Some(Shell::Fish)
        );
    }

    #[test]
    fn detect_picks_zsh() {
        assert_eq!(detect_from(Some("/bin/zsh"), None), Some(Shell::Zsh));
    }

    #[test]
    fn detect_picks_bash() {
        assert_eq!(detect_from(Some("/bin/bash"), None), Some(Shell::Bash));
    }

    #[test]
    fn detect_falls_through_to_pwsh_when_ps_module_path_set() {
        assert_eq!(detect_from(None, Some("/some/path")), Some(Shell::Pwsh));
    }

    #[test]
    fn detect_returns_none_when_no_signal() {
        assert_eq!(detect_from(None, None), None);
        assert_eq!(detect_from(Some("/bin/tcsh"), None), None);
        assert_eq!(detect_from(Some("/bin/tcsh"), Some("")), None);
    }

    #[test]
    fn from_str_accepts_canonical_names() {
        assert_eq!("bash".parse::<Shell>().unwrap(), Shell::Bash);
        assert_eq!("ZSH".parse::<Shell>().unwrap(), Shell::Zsh);
        assert_eq!("PowerShell".parse::<Shell>().unwrap(), Shell::Pwsh);
        assert!("tcsh".parse::<Shell>().is_err());
    }

    #[test]
    fn display_is_canonical_name() {
        assert_eq!(Shell::Bash.to_string(), "bash");
        assert_eq!(Shell::Pwsh.to_string(), "pwsh");
    }
}
