use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use try_rs::action::Action;
use try_rs::clock::{Clock, SystemClock};
use try_rs::shell::{emit, init, kind::Shell};
use anstream::AutoStream;

use try_rs::tui::{
    input::{self, ScriptedKeys, TerminalKeys},
    selector, terminal,
};
use try_rs::workspace::{ProjectsRoot, WorkspaceRoot};
use try_rs::{discover, git, naming};

const DEFAULT_SUBDIR: &str = "src/tries";

#[derive(Debug, Parser)]
#[command(name = "try", version, about = "ephemeral workspace manager")]
#[command(disable_version_flag = true)]
struct Cli {
    /// Print version.
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    /// Override the tries directory (default: $TRY_PATH or ~/src/tries).
    #[arg(long, global = true, value_name = "DIR")]
    path: Option<PathBuf>,

    /// Disable ANSI color escapes in TUI output. Equivalent to `NO_COLOR=1`.
    /// (Currently accepted for upstream parity; full color suppression is a
    /// follow-up polish item — the spec tests only inspect structural text.)
    #[arg(long = "no-colors", global = true)]
    no_colors: bool,

    /// Test-only: prefill the selector's search buffer.
    #[arg(long = "and-type", global = true, hide = true, value_name = "QUERY")]
    and_type: Option<String>,

    /// Test-only: render the first frame and exit (effective only when no
    /// --and-keys are provided).
    #[arg(long = "and-exit", global = true, hide = true)]
    and_exit: bool,

    /// Test-only: drive the TUI with a scripted key sequence.
    /// Grammar: `TOKEN[,TOKEN...]` where TOKEN is one of `UP`, `DOWN`,
    /// `LEFT`, `RIGHT`, `ENTER`, `ESC`, `BACKSPACE`, `CTRL-X`, or
    /// `TYPE=<text>`.
    #[arg(long = "and-keys", global = true, hide = true, value_name = "SPEC")]
    and_keys: Option<String>,

    /// Test-only: short-circuit the delete-confirm dialog with this text.
    /// `YES` confirms; anything else cancels.
    #[arg(long = "and-confirm", global = true, hide = true, value_name = "TEXT")]
    and_confirm: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Default)]
struct TestFlags {
    and_type: Option<String>,
    and_exit: bool,
    and_keys: Option<String>,
    and_confirm: Option<String>,
}

impl TestFlags {
    fn is_active(&self) -> bool {
        self.and_exit || self.and_keys.is_some() || self.and_type.is_some()
    }
}

/// `--no-colors` flag OR a non-empty `NO_COLOR` env var disables color output.
fn colors_disabled(flag: bool) -> bool {
    flag || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Output a shell wrapper function for shell integration.
    ///
    /// Add to your shell config:
    ///   bash/zsh: eval "$(try init)"
    ///   fish:     try init | source
    Init {
        /// Hard-code this path into the wrapper instead of reading $TRY_PATH.
        #[arg(value_name = "PATH")]
        explicit_path: Option<PathBuf>,
        /// Force a specific shell (bash, zsh, fish, pwsh) instead of auto-detecting.
        #[arg(long)]
        shell: Option<String>,
    },
    /// Clone a git repository into a date-prefixed directory.
    Clone {
        /// Git URL (https or git@).
        uri: String,
        /// Override the generated directory name.
        name: Option<String>,
    },
    /// Create a detached git worktree from the current repo in a date-prefixed directory.
    Worktree {
        /// Repo path (or the literal "dir" meaning cwd). Treated as the
        /// worktree source when present.
        repo: Option<String>,
        /// Optional name; falls back to `basename(repo)`.
        name: Option<String>,
    },
    /// Used by the shell wrapper to dispatch a command and emit a shell script.
    ///
    /// The wrapper captures stdout: exit 0 -> evaluate, non-zero -> print.
    Exec {
        #[command(subcommand)]
        sub: Option<ExecCommand>,
    },
    /// Any unknown subcommand is interpreted as an initial search query for
    /// the interactive selector (matches upstream `try <query>`).
    #[command(external_subcommand)]
    Search(Vec<String>),
}

#[derive(Debug, Subcommand)]
enum ExecCommand {
    /// Interactive selector (not yet implemented).
    Cd {
        /// Initial filter query; tokens are joined with spaces.
        query: Vec<String>,
    },
    /// Clone a git repository into a date-prefixed directory.
    Clone {
        uri: String,
        name: Option<String>,
    },
    /// Create a detached git worktree from the current repo in a date-prefixed directory.
    Worktree {
        repo: Option<String>,
        name: Option<String>,
    },
    /// Any unknown subcommand is interpreted as an initial search query.
    #[command(external_subcommand)]
    Search(Vec<String>),
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("try: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let clock = SystemClock;
    let root = WorkspaceRoot::new(resolve_root(cli.path.as_deref())?)
        .context("invalid workspace root")?;

    let test = TestFlags {
        and_type: cli.and_type,
        and_exit: cli.and_exit,
        and_keys: cli.and_keys,
        and_confirm: cli.and_confirm,
    };
    let strip_color = colors_disabled(cli.no_colors);

    // `try` with no subcommand is shorthand for `try exec` (the wrapper entry point).
    let cmd = cli.command.unwrap_or(Command::Exec { sub: None });

    match cmd {
        Command::Init { explicit_path, shell } => {
            init_cmd(&root, explicit_path.as_deref(), shell.as_deref())
        }
        Command::Clone { uri, name } => clone(&root, &clock, &uri, name.as_deref()),
        Command::Worktree { repo, name } => {
            worktree(&root, &clock, repo.as_deref(), name.as_deref())
        }
        Command::Exec { sub } => exec(&root, &clock, sub, &test, strip_color),
        Command::Search(words) => route_search(&root, &clock, &words, &test, strip_color),
    }
}

fn init_cmd(
    root: &WorkspaceRoot,
    explicit_path: Option<&Path>,
    shell_override: Option<&str>,
) -> Result<()> {
    let shell = match shell_override {
        Some(s) => s.parse::<Shell>().map_err(anyhow::Error::msg)?,
        None => Shell::detect().context(
            "could not detect shell from $SHELL; pass --shell bash|zsh|fish|pwsh explicitly",
        )?,
    };

    let binary = std::env::current_exe().context("failed to resolve current binary path")?;
    let explicit = explicit_path.map(expand_user).transpose()?;

    let snippet = init::wrapper(shell, &binary, explicit.as_deref(), root.as_path());
    print!("{snippet}");
    Ok(())
}

fn clone(root: &WorkspaceRoot, clock: &dyn Clock, uri: &str, name: Option<&str>) -> Result<()> {
    let dir = if let Some(n) = name
        && !n.is_empty()
    {
        n.to_string()
    } else {
        let parsed =
            git::parse(uri).with_context(|| format!("could not parse git URI: {uri}"))?;
        git::clone_dir_name(&parsed, &clock.today())
    };

    let action = Action::Clone {
        path: root.join(&dir),
        uri: uri.to_string(),
    };
    emit::emit(&mut io::stdout(), &action)?;
    Ok(())
}

fn worktree(
    root: &WorkspaceRoot,
    clock: &dyn Clock,
    repo: Option<&str>,
    name: Option<&str>,
) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to read current directory")?;

    // Resolve repo: "dir" sentinel or absent → cwd; otherwise expand the path.
    let (repo_dir, repo_was_explicit) = match repo {
        None | Some("dir") => (cwd.clone(), false),
        Some(p) => (expand_user(Path::new(p))?, true),
    };

    let base = if let Some(n) = name
        && !n.trim().is_empty()
    {
        naming::normalize(n.trim())
    } else {
        // Fall back to basename of the repo path (canonicalize when possible
        // so `.` and `./foo` resolve to a meaningful name).
        let canonical = std::fs::canonicalize(&repo_dir).unwrap_or_else(|_| repo_dir.clone());
        canonical
            .file_name()
            .and_then(|os| os.to_str())
            .map(naming::normalize)
            .context("could not derive a name from the repo path")?
    };

    let date = clock.today();
    let unique = naming::resolve_unique_base(root.as_path(), &date, &base);
    let dir_name = format!("{date}-{unique}");

    let action = Action::Worktree {
        path: root.join(dir_name),
        repo: repo_was_explicit.then_some(repo_dir),
    };
    emit::emit(&mut io::stdout(), &action)?;
    Ok(())
}

fn exec(
    root: &WorkspaceRoot,
    clock: &dyn Clock,
    sub: Option<ExecCommand>,
    test: &TestFlags,
    strip_color: bool,
) -> Result<()> {
    match sub {
        Some(ExecCommand::Clone { uri, name }) => clone(root, clock, &uri, name.as_deref()),
        Some(ExecCommand::Worktree { repo, name }) => {
            worktree(root, clock, repo.as_deref(), name.as_deref())
        }
        Some(ExecCommand::Cd { query }) => route_cd_query(root, clock, &query, test, strip_color),
        Some(ExecCommand::Search(words)) => route_search(root, clock, &words, test, strip_color),
        None => exec_cd(root, clock, "", test, strip_color),
    }
}

/// Handle the `cd [query...]` form: detect a leading git URL and route to
/// the clone flow; otherwise fall through to the selector with `query` as
/// the initial filter.
fn route_cd_query(
    root: &WorkspaceRoot,
    clock: &dyn Clock,
    words: &[String],
    test: &TestFlags,
    strip_color: bool,
) -> Result<()> {
    if let Some(first) = words.first()
        && git::looks_like_uri(first)
    {
        let name = (words.len() > 1).then(|| words[1].clone());
        return clone(root, clock, first, name.as_deref());
    }
    exec_cd(root, clock, &words.join(" "), test, strip_color)
}

/// Handle the catch-all "unknown subcommand" path: route git URLs to clone,
/// `.` / `./path` to worktree, otherwise treat the words as an initial
/// selector query.
fn route_search(
    root: &WorkspaceRoot,
    clock: &dyn Clock,
    words: &[String],
    test: &TestFlags,
    strip_color: bool,
) -> Result<()> {
    if let Some(first) = words.first() {
        if git::looks_like_uri(first) {
            let name = (words.len() > 1).then(|| words[1].clone());
            return clone(root, clock, first, name.as_deref());
        }
        if first == "." || first.starts_with("./") {
            let custom = words[1..].join(" ");
            if first == "." && custom.trim().is_empty() {
                bail!("'try .' requires a name argument");
            }
            let name_opt = (!custom.is_empty()).then_some(custom.as_str());
            return worktree(root, clock, Some(first), name_opt);
        }
    }
    exec_cd(root, clock, &words.join(" "), test, strip_color)
}

fn exec_cd(
    root: &WorkspaceRoot,
    clock: &dyn Clock,
    query: &str,
    test: &TestFlags,
    strip_color: bool,
) -> Result<()> {
    let workspaces = discover::scan(root, clock).context("failed to list workspaces")?;
    let projects = resolve_projects_root(root);

    // `--and-type` overrides the positional query (matches upstream).
    let initial_query = test.and_type.as_deref().unwrap_or(query);
    let confirm = test.and_confirm.as_deref();

    let action = if test.is_active() {
        // Test mode: skip terminal setup so captured stderr is raw frame bytes
        // without alt-screen escape codes.
        let mut keys = ScriptedKeys::new(input::parse_keys_spec(
            test.and_keys.as_deref().unwrap_or(""),
        ));
        run_selector(
            strip_color,
            &mut keys,
            root,
            &projects,
            clock,
            &workspaces,
            initial_query,
            confirm,
        )?
    } else {
        let _guard = terminal::Guard::enter().context("failed to enter alt-screen / raw mode")?;
        let mut keys: TerminalKeys = TerminalKeys;
        run_selector(
            strip_color,
            &mut keys,
            root,
            &projects,
            clock,
            &workspaces,
            initial_query,
            confirm,
        )?
        // Guard drops here: terminal restored before we touch stdout.
    };

    let Some(action) = action else {
        // Cancelled: exit non-zero with no stdout. The wrapper will `echo ""`
        // which is a no-op, leaving the user where they were.
        bail!("cancelled");
    };

    emit::emit(&mut io::stdout(), &action)?;
    Ok(())
}

/// Common selector entrypoint that conditionally wraps stderr in a
/// color-stripping writer.
#[allow(clippy::too_many_arguments)]
fn run_selector(
    strip_color: bool,
    keys: &mut dyn try_rs::tui::input::KeySource,
    root: &WorkspaceRoot,
    projects: &ProjectsRoot,
    clock: &dyn Clock,
    workspaces: &[try_rs::workspace::Workspace],
    initial_query: &str,
    confirm: Option<&str>,
) -> Result<Option<Action>> {
    // `AutoStream::never` strips SGR (color/attribute) escapes while
    // preserving positional escapes; `AutoStream::always` is a transparent
    // pass-through that keeps colors even when stderr is captured (the spec
    // test harness relies on this).
    if strip_color {
        let mut out = AutoStream::never(io::stderr());
        Ok(selector::run(
            &mut out,
            keys,
            root,
            projects,
            clock,
            workspaces,
            initial_query,
            confirm,
        )?)
    } else {
        let mut out = AutoStream::always(io::stderr());
        Ok(selector::run(
            &mut out,
            keys,
            root,
            projects,
            clock,
            workspaces,
            initial_query,
            confirm,
        )?)
    }
}

fn resolve_projects_root(root: &WorkspaceRoot) -> ProjectsRoot {
    if let Some(env) = std::env::var_os("TRY_PROJECTS")
        && let Ok(expanded) = expand_user(Path::new(&env))
    {
        return ProjectsRoot::new(expanded);
    }
    let parent = root
        .as_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.as_path().to_path_buf());
    ProjectsRoot::new(parent)
}

fn resolve_root(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return expand_user(p);
    }
    if let Some(env) = std::env::var_os("TRY_PATH") {
        return expand_user(Path::new(&env));
    }
    Ok(std::env::home_dir().context("HOME is not set")?.join(DEFAULT_SUBDIR))
}

fn expand_user(p: &Path) -> Result<PathBuf> {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        return Ok(std::env::home_dir().context("HOME is not set")?.join(rest));
    }
    if s == "~" {
        return std::env::home_dir().context("HOME is not set");
    }
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    // Resolve relative paths against the current working directory.
    Ok(std::env::current_dir()?.join(p))
}

