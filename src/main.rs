use nux::client;
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
    nux status                 Show whether the daemon is running
    nux kill-server            Kill every tab and stop the daemon
    nux restart-server         Restart the daemon (tabs are lost)
    nux config                 Print the config file path
    nux -h | --help            Show this help
    nux -V | --version         Show the version

SELECTORS
    A selector is a tab id ("0"), or a case-insensitive substring matched
    against the tab's title or program name ("codex"). If a selector matches
    more than one tab, nux lets you pick from a list.

CONFIG
    Keybindings and defaults live in nux/config.toml under your platform's
    config directory (see `nux config`). Defaults:
      Alt+C          new tab (default shell)
      Alt+Left/Right previous / next tab
      Alt+X          close current tab
      Alt+R          rename current tab
      Alt+/          tab picker
      Alt+D          detach (daemon keeps running)
"#;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = dispatch(args);
    if let Err(e) = result {
        eprintln!("nux: {e}");
        std::process::exit(1);
    }
}

fn dispatch(mut args: Vec<String>) -> anyhow::Result<()> {
    if args.is_empty() {
        return client::tui::run(Config::load(), client::tui::Start::Overview);
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
        "__daemon" => nux::daemon::run(Config::load()),
        "-t" | "attach" => {
            let selector = args.get(1).cloned().ok_or_else(|| anyhow::anyhow!("missing selector"))?;
            attach_by_selector(&selector)
        }
        "-k" | "kill" => {
            let selector = args.get(1).cloned().ok_or_else(|| anyhow::anyhow!("missing selector"))?;
            kill_by_selector(&selector)
        }
        "rename" => {
            if args.len() < 3 {
                anyhow::bail!("usage: nux rename <SELECTOR> <TITLE>");
            }
            rename_by_selector(&args[1], args[2..].join(" "))
        }
        "ls" | "list" => list_tabs(),
        "status" => status(),
        "kill-server" => kill_server(),
        "restart-server" | "restart" => restart_server(),
        "config" => {
            println!("{}", Config::config_path().display());
            Ok(())
        }
        "new" | "run" => {
            let command = args.split_off(1);
            client::tui::run(Config::load(), client::tui::Start::Create(command))
        }
        _ => client::tui::run(Config::load(), client::tui::Start::Create(args)),
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
fn resolve_one(stream: &mut interprocess::local_socket::Stream, selector: &str) -> anyhow::Result<TabInfo> {
    let tabs = fetch_tabs(stream)?;
    let matches = find_matches(&tabs, selector);
    match matches.len() {
        0 => anyhow::bail!("no tab matches {selector:?}"),
        1 => Ok(matches[0].clone()),
        _ => {
            println!("multiple tabs match {selector:?}:");
            for (i, t) in matches.iter().enumerate() {
                let title = if t.title.is_empty() { t.program() } else { &t.title };
                println!("  [{i}] {}: {title}", t.id);
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

fn attach_by_selector(selector: &str) -> anyhow::Result<()> {
    let id = with_connection(|s| resolve_one(s, selector).map(|t| t.id))?;
    client::tui::run(Config::load(), client::tui::Start::Attach(id))
}

fn kill_by_selector(selector: &str) -> anyhow::Result<()> {
    with_connection(|s| {
        let tab = resolve_one(s, selector)?;
        match client::request_once(s, &Request::KillTab { tab_id: tab.id })? {
            Response::Ok => {
                println!("sent kill signal to tab {} ({})", tab.id, tab.program());
                Ok(())
            }
            // The tab was already exited, so this call dismissed/removed it
            // outright instead of signaling a (nonexistent) process.
            Response::TabClosed(_) => {
                println!("dismissed exited tab {} ({})", tab.id, tab.program());
                Ok(())
            }
            Response::Error(e) => anyhow::bail!(e),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    })
}

fn rename_by_selector(selector: &str, title: String) -> anyhow::Result<()> {
    with_connection(|s| {
        let tab = resolve_one(s, selector)?;
        match client::request_once(s, &Request::RenameTab { tab_id: tab.id, title })? {
            Response::TabUpdated(info) => {
                println!("tab {} renamed to {:?}", info.id, info.title);
                Ok(())
            }
            Response::Error(e) => anyhow::bail!(e),
            other => anyhow::bail!("unexpected response: {other:?}"),
        }
    })
}

fn list_tabs() -> anyhow::Result<()> {
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
            None => "running".to_string(),
            Some(e) if e.success => "exited".to_string(),
            Some(e) => format!("exited(code {})", e.code),
        };
        println!("{:>3}  {:<20} pid={:<8} {}x{}  {status}", t.id, title, pid, t.cols, t.rows);
    }
    Ok(())
}

fn status() -> anyhow::Result<()> {
    if !client::is_running() {
        println!("nux daemon: not running");
        return Ok(());
    }
    let tabs = with_connection(fetch_tabs)?;
    println!("nux daemon: running ({} tab{})", tabs.len(), if tabs.len() == 1 { "" } else { "s" });
    println!("socket: {:?}", nux::ipc::socket_name().ok());
    println!("log: {}", nux::ipc::log_file().display());
    Ok(())
}

fn kill_server() -> anyhow::Result<()> {
    if client::kill_server()? {
        println!("nux daemon stopped");
    } else {
        println!("nux daemon was not running");
    }
    Ok(())
}

fn restart_server() -> anyhow::Result<()> {
    client::kill_server()?;
    // Give the old process a moment to release the socket before spawning a new one.
    std::thread::sleep(std::time::Duration::from_millis(300));
    client::ensure_daemon()?;
    println!("nux daemon restarted");
    Ok(())
}
