mod app;
mod conn;
mod history;
mod keys;
mod layout;
mod pane;
mod tui;
mod ui;
mod window;

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::{Result, bail};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::{cursor, execute, terminal};

use crate::app::App;

pub enum Event {
    Input(Vec<u8>),
    Output(u64),
    Exited(u64),
    Indexed(Vec<history::SessionInfo>),
    /// Remote mode: an event from the connection thread.
    Stream(conn::stream::StreamEvent),
    /// Remote mode: the answer to a REST call made off the UI thread.
    Rest {
        then: tui::remote::Then,
        result: Result<serde_json::Value, String>,
    },
    Resize,
    Tick,
    Quit,
}

pub struct Options {
    pub prefix: u8,
    pub claude: String,
    pub claude_args: Vec<String>,
    pub dirs: Vec<PathBuf>,
    pub resume_picker: bool,
    pub continue_last: bool,
    pub bell: bool,
    pub mouse: bool,
    /// `-s NAME`: attach to this server.
    pub server: Option<String>,
    /// `--local`: never attach to a server.
    pub local: bool,
}

const USAGE: &str = "\
cmux - a terminal multiplexer for Claude Code sessions

USAGE:
    cmux [OPTIONS] [DIR...] [-- CLAUDE_ARGS...]
    cmux login URL | logout [NAME] | servers    Pair with and manage cmuxd servers
    cmux [-s NAME] VERB ARGS...                 plans, board, new-plan, accept, pause,
                                                inbox, allow, deny, reply, send, stop,
                                                resume, tail, attach (`cmux plans -h`)

With a server (-s, a default in client.toml, or a cmuxd socket on this machine)
cmux attaches to it: plans, agents and terminals live on the server and keep
running when you detach (prefix d). Without one it runs claude locally.

ARGS:
    DIR...            Open a new Claude session in each directory (default: current dir)
    CLAUDE_ARGS...    Extra arguments for every claude process (e.g. -- --model opus)

OPTIONS:
    -r, --resume      Start with the session picker
    -c, --continue    Continue the most recent session in the current dir
    -s, --server NAME Attach to this cmuxd server (see `cmux servers`; `local` is this machine)
        --local       Run claude locally even if a server is configured
    -p, --prefix KEY  Prefix key, a letter for Ctrl-<letter> (default: b)
        --no-bell     Do not ring the terminal bell when a background session finishes
        --no-mouse    Leave the mouse to the terminal (native selection, no wheel scroll)
    -h, --help        Show this help
    -V, --version     Show the version

ENVIRONMENT:
    CMUX_CLAUDE       Path to the claude binary (default: claude)
    CMUX_PREFIX       Default prefix letter

Press the prefix then ? inside cmux to list all key bindings.
";

fn parse_prefix(s: &str) -> Result<u8> {
    let s = s.trim().to_ascii_lowercase();
    let s = s
        .strip_prefix("c-")
        .or_else(|| s.strip_prefix("ctrl-"))
        .unwrap_or(&s);
    match s.as_bytes() {
        [c @ b'a'..=b'z'] => Ok(c - b'a' + 1),
        _ => bail!("invalid prefix {s:?}: use a letter a-z"),
    }
}

fn parse_args(argv: impl IntoIterator<Item = String>) -> Result<Options> {
    let mut o = Options {
        prefix: parse_prefix(&std::env::var("CMUX_PREFIX").unwrap_or_else(|_| "b".into()))?,
        claude: std::env::var("CMUX_CLAUDE").unwrap_or_else(|_| "claude".into()),
        claude_args: Vec::new(),
        dirs: Vec::new(),
        resume_picker: false,
        continue_last: false,
        bell: true,
        mouse: true,
        server: None,
        local: false,
    };
    let mut args = argv.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("cmux {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-r" | "--resume" => o.resume_picker = true,
            "-c" | "--continue" => o.continue_last = true,
            "--no-bell" => o.bell = false,
            "--no-mouse" => o.mouse = false,
            "--local" => o.local = true,
            "-s" | "--server" => {
                let Some(v) = args.next() else {
                    bail!("--server needs a server name")
                };
                o.server = Some(v);
            }
            "-p" | "--prefix" => {
                let Some(v) = args.next() else {
                    bail!("--prefix needs a value")
                };
                o.prefix = parse_prefix(&v)?;
            }
            "--" => {
                o.claude_args.extend(args.by_ref());
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n\n{USAGE}"),
            dir => {
                let p = history::expand_tilde(dir);
                let p = std::fs::canonicalize(&p)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                o.dirs.push(p);
            }
        }
    }
    Ok(o)
}

fn restore_terminal() {
    let mut out = std::io::stdout();
    // Reset input modes a child may have turned on through us.
    let _ = out.write_all(b"\x1b[?2004l\x1b[?1l\x1b>\x1b[?1003l\x1b[?1006l\x1b[?1002l\x1b[?1000l");
    let _ = execute!(out, terminal::LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
}

/// Takes a leading `-s NAME` off `argv`, so `cmux -s home plans` reaches
/// the verb.
fn split_server(mut argv: Vec<String>) -> (Option<String>, Vec<String>) {
    match argv.first().map(String::as_str) {
        Some("-s" | "--server")
            if argv.len() >= 3 && conn::verbs::VERBS.contains(&argv[2].as_str()) =>
        {
            let rest = argv.split_off(2);
            (argv.pop(), rest)
        }
        _ => (None, argv),
    }
}

fn main() -> Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("login") => return conn::cli::login(&argv[1..]),
        Some("servers") => return conn::cli::servers(&argv[1..], false),
        Some("logout") => return conn::cli::servers(&argv[1..], true),
        _ => {}
    }
    let (server_name, argv) = split_server(argv);
    match argv.first().map(String::as_str) {
        Some("attach") => return attach(server_name, &argv[1..]),
        Some(v) if conn::verbs::VERBS.contains(&v) => {
            std::process::exit(conn::verbs::main(server_name.as_deref(), v, &argv[1..]))
        }
        _ => {}
    }
    let opts = parse_args(argv)?;
    let server = conn::servers::pick(
        &conn::servers::ClientConfig::load(&conn::servers::client_toml())?,
        opts.server.as_deref(),
        opts.local,
        &conn::servers::local_socket(),
        conn::servers::socket_alive,
    )?;
    start_tui(opts, server, None)
}

/// `cmux attach [<agent>] [OPTIONS]`: the TUI on a server, opened on that
/// agent. Exit codes as for the other verbs.
fn attach(server_name: Option<String>, args: &[String]) -> Result<()> {
    let (agent, rest) = match args.first() {
        Some(a) if !a.starts_with('-') => (Some(a.as_str()), &args[1..]),
        _ => (None, args),
    };
    let fail = |e: conn::verbs::VerbError| -> ! {
        eprintln!("{}", e.line());
        std::process::exit(e.code())
    };
    let mut opts = parse_args(rest.to_vec())
        .unwrap_or_else(|e| fail(conn::verbs::VerbError::Usage(e.to_string())));
    if server_name.is_some() {
        opts.server = server_name;
    }
    let server = conn::servers::ClientConfig::load(&conn::servers::client_toml())
        .and_then(|c| c.resolve(opts.server.as_deref(), &conn::servers::local_socket()))
        .unwrap_or_else(|e| fail(conn::verbs::VerbError::Server(format!("{e:#}"))));
    let http = conn::http::Http::new(server.endpoint.clone());
    let target = conn::verbs::attach_target(&http, agent).unwrap_or_else(|e| fail(e));
    start_tui(opts, Some(server), target)
}

fn start_tui(
    opts: Options,
    server: Option<conn::servers::Server>,
    target: Option<tui::state::Node>,
) -> Result<()> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        bail!("cmux needs an interactive terminal");
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    terminal::enable_raw_mode()?;
    execute!(std::io::stdout(), terminal::EnterAlternateScreen)?;
    if opts.mouse {
        // Button presses, drags and the wheel, in SGR encoding.
        std::io::stdout().write_all(b"\x1b[?1000h\x1b[?1002h\x1b[?1006h")?;
    }
    let mut term = Terminal::new(CrosstermBackend::new(std::io::stdout()))?;

    let result = run(&mut term, opts, server, target);
    restore_terminal();
    result
}

fn run(
    term: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    opts: Options,
    server: Option<conn::servers::Server>,
    target: Option<tui::state::Node>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel::<Event>();

    // Raw stdin reader. Pane input is forwarded byte for byte.
    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(Event::Quit);
                        break;
                    }
                    Ok(n) => {
                        if tx.send(Event::Input(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    {
        use signal_hook::consts::{SIGHUP, SIGTERM, SIGWINCH};
        let tx = tx.clone();
        let mut signals = signal_hook::iterator::Signals::new([SIGWINCH, SIGTERM, SIGHUP])?;
        std::thread::spawn(move || {
            for sig in signals.forever() {
                let ev = if sig == SIGWINCH {
                    Event::Resize
                } else {
                    Event::Quit
                };
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
    }

    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while tx.send(Event::Tick).is_ok() {
                std::thread::sleep(Duration::from_millis(250));
            }
        });
    }

    let (cols, rows) = terminal::size()?;
    let mut app = match server {
        None => App::new(opts, tx, rows, cols),
        Some(s) => {
            let rest = std::sync::Arc::new(tui::remote::HttpRest(conn::http::Http::new(
                s.endpoint.clone(),
            )));
            let mut remote =
                app::remote::RemoteState::connect(s.name, s.endpoint, rest, tx.clone());
            remote.open_on_start = target;
            App::new_remote(opts, tx, rows, cols, remote)
        }
    };
    app.start();

    loop {
        let ev = rx.recv()?;
        app.handle(ev);
        // Drain everything already queued so a burst of output costs one frame.
        while let Ok(ev) = rx.try_recv() {
            app.handle(ev);
        }
        if app.quit {
            break;
        }
        if app.take_redraw() {
            term.draw(|f| ui::draw(f, &app))?;
            let out = term.backend_mut();
            out.write_all(&app.take_passthrough())?;
            out.flush()?;
        }
    }
    app.shutdown();
    Ok(())
}
