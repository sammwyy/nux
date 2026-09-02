use nux::client;
use nux::color::{should_colorize, Painter};
use nux::config::Config;
use nux::protocol::{Request, Response, TabInfo};
use nux::selector::find_matches;

const HELP: &str = r#"nux — a modern, daemon-backed terminal multiplexer

USAGE:
    nux                        Open the tab overview / attach to the last tab
    nux <PROGRAM> [ARGS...]    Open PROGRAM in a new tab and attach to it
    nux new <PROGRAM> [ARGS...] Same as above, for programs that collide with a
                                  nux subcommand name (e.g. `nux new ls`)
    nux -t <SELECTOR>          Attach to a tab by id, title or program name
    nux -k <SELECTOR>          Kill a tab by id, title or program name
    nux attach <SELECTOR>      Same as -t
    nux kill <SELECTOR>        Same as -k
    nux rename <SELECTOR> <TITLE>
                                  Rename a tab
    nux ls | list               List open tabs
    nux daemon                 Show whether the daemon is running
    nux daemon kill            Kill every tab and stop the daemon
    nux daemon restart         Restart the daemon (tabs are lost)
    nux config                 Print the config file path and its contents
    nux config <KEY>           Print one config value
    nux config <KEY> <VALUE>   Set and save one config value
    nux -h | --help            Show this help
    nux -V | --version         Show the version
    --colors | --no-colors     Force-enable/disable colored output for this run
                                  (overrides the `color` config setting)

SELECTORS
    A selector is a tab id ("0"), or a case-insensitive substring matched
    against the tab's title or program name ("codex"). If a selector matches
    more than one tab, nux lets you pick from a list.

CONFIG
    Keybindings and defaults live in nux/config.toml under your platform's
    config directory (see `nux config`). Nested keys use dots, e.g.
    `nux config keybindings.new_tab "Alt+n"` or `nux config layout.tab_bar_row top`.
    Defaults:
      Alt+N          new tab (default shell)
      Alt+Left/Right previous / next tab
      Alt+X          close current tab
      Alt+R          rename current tab
      Alt+/          tab picker
      Alt+D          detach (daemon keeps running)
"#;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let forced_color = extract_color_flag(&mut args);
    let cfg = Config::load();
    let painter = Painter::new(should_colorize(cfg.color, forced_color));

    let result = dispatch(args, cfg, &painter);
    if let Err(e) = result {
        eprintln!("{} {e}", painter.red("nux:"));
        std::process::exit(1);
    }
}

/// Pulls `--colors`/`--no-colors` (and the `--color`/`--no-color` singular
/// spellings) out of `args`, wherever they appear, returning the forced
/// value if either was present.
fn extract_color_flag(args: &mut Vec<String>) -> Option<bool> {
    let mut forced = None;
    args.retain(|a| match a.as_str() {
        "--colors" | "--color" => {
            forced = Some(true);
            false
        }
        "--no-colors" | "--nocolors" | "--no-color" => {
            forced = Some(false);
            false
        }
        _ => true,
    });
    forced
}

fn dispatch(mut args: Vec<String>, cfg: Config, painter: &Painter) -> anyhow::Result<()> {
    if args.is_empty() {
        return client::tui::run(cfg, client::tui::Start::Overview);
    }

    match args[0].as_str() {
        "-h" | "--help" => {
            print!("{HELP}");
            Ok(())
        }
        "-V" | "--version" => {
            println!("nux {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "__daemon" => nux::daemon::run(cfg),
        "-t" | "attach" => {
            let selector = args.get(1).cloned().ok_or_else(|| anyhow::anyhow!("missing selector"))?;
            attach_by_selector(cfg, &selector)
        }
        "-k" | "kill" => {
            let selector = args.get(1).cloned().ok_or_else(|| anyhow::anyhow!("missing selector"))?;
            kill_by_selector(&selector, painter)
        }
        "rename" => {
            if args.len() < 3 {
                anyhow::bail!("usage: nux rename <SELECTOR> <TITLE>");
            }
            rename_by_selector(&args[1], args[2..].join(" "), painter)
        }
        "ls" | "list" => list_tabs(painter),
        "daemon" => match args.get(1).map(String::as_str) {
            None => daemon_status(painter),
            Some("kill") => daemon_kill(painter),
            Some("restart") => daemon_restart(painter),
            Some(other) => anyhow::bail!("unknown `nux daemon` subcommand {other:?} (expected kill|restart)"),
        },
        "config" => config_cmd(&args[1..], cfg, painter),
        "new" | "run" => {
            let command = args.split_off(1);
            client::tui::run(cfg, client::tui::Start::Create(command))
        }
        _ => client::tui::run(cfg, client::tui::Start::Create(args)),
    }
}

fn with_connection<T>(f: impl FnOnce(&mut interprocess::local_socket::Stream) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let mut stream = client::connect()
        .map_err(|_| anyhow::anyhow!("daemon is not running (start it with `nux`)"))?;
    f(&mut stream)
}

fn fetch_tabs(stream: &mut interprocess::local_socket::Stream) -> anyhow::Result<Vec<TabInfo>> {
    match client::request_once(stream, &Request::ListTabs)? {
        Response::TabList(tabs) => Ok(tabs),
        Response::Error(e) => anyhow::bail!(e),
        other => anyhow::bail!("unexpected response: {other:?}"),
    }
}

/// Resolves `selector` against the live tab list, printing a picker prompt on the
/// terminal (not the TUI) if it's ambiguous.
fn resolve_one(stream: &mut interprocess::local_socket::Stream, selector: &str, painter: &Painter) -> anyhow::Result<TabInfo> {
    let tabs = fetch_tabs(stream)?;
    let matches = find_matches(&tabs, selector);
    match matches.len() {
        0 => anyhow::bail!("no tab matches {selector:?}"),
        1 => Ok(matches[0].clone()),
        _ => {
            println!("multiple tabs match {selector:?}:");
            for (i, t) in matches.iter().enumerate() {
                let title = if t.title.is_empty() { t.program() } else { &t.title };
                println!("  [{}] {}: {title}", painter.cyan(&i.to_string()), t.id);
            }
            print!("pick one [0-{}]: ", matches.len() - 1);
            use std::io::Write;
            std::io::stdout().flush().ok();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            let idx: usize = line.trim().parse().map_err(|_| anyhow::anyhow!("invalid selection"))?;
            matches
                .get(idx)
                .map(|t| (*t).clone())
                .ok_or_else(|| anyhow::anyhow!("selection out of range"))
        }
    }
}

fn attach_by_selector(cfg: Config, selector: &str) -> anyhow::Result<()> {
    let painter = Painter::new(should_colorize(cfg.color, None));
    let id = with_connection(|s| resolve_one(s, selector, &painter).map(|t| t.id))?;
    client::tui::run(cfg, client::tui::Start::Attach(id))
}

fn kill_by_selector(selector: &str, painter: &Painter) -> anyhow::Result<()> {
    with_connection(|s| {
        let tab = resolve_one(s, selector, painter)?;
        match client::request_once(s, &Request::KillTab { tab_id: tab.id })? {
            Response::Ok => {
                println!("{} kill signal to tab {} ({})", painter.yellow("sent"), tab.id, tab.program());
                Ok(())
            }
            // The tab was already exited, so this call dismissed/removed it
            // outright instead of signaling a (nonexistent) process.
            Response::TabClosed(_) => {
                println!("{} exited tab {} ({})", painter.green("dismissed"), tab.id, tab.program());
                Ok(())
            }
            Response::Error(e) => anyhow::bail!(e),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    })
}

fn rename_by_selector(selector: &str, title: String, painter: &Painter) -> anyhow::Result<()> {
    with_connection(|s| {
        let tab = resolve_one(s, selector, painter)?;
        match client::request_once(s, &Request::RenameTab { tab_id: tab.id, title })? {
            Response::TabUpdated(info) => {
                println!("tab {} {} to {:?}", info.id, painter.green("renamed"), info.title);
                Ok(())
            }
            Response::Error(e) => anyhow::bail!(e),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    })
}

fn list_tabs(painter: &Painter) -> anyhow::Result<()> {
    if !client::is_running() {
        println!("daemon is not running (no tabs)");
        return Ok(());
    }
    let tabs = with_connection(fetch_tabs)?;
    if tabs.is_empty() {
        println!("no open tabs");
        return Ok(());
    }
    for t in &tabs {
        let title = if t.title.is_empty() { t.program() } else { &t.title };
        let pid = t.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
        let status = match t.exit {
            None => painter.green("running"),
            Some(e) if e.success => painter.yellow("exited"),
            Some(e) => painter.red(&format!("exited(code {})", e.code)),
        };
        println!("{:>3}  {:<20} pid={:<8} {}x{}  {status}", painter.bold(&t.id.to_string()), title, pid, t.cols, t.rows);
    }
    Ok(())
}

fn daemon_status(painter: &Painter) -> anyhow::Result<()> {
    if !client::is_running() {
        println!("nux daemon: {}", painter.red("not running"));
        return Ok(());
    }
    let tabs = with_connection(fetch_tabs)?;
    println!(
        "nux daemon: {} ({} tab{})",
        painter.green("running"),
        tabs.len(),
        if tabs.len() == 1 { "" } else { "s" }
    );
    println!("socket: {:?}", nux::ipc::socket_name().ok());
    println!("log: {}", nux::ipc::log_file().display());
    Ok(())
}

fn daemon_kill(painter: &Painter) -> anyhow::Result<()> {
    if client::kill_server()? {
        println!("nux daemon {}", painter.yellow("stopped"));
    } else {
        println!("nux daemon was not running");
    }
    Ok(())
}

fn daemon_restart(painter: &Painter) -> anyhow::Result<()> {
    client::kill_server()?;
    client::ensure_daemon()?;
    println!("nux daemon {}", painter.green("restarted"));
    Ok(())
}

fn config_cmd(args: &[String], cfg: Config, painter: &Painter) -> anyhow::Result<()> {
    match args {
        [] => {
            println!("{}\n", painter.dim(&Config::config_path().display().to_string()));
            print!("{}", painter.toml(&cfg.pretty()));
            Ok(())
        }
        [key] => match nux::config::get_config_key(&cfg, key) {
            Some(value) => {
                println!("{value}");
                Ok(())
            }
            None => anyhow::bail!("unknown config key {key:?}"),
        },
        [key, value] => {
            let mut cfg = cfg;
            nux::config::set_config_key(&mut cfg, key, value).map_err(|e| anyhow::anyhow!(e))?;
            cfg.save()?;
            println!("{}", painter.green(&format!("{key} = {value}")));
            Ok(())
        }
        _ => anyhow::bail!("usage: nux config [<key> [<value>]]"),
    }
}
