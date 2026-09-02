# nemux

A modern, daemon-backed terminal multiplexer. Sessions live in a background
daemon that starts automatically and keeps running independently of any
terminal you attach from — think tmux/screen, rebuilt around a client/server
split with a first-class TUI tab bar instead of prefix-key chords.

```
nemux                # open the tab overview, switch with a keystroke
nemux codex           # run `codex` in a new tab and attach to it
nemux -t codex        # jump to a tab whose title/program matches "codex"
nemux -k 2             # kill tab 2
```

## Features

- **Daemon-backed sessions.** The first `nemux` invocation spawns a detached
  background daemon (survives the launching shell exiting); every later
  invocation talks to it over a local socket. Tabs and their child processes
  outlive your terminal.
- **Tabs, not windows.** Open, switch, rename and close tabs from a bottom
  status bar, or jump straight to one from the command line by id, title or
  program name.
- **Real terminal emulation per tab**, powered by `vt100`: colors, cursor
  shape, alternate screen apps (vim, htop, ...), and OSC title updates all
  work.
- **Exited tabs stay put.** A process ending doesn't close its tab: nemux
  marks it `[exited]`, keeps its final screen (up to `scrollback_lines`) so
  you can see what happened, and leaves it in the list until you dismiss it —
  same close-tab key/command as always, applied twice (once to kill, once to
  dismiss).
- **The daemon shuts itself down** once the last tab is dismissed — like a
  tmux server exiting when its last session is killed — so nothing lingers
  once you're done.
- **Configurable keybindings** (`config.toml`), sensible defaults, no
  mandatory prefix key.
- **Cross-platform.** PTYs via `portable-pty` (ConPTY on Windows, real PTYs on
  Unix), local sockets via `interprocess` (named pipes on Windows, Unix domain
  sockets elsewhere).

## Install / build

```sh
cargo build --release
# binary at target/release/nemux[.exe]
```

Requires a stable Rust toolchain (2021 edition). No system dependencies beyond
a C toolchain for a couple of transitive crates.

## Usage

```
nemux                          Open the tab overview / attach to the last tab
nemux <PROGRAM> [ARGS...]      Open PROGRAM in a new tab and attach to it
nemux new <PROGRAM> [ARGS...]  Same, for programs that collide with a nemux
                                subcommand name (e.g. `nemux new ls`)
nemux -t <SELECTOR>            Attach to a tab by id, title or program name
nemux -k <SELECTOR>            Kill (or, if already exited, dismiss) a tab
nemux attach <SELECTOR>        Same as -t
nemux kill <SELECTOR>          Same as -k
nemux rename <SELECTOR> <TITLE> Rename a tab
nemux ls | list                 List open tabs (plain text, non-interactive)
nemux status                   Show whether the daemon is running
nemux kill-server              Kill every tab and stop the daemon
nemux restart-server           Restart the daemon (open tabs are lost)
nemux config                   Print the config file path
nemux -h | --help              Show help
nemux -V | --version           Show the version
```

### Selectors

A *selector* (used by `-t`/`attach`, `-k`/`kill`, `rename`) is tried in order
as:

1. An exact numeric tab id (`nemux -t 0`).
2. A case-insensitive substring match against the tab's title **or** its
   program name (`nemux -t codex` matches a tab titled "codex session" or
   running `/usr/bin/codex`).

If more than one tab matches, nemux prints a numbered list and asks you to
pick — from the shell for `-t`/`-k`/`rename`, or as an in-TUI popup for the
picker keybind (see below).

### Opening a specific program

`nemux <program> [args...]` is the fast path: it creates a new tab running
that exact command line and attaches to it immediately. Since a handful of
words are reserved as subcommands (`ls`, `kill`, `attach`, `rename`,
`kill-server`, `restart-server`, `status`, `config`, `new`, `run`), use
`nemux new ls` (or `nemux run ls`) if you actually want to open a program
named e.g. `ls` in a tab.

### Tab lifecycle

When a tab's process exits, the tab is **not** removed. It's marked
`[exited]` in the tab bar (with the exit code, if nonzero, shown while it's
the attached tab) and keeps whatever was on screen — up to `scrollback_lines`
of it — so you can read what happened before deciding what to do next.

Killing a tab (`Alt+X`, `nemux -k <selector>`, `nemux kill <selector>`) is
therefore a two-step action on the same key/command:

1. On a **running** tab: sends a termination signal to its process. The tab
   stays listed, now waiting to be marked exited.
2. On an **already-exited** tab: dismisses it — this is the step that
   actually removes it from the list.

Once the last tab is dismissed this way, the daemon shuts itself down
automatically (see [Architecture](#architecture)) — there's no need to run
`kill-server` by hand after closing everything from inside the TUI.

## Keybindings

Configured in `config.toml` (path shown by `nemux config`, created with these
defaults on first run):

| Action           | Default       |
|------------------|---------------|
| New tab (shell)  | `Alt+C`       |
| Next tab         | `Alt+Right`   |
| Previous tab     | `Alt+Left`    |
| Close current tab| `Alt+X`       |
| Rename current tab | `Alt+R`     |
| Tab picker (fuzzy switch) | `Alt+/` |
| Detach (daemon keeps running) | `Alt+D` |

Any key not bound to an action is forwarded to the attached program as
terminal input, including `Ctrl+C` — so it reaches your shell/program as
usual, not nemux.

Keys are written as `Modifier+Modifier+Key`, e.g. `"Ctrl+Shift+n"`,
`"Alt+Right"`, `"F2"`. Supported modifiers: `Ctrl`, `Alt`, `Shift`.

```toml
scrollback_lines = 5000

[keybindings]
new_tab = "Alt+c"
next_tab = "Alt+Right"
prev_tab = "Alt+Left"
close_tab = "Alt+x"
rename_tab = "Alt+r"
detach = "Alt+d"
picker = "Alt+/"
```

A top-level `shell = "..."` key (absent by default, so not shown in the
generated file above) overrides the program used for new tabs opened without
an explicit command; without it, nemux uses `$SHELL` on Unix and `%COMSPEC%`
on Windows.

## Architecture

```
   nemux (client, any invocation)  <── local socket ──>  nemux __daemon
        │                                                     │
        │ TUI: ratatui + crossterm                            │ one PTY per tab
        │ mirrors screen state with its own                   │ (portable-pty)
        │ vt100::Parser, fed by diff bytes                     │
        │ from the daemon                                     │ vt100::Parser
        └─ CLI one-shot commands (ls/kill/rename/...)          │ per tab mirrors
           talk to the same socket, no TUI needed              │ the PTY output
                                                                 ▼
                                                        child process (shell,
                                                        codex, vim, ...)
```

- **Daemon** (`src/daemon/`): owns every tab (`TabManager` → `Tab`). Each tab
  runs its child process in a PTY and has a dedicated thread that reads PTY
  output, feeds it through a `vt100::Parser` to keep authoritative screen
  state, and pushes the resulting diff (or, for the first subscriber, a full
  redraw) to every client currently attached to it. When that thread hits
  EOF, it records the process's exit status on the tab instead of removing
  it (`TabManager::kill` only ever removes a tab that's already marked
  exited); removing the last tab this way makes the daemon exit the process.
- **Wire protocol** (`src/protocol.rs`): a small `Request`/`Response` enum
  pair, length-prefixed bincode over the socket. Requests always flow
  client→daemon, responses always flow daemon→client, so one connection's two
  halves never need to distinguish message shapes.
- **Client** (`src/client/`): `nemux` with no special flags spawns the daemon
  if needed (`__daemon`, detached — `setsid` on Unix, `DETACHED_PROCESS` on
  Windows), then either runs the interactive TUI (`client::tui`) or, for
  plain commands like `ls`/`kill`/`rename`, sends one request and prints the
  response without touching the terminal.
- The TUI keeps its **own** `vt100::Parser` per attached tab, replaying the
  diff bytes the daemon sends; a custom `ratatui::widgets::Widget` paints that
  parser's screen cell-by-cell (colors, bold/italic/underline/reverse) into
  the frame buffer above a tab-bar row.

## Development

```sh
cargo build
cargo test           # unit tests + integration tests that spawn a real,
                      # isolated daemon process over a throwaway socket namespace
cargo clippy --all-targets
```

`RUST_LOG=debug nemux` (or set it before the daemon auto-starts) turns on
verbose daemon logging; daemon stdout/stderr always go to
`nemux status`'s reported log path regardless of `RUST_LOG`.

## Platform support

Linux and Windows are both first-class targets (PTYs via `portable-pty`,
local sockets via `interprocess`'s named sockets, which map to the Linux
abstract namespace or Windows named pipes respectively). macOS should work
through the same Unix code paths (PTYs, `setsid`, filesystem-path local
sockets) but is untested.

## License

MIT — see [LICENSE](LICENSE).
