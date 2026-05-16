//! Render an [`Action`] as a shell script on a writer.
//!
//! Commands are joined with ` && \` and continuation lines are indented by two
//! spaces, matching the upstream emitter so existing golden tests transfer.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::action::{Action, DeleteTarget};
use crate::shell::posix::{quote, quote_path};

const WARNING: &str =
    "# if you can read this, you didn't launch try from an alias. run try --help.";

/// Emit a shell script for `action` to `out`. The current working directory
/// is captured automatically for the `Action::Delete` "cd back if pwd was
/// removed" safety command. Use [`emit_with_pwd`] in tests to pin pwd.
pub fn emit<W: Write>(out: &mut W, action: &Action) -> io::Result<()> {
    let pwd = std::env::current_dir().ok();
    emit_with_pwd(out, action, pwd.as_deref())
}

/// Variant of [`emit`] that takes the working directory explicitly. Production
/// code uses [`emit`]; tests use this to get deterministic snapshot output.
pub fn emit_with_pwd<W: Write>(
    out: &mut W,
    action: &Action,
    pwd: Option<&Path>,
) -> io::Result<()> {
    let cmds = commands(action, pwd);
    writeln!(out, "{WARNING}")?;
    let last = cmds.len().saturating_sub(1);
    for (i, cmd) in cmds.iter().enumerate() {
        let prefix = if i == 0 { "" } else { "  " };
        if i < last {
            writeln!(out, "{prefix}{cmd} && \\")?;
        } else {
            writeln!(out, "{prefix}{cmd}")?;
        }
    }
    Ok(())
}

fn commands(action: &Action, pwd: Option<&Path>) -> Vec<String> {
    match action {
        Action::Cd { path } => cd_cmds(path),

        Action::Mkdir { path } => {
            let mut v = vec![format!("mkdir -p {}", quote_path(path))];
            v.extend(cd_cmds(path));
            v
        }

        Action::Clone { path, uri } => {
            let qp = quote_path(path);
            let mut v = vec![
                format!("mkdir -p {qp}"),
                format!(
                    "echo {}",
                    quote(&format!("Using git clone to create this trial from {uri}."))
                ),
                format!("git clone {} {qp}", quote(uri)),
            ];
            v.extend(cd_cmds(path));
            v
        }

        Action::Worktree { path, repo } => {
            let qp = quote_path(path);
            let (msg_src, inner) = match repo {
                Some(r) => {
                    let qr = quote_path(r);
                    (
                        r.display().to_string(),
                        format!(
                            "/usr/bin/env sh -c 'if git -C {qr} rev-parse --is-inside-work-tree >/dev/null 2>&1; then repo=$(git -C {qr} rev-parse --show-toplevel); git -C \"$repo\" worktree add --detach {qp} >/dev/null 2>&1 || true; fi; exit 0'"
                        ),
                    )
                }
                None => (
                    "current directory".to_string(),
                    format!(
                        "/usr/bin/env sh -c 'if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then repo=$(git rev-parse --show-toplevel); git -C \"$repo\" worktree add --detach {qp} >/dev/null 2>&1 || true; fi; exit 0'"
                    ),
                ),
            };
            let mut v = vec![
                format!("mkdir -p {qp}"),
                format!(
                    "echo {}",
                    quote(&format!("Using git worktree to create this trial from {msg_src}."))
                ),
                inner,
            ];
            v.extend(cd_cmds(path));
            v
        }

        Action::Delete { targets, base } => {
            let qbase = quote_path(base);
            let mut v = vec![format!("cd {qbase}")];
            for DeleteTarget { basename, .. } in targets {
                let qb = quote(basename);
                v.push(format!("test -d {qb} && rm -rf {qb}"));
            }
            // The Ruby version captures `Dir.pwd` at emit-time and falls back
            // to `base` if that pwd no longer exists. We do the same here so
            // the shell ends up in a valid directory.
            let pwd_buf: PathBuf = pwd.map_or_else(|| base.clone(), Path::to_path_buf);
            v.push(format!("cd {} 2>/dev/null || cd {qbase}", quote_path(&pwd_buf)));
            v
        }

        Action::Rename { base, from, to } => {
            let new_path = base.join(to);
            vec![
                format!("cd {}", quote_path(base)),
                format!("mv {} {}", quote(from), quote(to)),
                format!("echo {}", quote_path(&new_path)),
                format!("cd {}", quote_path(&new_path)),
            ]
        }

        Action::Graduate {
            source,
            dest,
            basename,
            base,
            is_worktree,
        } => {
            let symlink_path = base.join(basename);
            let mv_cmd = if *is_worktree {
                format!(
                    "git worktree move {} {}",
                    quote_path(source),
                    quote_path(dest)
                )
            } else {
                format!("mv {} {}", quote_path(source), quote_path(dest))
            };
            let mut v = vec![
                mv_cmd,
                format!("ln -s {} {}", quote_path(dest), quote_path(&symlink_path)),
                format!(
                    "echo {}",
                    quote(&format!("Graduated: {basename} → {}", dest.display()))
                ),
            ];
            v.extend(cd_cmds(dest));
            v
        }
    }
}

fn cd_cmds(path: &Path) -> Vec<String> {
    let q = quote_path(path);
    vec![
        format!("touch {q}"),
        format!("echo {q}"),
        format!("cd {q}"),
    ]
}

#[cfg(test)]
mod tests {
    //! Byte-exact snapshot tests of the emitted shell scripts.
    //!
    //! These pin the contract with the shell wrapper: the wrapper `eval`s
    //! whatever we print here, so any drift in spacing, quoting, or chaining
    //! is a regression. Snapshots can be regenerated with:
    //!
    //! ```text
    //! SNAPSHOTS=overwrite cargo test --lib shell::emit
    //! ```
    //!
    //! Tests pass `pwd` explicitly so output is deterministic across machines.
    use super::*;
    use snapbox::{assert_data_eq, str};
    use std::path::PathBuf;

    fn emit_str(action: &Action) -> String {
        let mut buf = Vec::new();
        emit_with_pwd(&mut buf, action, Some(Path::new("/test-pwd"))).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn cd_snapshot() {
        let out = emit_str(&Action::Cd {
            path: PathBuf::from("/tmp/foo"),
        });
        assert_data_eq!(
            out,
            str![[r#"
# if you can read this, you didn't launch try from an alias. run try --help.
touch '/tmp/foo' && /
  echo '/tmp/foo' && /
  cd '/tmp/foo'

"#]]
        );
    }

    #[test]
    fn mkdir_snapshot() {
        let out = emit_str(&Action::Mkdir {
            path: PathBuf::from("/tmp/foo bar"),
        });
        assert_data_eq!(
            out,
            str![[r#"
# if you can read this, you didn't launch try from an alias. run try --help.
mkdir -p '/tmp/foo bar' && /
  touch '/tmp/foo bar' && /
  echo '/tmp/foo bar' && /
  cd '/tmp/foo bar'

"#]]
        );
    }

    #[test]
    fn clone_snapshot() {
        let out = emit_str(&Action::Clone {
            path: PathBuf::from("/tmp/2025-01-02-tobi-try"),
            uri: "https://github.com/tobi/try.git".into(),
        });
        assert_data_eq!(
            out,
            str![[r#"
# if you can read this, you didn't launch try from an alias. run try --help.
mkdir -p '/tmp/2025-01-02-tobi-try' && /
  echo 'Using git clone to create this trial from https://github.com/tobi/try.git.' && /
  git clone 'https://github.com/tobi/try.git' '/tmp/2025-01-02-tobi-try' && /
  touch '/tmp/2025-01-02-tobi-try' && /
  echo '/tmp/2025-01-02-tobi-try' && /
  cd '/tmp/2025-01-02-tobi-try'

"#]]
        );
    }

    #[test]
    fn worktree_with_repo_snapshot() {
        let out = emit_str(&Action::Worktree {
            path: PathBuf::from("/tmp/wt"),
            repo: Some(PathBuf::from("/home/me/repo")),
        });
        assert_data_eq!(
            out,
            str![[r##"
# if you can read this, you didn't launch try from an alias. run try --help.
mkdir -p '/tmp/wt' && /
  echo 'Using git worktree to create this trial from /home/me/repo.' && /
  /usr/bin/env sh -c 'if git -C '/home/me/repo' rev-parse --is-inside-work-tree >/dev/null 2>&1; then repo=$(git -C '/home/me/repo' rev-parse --show-toplevel); git -C "$repo" worktree add --detach '/tmp/wt' >/dev/null 2>&1 || true; fi; exit 0' && /
  touch '/tmp/wt' && /
  echo '/tmp/wt' && /
  cd '/tmp/wt'

"##]]
        );
    }

    #[test]
    fn worktree_without_repo_snapshot() {
        let out = emit_str(&Action::Worktree {
            path: PathBuf::from("/tmp/wt"),
            repo: None,
        });
        assert_data_eq!(
            out,
            str![[r##"
# if you can read this, you didn't launch try from an alias. run try --help.
mkdir -p '/tmp/wt' && /
  echo 'Using git worktree to create this trial from current directory.' && /
  /usr/bin/env sh -c 'if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then repo=$(git rev-parse --show-toplevel); git -C "$repo" worktree add --detach '/tmp/wt' >/dev/null 2>&1 || true; fi; exit 0' && /
  touch '/tmp/wt' && /
  echo '/tmp/wt' && /
  cd '/tmp/wt'

"##]]
        );
    }

    #[test]
    fn delete_snapshot() {
        let out = emit_str(&Action::Delete {
            targets: vec![
                DeleteTarget {
                    real_path: PathBuf::from("/tmp/tries/a"),
                    basename: "a".into(),
                },
                DeleteTarget {
                    real_path: PathBuf::from("/tmp/tries/b"),
                    basename: "b".into(),
                },
            ],
            base: PathBuf::from("/tmp/tries"),
        });
        assert_data_eq!(
            out,
            str![[r#"
# if you can read this, you didn't launch try from an alias. run try --help.
cd '/tmp/tries' && /
  test -d 'a' && rm -rf 'a' && /
  test -d 'b' && rm -rf 'b' && /
  cd '/test-pwd' 2>/dev/null || cd '/tmp/tries'

"#]]
        );
    }

    #[test]
    fn rename_snapshot() {
        let out = emit_str(&Action::Rename {
            base: PathBuf::from("/tmp/tries"),
            from: "old-name".into(),
            to: "new-name".into(),
        });
        assert_data_eq!(
            out,
            str![[r#"
# if you can read this, you didn't launch try from an alias. run try --help.
cd '/tmp/tries' && /
  mv 'old-name' 'new-name' && /
  echo '/tmp/tries/new-name' && /
  cd '/tmp/tries/new-name'

"#]]
        );
    }

    #[test]
    fn graduate_with_worktree_snapshot() {
        let out = emit_str(&Action::Graduate {
            source: PathBuf::from("/tmp/tries/2025-01-02-app"),
            dest: PathBuf::from("/tmp/projects/app"),
            basename: "2025-01-02-app".into(),
            base: PathBuf::from("/tmp/tries"),
            is_worktree: true,
        });
        assert_data_eq!(
            out,
            str![[r#"
# if you can read this, you didn't launch try from an alias. run try --help.
git worktree move '/tmp/tries/2025-01-02-app' '/tmp/projects/app' && /
  ln -s '/tmp/projects/app' '/tmp/tries/2025-01-02-app' && /
  echo 'Graduated: 2025-01-02-app → /tmp/projects/app' && /
  touch '/tmp/projects/app' && /
  echo '/tmp/projects/app' && /
  cd '/tmp/projects/app'

"#]]
        );
    }

    #[test]
    fn graduate_without_worktree_snapshot() {
        let out = emit_str(&Action::Graduate {
            source: PathBuf::from("/tmp/tries/x"),
            dest: PathBuf::from("/tmp/projects/x"),
            basename: "x".into(),
            base: PathBuf::from("/tmp/tries"),
            is_worktree: false,
        });
        assert_data_eq!(
            out,
            str![[r#"
# if you can read this, you didn't launch try from an alias. run try --help.
mv '/tmp/tries/x' '/tmp/projects/x' && /
  ln -s '/tmp/projects/x' '/tmp/tries/x' && /
  echo 'Graduated: x → /tmp/projects/x' && /
  touch '/tmp/projects/x' && /
  echo '/tmp/projects/x' && /
  cd '/tmp/projects/x'

"#]]
        );
    }
}
