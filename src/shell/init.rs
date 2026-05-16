//! Generate the shell wrapper function that users `eval` to get the `try`
//! integration.
//!
//! Each shell's wrapper captures stdout from `try exec ...`, then either
//! `eval`s the script (exit 0 — the success path that performs `cd`,
//! `mkdir`, `git clone`, etc.) or echoes it (non-zero — usage/cancel/error).

use std::path::Path;

use super::kind::Shell;
use super::posix::quote;

/// Render the wrapper function for `shell`.
///
/// - `binary`: absolute path to the `try` binary; embedded literally in the
///   wrapper so the user's `$PATH` is not consulted at call time.
/// - `explicit_path`: if `Some`, the wrapper hard-codes this as the tries
///   root, ignoring `$TRY_PATH` at call time.
/// - `default_path`: the fallback used when `$TRY_PATH` is unset.
#[must_use]
pub fn wrapper(
    shell: Shell,
    binary: &Path,
    explicit_path: Option<&Path>,
    default_path: &Path,
) -> String {
    let binary_q = quote(&binary.to_string_lossy());
    let default = default_path.to_string_lossy();

    match shell {
        Shell::Bash | Shell::Zsh => render_posix(&binary_q, explicit_path, &default),
        Shell::Fish => render_fish(&binary_q, explicit_path, &default),
        Shell::Pwsh => render_pwsh(&binary_q, explicit_path, &default),
    }
}

fn render_posix(binary_q: &str, explicit: Option<&Path>, default: &str) -> String {
    let path_arg = match explicit {
        Some(p) => format!(" --path {}", quote(&p.to_string_lossy())),
        None => format!(" --path \"${{TRY_PATH:-{default}}}\""),
    };

    format!(
        r#"try() {{
  local out
  out=$({binary_q} exec{path_arg} "$@" 2>/dev/tty)
  if [ $? -eq 0 ]; then
    eval "$out"
  else
    echo "$out"
  fi
}}
"#
    )
}

fn render_fish(binary_q: &str, explicit: Option<&Path>, default: &str) -> String {
    let path_arg = match explicit {
        Some(p) => format!(" --path {}", quote(&p.to_string_lossy())),
        None => format!(
            " --path (if set -q TRY_PATH; echo \"$TRY_PATH\"; else; echo '{default}'; end)"
        ),
    };

    format!(
        r#"function try
  set -l out ({binary_q} exec{path_arg} $argv 2>/dev/tty | string collect)
  if test $pipestatus[1] -eq 0
    eval $out
  else
    echo $out
  end
end
"#
    )
}

fn render_pwsh(binary_q: &str, explicit: Option<&Path>, default: &str) -> String {
    let path_expr = match explicit {
        Some(p) => format!("'{}'", p.to_string_lossy()),
        None => format!("$(if ($env:TRY_PATH) {{ $env:TRY_PATH }} else {{ '{default}' }})"),
    };

    format!(
        r#"function try {{
  $tryPath = {path_expr}
  $tempErr = [System.IO.Path]::GetTempFileName()
  $out = & {binary_q} exec --path $tryPath @args 2>$tempErr
  if ($LASTEXITCODE -eq 0) {{
    $out | Invoke-Expression
  }} else {{
    Get-Content $tempErr | Write-Host
    $out | Write-Output
  }}
  Remove-Item $tempErr -ErrorAction SilentlyContinue
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const BIN: &str = "/usr/local/bin/try";
    const DEFAULT: &str = "/home/u/src/tries";

    fn render(shell: Shell, explicit: Option<&str>) -> String {
        wrapper(
            shell,
            Path::new(BIN),
            explicit.map(Path::new),
            Path::new(DEFAULT),
        )
    }

    #[test]
    fn bash_uses_posix_function_syntax_and_quoted_binary() {
        let s = render(Shell::Bash, None);
        assert!(s.starts_with("try() {\n"));
        assert!(s.contains("local out"));
        assert!(s.contains("'/usr/local/bin/try' exec"));
        assert!(s.contains(r#""${TRY_PATH:-/home/u/src/tries}""#));
        assert!(s.contains(r#"eval "$out""#));
    }

    #[test]
    fn zsh_and_bash_render_identically() {
        assert_eq!(render(Shell::Bash, None), render(Shell::Zsh, None));
    }

    #[test]
    fn bash_with_explicit_path_skips_env_fallback() {
        let s = render(Shell::Bash, Some("/opt/tries"));
        assert!(s.contains(" --path '/opt/tries'"));
        assert!(!s.contains("TRY_PATH"));
    }

    #[test]
    fn fish_uses_function_keyword_and_string_collect() {
        let s = render(Shell::Fish, None);
        assert!(s.starts_with("function try\n"));
        assert!(s.contains("string collect"));
        assert!(s.contains("$pipestatus[1]"));
        assert!(s.contains("if set -q TRY_PATH"));
        assert!(s.contains("'/home/u/src/tries'"));
    }

    #[test]
    fn fish_with_explicit_path_inlines_quoted_path() {
        let s = render(Shell::Fish, Some("/opt/tries"));
        assert!(s.contains(" --path '/opt/tries'"));
        assert!(!s.contains("TRY_PATH"));
    }

    #[test]
    fn pwsh_uses_lastexitcode_and_temp_file() {
        let s = render(Shell::Pwsh, None);
        assert!(s.starts_with("function try {\n"));
        assert!(s.contains("$LASTEXITCODE"));
        assert!(s.contains("$env:TRY_PATH"));
        assert!(s.contains("Invoke-Expression"));
        assert!(s.contains("Remove-Item $tempErr"));
    }

    #[test]
    fn pwsh_with_explicit_path_inlines_literal() {
        let s = render(Shell::Pwsh, Some("/opt/tries"));
        assert!(s.contains("$tryPath = '/opt/tries'"));
    }

    #[test]
    fn binary_path_with_quote_is_escaped() {
        // POSIX quoting must survive a binary path containing an apostrophe.
        let s = wrapper(
            Shell::Bash,
            Path::new("/tmp/it's/try"),
            None,
            Path::new(DEFAULT),
        );
        assert!(s.contains(r#"'/tmp/it'"'"'s/try'"#));
    }

    #[test]
    fn snippet_ends_with_newline() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish, Shell::Pwsh] {
            let s = render(shell, None);
            assert!(s.ends_with('\n'), "{shell}: trailing newline missing");
        }
    }
}
