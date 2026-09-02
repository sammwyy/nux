# nux

A modern, daemon-backed terminal multiplexer. Sessions live in a background
daemon that starts automatically and keeps running independently of any
terminal you attach from — think tmux/screen, rebuilt around a client/server
split with a first-class TUI tab bar instead of prefix-key chords.

```
nux                # open the tab overview, switch with a keystroke
nux codex           # run `codex` in a new tab and attach to it
nux -t codex        # jump to a tab whose title/program matches "codex"
nux -k 2             # kill tab 2
```

## Features

- **Daemon-backed sessions.** The first `nux` invocation spawns a detached
  background daemon (survives the launching shell exiting); every later
  invocation talks to it over a local socket. Tabs and their child processes
  outlive your terminal.
- **Tabs, not windows.** Open, switch, rename and close tabs from a bottom
  status bar, or jump straight to one from the command line by id, title or
  program name.
- **Real terminal emulation per tab**, powered by `vt100`: colors, cursor
  shape, alternate screen apps (vim, htop, ...), and OSC title updates all
  work.
- **Optionally, exited tabs stay put.** With `keep_exited_tab_open = true`, a
  process ending doesn't close its tab: nux marks it `[exited]`, keeps its
  final screen (up to `scrollback_lines`) so you can see what happened, and
  leaves it in the list until you dismiss it — same close-tab key/command as
  always, applied twice (once to kill, once to dismiss).
- **The daemon shuts itself down** once the last tab is dismissed — like a
  tmux server exiting when its last session is killed — so nothing lingers
  once you're done.
- **Configurable keybindings** (`config.toml`), sensible defaults, no
  mandatory prefix key.
- **Cross-platform.** PTYs via `portable-pty` (ConPTY on Windows, real PTYs on
  Unix), local sockets via `interprocess` (named pipes on Windows, Unix domain
  sockets elsewhere).

## Install / build

Linux (x86_64, arm64, or armv7 — e.g. Raspberry Pi):

```sh
curl -fsSL https://raw.githubusercontent.com/sammwyy/nux/main/install.sh | sh
```

Windows:

```powershell
irm https://raw.githubusercontent.com/sammwyy/nux/main/install.ps1 | iex
```

Both scripts fetch the latest [release](https://github.com/sammwyy/nux/releases)
binary for your OS/arch and put it on your `PATH` — `~/.local/bin` on Linux
(added to your shell rc if it isn't already on `PATH`), `%APPDATA%\nux\bin`
on Windows (added to your user `PATH` environment variable). Both are
per-user installs; no admin/sudo required.

Or via cargo:

```sh
cargo install nux-term
```

Or build from a checkout:

```sh
cargo build --release
# binary at target/release/nux[.exe]
```

Requires a stable Rust toolchain (2021 edition). No system dependencies beyond
a C toolchain for a couple of transitive crates.

## Usage

```
nux                          Open the tab overview / attach to the last tab
nux <PROGRAM> [ARGS...]      Open PROGRAM in a new tab and attach to it
nux new <PROGRAM> [ARGS...]  Same, for programs that collide with a nux
                                subcommand name (e.g. `nux new ls`)
nux -t <SELECTOR>            Attach to a tab by id, title or program name
nux -k <SELECTOR>            Kill (or, if already exited, dismiss) a tab
nux attach <SELECTOR>        Same as -t
nux kill <SELECTOR>          Same as -k
nux rename <SELECTOR> <TITLE> Rename a tab
nux ls | list                 List open tabs (plain text, non-interactive)
nux daemon                   Show whether the daemon is running
nux daemon kill               Kill every tab and stop the daemon
nux daemon restart            Restart the daemon (open tabs are lost)
nux config                   Print the config path and its contents
nux config <KEY>             Print one config value
nux config <KEY> <VALUE>     Set and save one config value
nux -h | --help              Show help
nux -V | --version           Show the version
--colors | --no-colors       Force-enable/disable colored output for this run
```

Plain-command output (`ls`, `daemon`, `config`) is colorized when stdout is a
terminal, controlled by the `color` config key (`"auto"` by default, or
`"always"`/`"never"`) and overridable per invocation with `--colors`/
`--no-colors` regardless of where they appear on the command line.

### Selectors

A *selector* (used by `-t`/`attach`, `-k`/`kill`, `rename`) is tried in order
as:

1. An exact numeric tab id (`nux -t 0`).
2. A case-insensitive substring match against the tab's title **or** its
   program name (`nux -t codex` matches a tab titled "codex session" or
   running `/usr/bin/codex`).

If more than one tab matches, nux prints a numbered list and asks you to
pick — from the shell for `-t`/`-k`/`rename`, or as an in-TUI popup for the
picker keybind (see below).

### Opening a specific program

`nux <program> [args...]` is the fast path: it creates a new tab running
that exact command line and attaches to it immediately. Since a handful of
words are reserved as subcommands (`ls`, `kill`, `attach`, `rename`,
`daemon`, `config`, `new`, `run`), use `nux new ls` (or `nux run ls`) if you
actually want to open a program named e.g. `ls` in a tab.

### Tab lifecycle

By default a tab closes the instant its process exits. Set
`keep_exited_tab_open = true` in `config.toml` to leave it around instead:
marked `[exited]` in the tab bar (with the exit code, if nonzero, shown while
it's the attached tab), keeping whatever was on screen — up to
`scrollback_lines` of it — so you can read what happened before deciding what
to do next.

With `keep_exited_tab_open` on, killing a tab (`Alt+X`, `nux -k <selector>`,
`nux kill <selector>`) becomes a two-step action on the same key/command:

1. On a **running** tab: sends a termination signal to its process. The tab
   stays listed, now waiting to be marked exited.
2. On an **already-exited** tab: dismisses it — this is the step that
   actually removes it from the list.

Either way, once the last tab is gone the daemon shuts itself down
automatically (see [Architecture](#architecture)) — there's no need to run
`nux daemon kill` by hand after closing everything from inside the TUI.

## Keybindings

Configured in `config.toml` (edit by hand, or with `nux config <key> <value>`
— dotted paths like `keybindings.new_tab` or `layout.tab_bar_row`, type- and
value-checked before it's saved). `nux config` on its own pretty-prints the
whole file; `nux config <key>` prints just that value. Defaults on first run:

| Action           | Default       |
|------------------|---------------|
| New tab (shell)  | `Alt+N`       |
| Next tab         | `Alt+Right`   |
| Previous tab     | `Alt+Left`    |
| Close current tab| `Alt+X`       |
| Rename current tab | `Alt+R`     |
| Tab picker (fuzzy switch) | `Alt+/` |
| Detach (daemon keeps running) | `Alt+D` |

Any key not bound to an action is forwarded to the attached program as
terminal input, including `Ctrl+C` — so it reaches your shell/program as
usual, not nux.

Keys are written as `Modifier+Modifier+Key`, e.g. `"Ctrl+Shift+n"`,
`"Alt+Right"`, `"F2"`. Supported modifiers: `Ctrl`, `Alt`, `Shift`.

```toml
scrollback_lines = 5000
keep_exited_tab_open = false

[keybindings]
new_tab = "Alt+n"
next_tab = "Alt+Right"
prev_tab = "Alt+Left"
close_tab = "Alt+x"
rename_tab = "Alt+r"
detach = "Alt+d"
picker = "Alt+/"

[layout]
tab_bar_row = "bottom"
tab_bar_side = "left"
workspace_bar_row = "bottom"
workspace_bar_side = "right"
workspace_bar_width = 32
```

A top-level `shell = "..."` key (absent by default, so not shown in the
generated file above) overrides the program used for new tabs opened without
an explicit command; without it, nux uses `$SHELL` on Unix and `%COMSPEC%`
on Windows.

## Status bars

Two independent bars: the tab bar (`Nux ›` plus the scrollable tab strip)
and the workspace bar (the attached tab's directory, right-trimmed with `…`
when it doesn't fit).

Each one is placed with `<bar>_row` (`top`/`bottom`) and `<bar>_side`
(`left`/`right`). Sharing a row splits it between both sides; alone on a row,
a bar gets the full width. A row neither bar uses isn't reserved — set both
to `bottom` (the default) and the top row disappears entirely, or split them
across `top`/`bottom` for two separate lines. `workspace_bar_width` caps how
many columns the workspace bar takes when it shares a row with the tab bar.

The tab strip shows as many tabs as fit, keeping the selected one centered
until it nears either end of the list, with `‹`/`›` when tabs are hidden off
that side.

## Architecture

```
   nux (client, any invocation)  <── local socket ──>  nux __daemon
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
- **Client** (`src/client/`): `nux` with no special flags spawns the daemon
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

`RUST_LOG=debug nux` (or set it before the daemon auto-starts) turns on
verbose daemon logging; daemon stdout/stderr always go to
`nux daemon`'s reported log path regardless of `RUST_LOG`.

`NUX_CONFIG_PATH` overrides the config file location (used by the test suite
for isolation, since `dirs::config_dir()` doesn't honor `XDG_CONFIG_HOME` on
macOS/Windows) — also handy for running more than one isolated setup by hand.

## Platform support

Linux and Windows are both first-class targets (PTYs via `portable-pty`,
local sockets via `interprocess`'s named sockets, which map to the Linux
abstract namespace or Windows named pipes respectively). macOS should work
through the same Unix code paths (PTYs, `setsid`, filesystem-path local
sockets) but is untested.

Linux releases are published for x86_64, arm64 and armv7 (32-bit) — the
latter two cover Raspberry Pi and other single-board computers.

## License

MIT — see [LICENSE](LICENSE).
