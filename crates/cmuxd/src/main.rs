//! `cmuxd`: the cmux server. `serve` runs it; the other subcommands are the
//! helpers that talk to a running one.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use cmux_proto::api::Scope;
use cmuxd::ServerBuilder;
use cmuxd::config::{Config, Overrides};

#[derive(Parser)]
#[command(name = "cmuxd", version, about = "The cmux server")]
struct Cli {
    /// Config file (default ~/.config/cmuxd/config.toml).
    #[arg(long, global = true, env = "CMUXD_CONFIG")]
    config: Option<PathBuf>,
    /// Data directory (default ~/.local/share/cmuxd).
    #[arg(long, global = true, env = "CMUXD_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Unix socket path.
    #[arg(long, global = true, env = "CMUXD_SOCKET")]
    socket: Option<PathBuf>,
    /// TCP listen address, or "off".
    #[arg(long, global = true, env = "CMUXD_LISTEN")]
    listen: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the server.
    Serve,
    /// Print a pairing code and QR for a new device (needs a running server).
    Pair {
        /// What the new device may do.
        #[arg(long, value_parser = parse_scope)]
        scope: Option<Scope>,
    },
    /// Claude Code hook entry point (not yet implemented).
    Hook { event: Option<String> },
    /// Stdio MCP server for an agent (not yet implemented).
    Mcp,
    /// Check the installation (not yet implemented).
    Doctor,
}

fn parse_scope(s: &str) -> Result<Scope, String> {
    match s {
        "full" => Ok(Scope::Full),
        "read" => Ok(Scope::Read),
        _ => Err("expected `full` or `read`".into()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("cmuxd: {msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode, String> {
    let overrides = Overrides {
        config: cli.config,
        data_dir: cli.data_dir,
        socket: cli.socket,
        listen: cli.listen,
    };
    match cli.cmd {
        // Hooks must never fail Claude's turn.
        Cmd::Hook { .. } => {
            eprintln!("cmuxd hook: not yet implemented");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Mcp => Err("mcp: not yet implemented".into()),
        Cmd::Doctor => {
            println!("cmuxd doctor: not yet implemented");
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Pair { scope } => {
            let config = Config::load(&overrides).map_err(|e| e.to_string())?;
            let rt = runtime()?;
            rt.block_on(cmuxd::cli::pair(&config.socket_path(), scope))?;
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Serve => {
            let config = Config::load(&overrides).map_err(|e| e.to_string())?;
            runtime()?.block_on(serve(config))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

async fn serve(config: Config) -> Result<(), String> {
    let server = ServerBuilder::new(config)
        .global_tracing(true)
        .start()
        .await
        .map_err(|e| e.to_string())?;
    eprintln!("cmuxd {} listening", env!("CARGO_PKG_VERSION"));
    eprintln!("  socket: {}", server.socket_path.display());
    if let Some(a) = server.tcp_addr {
        eprintln!("  tcp:    {a}");
    }
    shutdown_signal().await;
    eprintln!("cmuxd: shutting down");
    server.shutdown().await;
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
