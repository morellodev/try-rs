# try-rs

Rust port of [`tobi/try`](https://github.com/tobi/try) — an ephemeral workspace manager. Quickly create, fuzzy-search, and `cd` into date-prefixed experiment directories from your shell.

```
🏠 Try Directory Selection
────────────────────────────────────────────────
Search: redis
────────────────────────────────────────────────
→ 📁 2026-05-16-redis-pool                     just now, 5.2
  📁 2025-08-03-thread-pool                    9m ago, 4.1
  📁 2025-07-22-db-pooling                    2w ago, 0.6
  📂 Create new: 2026-05-16-redis
────────────────────────────────────────────────
↑/↓ Navigate  Enter Select  ^R Rename  ^G Graduate  ^D Delete  Esc Cancel
```

## Install

**Homebrew (macOS / Linux):**

```sh
brew install morellodev/tap/try
```

**From source:**

```sh
cargo install --git https://github.com/morellodev/try-rs
```

Then add the shell wrapper to your rc file:

```sh
# bash / zsh — .bashrc or .zshrc
eval "$(try init)"

# fish — config.fish
try init | source
```

The wrapper is what lets `try` change your shell's `cwd` (a process can't change its parent's directory, so `try` emits a shell script that the wrapper `eval`s).

## Usage

```sh
try                              # open the interactive selector
try redis                        # selector pre-filtered by "redis"
try clone https://github.com/x/y # clone into 2026-05-16-x-y
try https://github.com/x/y       # bare URL also clones
try worktree feature-branch      # detached git worktree from current repo
try . my-experiment              # worktree from current dir
```

Inside the TUI:

| Key | Action |
|---|---|
| `↑` / `↓` or `Ctrl-P` / `Ctrl-N` | Navigate |
| `Enter` | Select highlighted entry, or create from "Create new" row |
| `Ctrl-T` | Create immediately from current query |
| `Ctrl-R` | Rename selected workspace |
| `Ctrl-G` | "Graduate" — move to a permanent location and leave a symlink |
| `Ctrl-D` | Mark for deletion (multiple); press `Enter` then type `YES` to confirm |
| `Ctrl-A` / `Ctrl-E` / `Ctrl-B` / `Ctrl-F` / `Ctrl-K` / `Ctrl-W` | Line editing |
| `Esc` / `Ctrl-C` | Cancel (in delete mode: clear marks instead) |

## Configuration

| Variable | Default | Effect |
|---|---|---|
| `TRY_PATH` | `~/src/tries` | Where workspaces live |
| `TRY_PROJECTS` | parent of `TRY_PATH` | Default destination when graduating |
| `NO_COLOR` | unset | Disable ANSI color output (any non-empty value) |

You can also pass `--path DIR` on any command and `--no-colors` to disable color.

## What's different from the Ruby upstream

Behaviorally identical — the project tracks the upstream [spec test suite](spec/) and **passes all 387 cases**. Implementation differences:

- Native binary; no Ruby runtime needed.
- Color palette redesigned for accessibility — no row backgrounds, emphasis from bold + arrow + icon. Works on light and dark terminal themes.
- `Ctrl-K` is *kill-to-end-of-line* (matches the upstream code; the upstream docs occasionally list it as a navigation alias, but the runtime behavior is the editing one).

## Building

```sh
cargo build --release                                # binary at target/release/try
cargo test                                           # 117 unit + doc tests
./spec/tests/runner.sh ./target/release/try          # full upstream spec runner
```

Requires Rust 1.91+ (edition 2024; uses `str::floor_char_boundary`).

## License

MIT, same as the upstream project.

## Credits

All credit for the original concept, UX, and reference implementation goes to [@tobi](https://github.com/tobi). This port preserves the upstream's behavior contract via the vendored `spec/` test suite.
