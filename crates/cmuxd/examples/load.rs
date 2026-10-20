//! A load test for `cmuxd serve`. Not installed and not run by `cargo test`
//! (it is an example target; `cargo test` only builds it).
//!
//! ```sh
//! cargo run --release -p cmuxd --example load
//! cargo run --release -p cmuxd --example load -- --agents 20 --busy-secs 10
//! ```
//!
//! It builds `cmuxd` and the fake claude (`examples/fake-claude.rs`) in the
//! same profile as itself, then starts a real `cmuxd serve` with a temp data
//! dir, a temp socket and a free loopback port, and `[agents] claude_bin` set
//! to the fake claude. Each agent is the executor of its own plan; its script
//! prints one line every `--print-ms` until the measurements are done, then
//! ends its turn and idles. `--clients` WebSocket clients pair over TCP
//! (`/v1/pair/start` on the socket, `/v1/pair/complete` on TCP), attach to
//! `/v1/stream` and subscribe to every agent terminal. It measures:
//!
//! 1. the echo latency: the first client types one byte into a raw `cat`
//!    terminal and waits for it in an output frame, `--samples` times, while
//!    the agents print (p50, p95, max). The echo of a key after a quiet
//!    interval waits for no batch timer (`term::hub::BATCH`);
//! 2. the busy CPU of the cmuxd process (children not counted) over
//!    `--busy-secs`, with the agents printing;
//! 3. the idle CPU over `--idle-secs`, after every agent has gone idle (the
//!    clients stay attached);
//! 4. the cmuxd RSS at the end (children not counted).
//!
//! CPU % is of one core. Every option also reads an env var (`LOAD_AGENTS`,
//! ...); `--cmuxd` (`CMUXD_BIN`) skips the build and uses that binary, with the
//! fake claude next to it in `examples/`. Works on macOS and Linux.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use clap::Parser;
use cmux_proto::api::{
    CreateTerminalReq, NewTerminalKind, PairCompleteReq, PairCompleteResp, PairStartReq,
    PairStartResp, route,
};
use cmux_proto::stream::{ClientKind, ClientMsg, Frame};
use cmux_proto::view::{Activity, StateView, TermId, TerminalView};
use cmuxd::client::Endpoint;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Anyhow<T> = anyhow::Result<T>;
type Sink = SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

#[derive(Parser, Debug)]
#[command(about = "Load test for cmuxd serve")]
struct Opts {
    /// Fake claude agents that print at a steady rate.
    #[arg(long, env = "LOAD_AGENTS", default_value_t = 10)]
    agents: usize,
    /// WebSocket clients subscribed to every agent terminal.
    #[arg(long, env = "LOAD_CLIENTS", default_value_t = 3)]
    clients: usize,
    /// Each agent prints one line every this many milliseconds.
    #[arg(long, env = "LOAD_PRINT_MS", default_value_t = 100)]
    print_ms: u64,
    /// Echo samples (one byte each).
    #[arg(long, env = "LOAD_SAMPLES", default_value_t = 200)]
    samples: usize,
    /// Pause between two echo samples, in milliseconds.
    #[arg(long, env = "LOAD_SAMPLE_GAP_MS", default_value_t = 25)]
    sample_gap_ms: u64,
    /// The busy CPU window, in seconds.
    #[arg(long, env = "LOAD_BUSY_SECS", default_value_t = 30)]
    busy_secs: u64,
    /// The idle CPU window, in seconds.
    #[arg(long, env = "LOAD_IDLE_SECS", default_value_t = 30)]
    idle_secs: u64,
    /// A prebuilt cmuxd binary; the fake claude must be in its `examples/`.
    #[arg(long, env = "CMUXD_BIN")]
    cmuxd: Option<PathBuf>,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Anyhow<()> {
    let opts = Opts::parse();
    let (cmuxd, fake) = binaries(opts.cmuxd.clone())?;
    let tmp = tempfile::Builder::new().prefix("cmux-load").tempdir()?;
    let root = tmp.path().canonicalize()?;

    let mut server = Serve::start(&root, &cmuxd, &fake, &opts)?;
    let result = run(&opts, &root, &server).await;
    let rss_kb = server.rss_kb();
    server.stop();
    let r = result?;
    report(&opts, &r, rss_kb?);
    Ok(())
}

struct Results {
    echo_ms: Vec<f64>,
    busy_cpu: f64,
    idle_cpu: f64,
    bytes_per_client: Vec<u64>,
}

async fn run(opts: &Opts, root: &Path, server: &Serve) -> Anyhow<Results> {
    let unix = Endpoint::Unix(server.socket.clone());
    let tcp = Endpoint::Tcp(server.addr);
    wait_for("the server", Duration::from_secs(20), async || {
        unix.request("GET", route::STATUS, &[], None).await.ok()
    })
    .await?;
    let repo = make_repo(root)?;

    // One plan per agent: its executor is the fake claude.
    for i in 0..opts.agents {
        let r = unix
            .json(
                "POST",
                route::PLANS,
                &[],
                &json!({"title": format!("load {i}"),
                        "repos": [{"path": repo.display().to_string()}],
                        "request": "print"}),
            )
            .await?;
        anyhow::ensure!(r.status == 201, "create plan: {} {}", r.status, r.text());
    }
    let agent_terms = wait_for("the agents", Duration::from_secs(60), async || {
        let v = state(&unix).await.ok()?;
        let terms: Vec<TermId> = v.agents.iter().filter_map(|a| a.term).collect();
        (terms.len() >= opts.agents).then_some(terms)
    })
    .await?;
    let echo_term = {
        let r = unix
            .json(
                "POST",
                route::TERMINALS,
                &[],
                &CreateTerminalReq {
                    kind: NewTerminalKind::Command {
                        argv: vec!["sh".into(), "-c".into(), "stty raw -echo; exec cat".into()],
                        label: Some("echo".into()),
                    },
                    cwd: Some(root.display().to_string()),
                    rows: Some(24),
                    cols: Some(80),
                },
            )
            .await?;
        anyhow::ensure!(r.status == 201, "create terminal: {}", r.text());
        r.json::<TerminalView>()?.id
    };

    let echoes = Arc::new(Counter::default());
    let mut clients = Vec::new();
    for i in 0..opts.clients {
        let echo = (i == 0).then(|| (echo_term, echoes.clone()));
        clients.push(Client::connect(&unix, &tcp, &agent_terms, echo).await?);
    }
    // Let `stty raw` take effect and the agents get going.
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut echo_ms = Vec::with_capacity(opts.samples);
    if let Some(c) = clients.first_mut() {
        for _ in 0..opts.samples {
            let before = echoes.n.load(Ordering::SeqCst);
            let t0 = Instant::now();
            c.input(echo_term, b"k").await?;
            tokio::time::timeout(Duration::from_secs(5), echoes.above(before))
                .await
                .map_err(|_| anyhow::anyhow!("no echo within 5 s"))?;
            echo_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            tokio::time::sleep(Duration::from_millis(opts.sample_gap_ms)).await;
        }
    }

    let busy_cpu = server
        .cpu_percent(Duration::from_secs(opts.busy_secs))
        .await?;
    // The scripts end soon after the busy window.
    wait_for("the agents to idle", Duration::from_secs(120), async || {
        let v = state(&unix).await.ok()?;
        v.agents
            .iter()
            .all(|a| a.activity == Activity::Idle)
            .then_some(())
    })
    .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let idle_cpu = server
        .cpu_percent(Duration::from_secs(opts.idle_secs))
        .await?;

    let bytes_per_client = clients
        .iter()
        .map(|c| c.bytes.load(Ordering::SeqCst))
        .collect();
    Ok(Results {
        echo_ms,
        busy_cpu,
        idle_cpu,
        bytes_per_client,
    })
}

fn report(opts: &Opts, r: &Results, rss_kb: u64) {
    let mut e = r.echo_ms.clone();
    e.sort_by(f64::total_cmp);
    let pct = |p: f64| {
        e.get(((e.len() as f64 * p).ceil() as usize).saturating_sub(1))
            .copied()
            .unwrap_or(f64::NAN)
    };
    let (p50, p95, max) = (pct(0.50), pct(0.95), e.last().copied().unwrap_or(f64::NAN));
    let rss_mb = rss_kb as f64 / 1024.0;
    let ok = |b: bool| if b { "ok" } else { "MISS" };

    println!();
    println!(
        "cmuxd load: {} agents (1 line / {} ms each), {} clients, {} echo samples",
        opts.agents,
        opts.print_ms,
        opts.clients,
        e.len()
    );
    println!("bytes streamed per client: {:?}", r.bytes_per_client);
    println!();
    println!("| metric                         | value      | target   |      |");
    println!("|--------------------------------|------------|----------|------|");
    println!(
        "| idle CPU ({:>3} s, agents idle)  | {:>8.2} % | < 1 %    | {:<4} |",
        opts.idle_secs,
        r.idle_cpu,
        ok(r.idle_cpu < 1.0)
    );
    println!(
        "| busy CPU ({:>3} s, agents print) | {:>8.2} % | -        |      |",
        opts.busy_secs, r.busy_cpu
    );
    println!(
        "| echo p50                       | {p50:>7.2} ms | < 5 ms   | {:<4} |",
        ok(p50 < 5.0)
    );
    println!(
        "| echo p95                       | {p95:>7.2} ms | < 5 ms   | {:<4} |",
        ok(p95 < 5.0)
    );
    println!("| echo max                       | {max:>7.2} ms | -        |      |");
    println!(
        "| RSS at the end                 | {rss_mb:>7.1} MB | < 150 MB | {:<4} |",
        ok(rss_mb < 150.0)
    );
}

// ---------------------------------------------------------------------------
// The server process
// ---------------------------------------------------------------------------

/// `cmuxd` and the fake claude, built in this example's profile.
fn binaries(given: Option<PathBuf>) -> Anyhow<(PathBuf, PathBuf)> {
    let (cmuxd, built) = match given {
        Some(p) => (p, true),
        None => {
            // target/<profile>/examples/load
            let exe = std::env::current_exe()?;
            let dir = exe
                .parent()
                .and_then(Path::parent)
                .ok_or_else(|| anyhow::anyhow!("no target dir above {}", exe.display()))?;
            (dir.join("cmuxd"), false)
        }
    };
    let dir = cmuxd.parent().unwrap_or(Path::new("."));
    let fake = dir.join("examples").join("fake-claude");
    if !built {
        let release = dir.file_name().is_some_and(|n| n == "release");
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let mut c = Command::new(cargo);
        c.args([
            "build",
            "-p",
            "cmuxd",
            "--bin",
            "cmuxd",
            "--example",
            "fake-claude",
        ]);
        if release {
            c.arg("--release");
        }
        anyhow::ensure!(c.status()?.success(), "cargo build failed");
    }
    anyhow::ensure!(cmuxd.exists(), "no cmuxd at {}", cmuxd.display());
    anyhow::ensure!(fake.exists(), "no fake claude at {}", fake.display());
    Ok((cmuxd, fake))
}

struct Serve {
    child: std::process::Child,
    socket: PathBuf,
    addr: std::net::SocketAddr,
}

impl Serve {
    fn start(root: &Path, cmuxd: &Path, fake: &Path, opts: &Opts) -> Anyhow<Serve> {
        let scripts = root.join("scripts");
        std::fs::create_dir_all(&scripts)?;
        // Print and sleep steps for the echo and busy windows plus a margin;
        // then the turn ends and the agent idles.
        let window =
            2 + opts.busy_secs + 20 + (opts.samples as u64 * (opts.sample_gap_ms + 10)) / 1000;
        let lines = (window * 1000 / opts.print_ms.max(1)).max(1);
        let mut steps = Vec::with_capacity(lines as usize * 2 + 1);
        for n in 0..lines {
            steps.push(json!({"print": format!(
                "load line {n:06} the quick brown fox jumps over the lazy dog 0123456789"
            )}));
            steps.push(json!({"sleep_ms": opts.print_ms}));
        }
        std::fs::write(
            scripts.join("executor.json"),
            json!({"first": steps, "later": []}).to_string(),
        )?;
        let claude = root.join("claude");
        std::fs::write(
            &claude,
            format!(
                "#!/bin/sh\nFAKE_CLAUDE_SCRIPTS='{}' exec '{}' \"$@\"\n",
                scripts.display(),
                fake.display()
            ),
        )?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755))?;
        }

        let addr = std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?;
        let socket = root.join("api.sock");
        let config = root.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "data_dir = {data:?}\nsocket = {socket:?}\nlisten = \"{addr}\"\n\
                 [agents]\nclaude_bin = {claude:?}\npre_trust = false\n\
                 [limits]\nlive_processes = {live}\n\
                 [github]\nenabled = false\n",
                data = root.join("data").display().to_string(),
                socket = socket.display().to_string(),
                claude = claude.display().to_string(),
                live = opts.agents + 4,
            ),
        )?;
        let log = std::fs::File::create(root.join("cmuxd.log"))?;
        let child = Command::new(cmuxd)
            .arg("--config")
            .arg(&config)
            .arg("serve")
            .env("HOME", root)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?;
        Ok(Serve {
            child,
            socket,
            addr,
        })
    }

    /// The CPU of the cmuxd process alone over `window`, in % of one core.
    async fn cpu_percent(&self, window: Duration) -> Anyhow<f64> {
        let pid = self.child.id();
        let (c0, t0) = (cpu_secs(pid)?, Instant::now());
        tokio::time::sleep(window).await;
        let (c1, t1) = (cpu_secs(pid)?, Instant::now());
        Ok((c1 - c0) / (t1 - t0).as_secs_f64() * 100.0)
    }

    fn rss_kb(&self) -> Anyhow<u64> {
        let out = ps(self.child.id(), "rss=")?;
        Ok(out.trim().parse()?)
    }

    fn stop(&mut self) {
        // SIGTERM: cmuxd stops its agents and exits.
        unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn ps(pid: u32, field: &str) -> Anyhow<String> {
    let out = Command::new("ps")
        .args(["-o", field, "-p", &pid.to_string()])
        .output()?;
    anyhow::ensure!(out.status.success(), "ps: cmuxd ({pid}) is gone");
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// User plus system CPU seconds of `pid` (not its children).
#[cfg(target_os = "linux")]
fn cpu_secs(pid: u32) -> Anyhow<f64> {
    // `ps` on Linux rounds to whole seconds; /proc has clock ticks.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let rest = stat
        .rsplit_once(')')
        .ok_or_else(|| anyhow::anyhow!("bad /proc stat"))?
        .1;
    // After the name: state is field 3, utime 14 and stime 15.
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks: f64 = f[11].parse::<f64>()? + f[12].parse::<f64>()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    Ok(ticks / hz)
}

/// User plus system CPU seconds of `pid` (not its children).
#[cfg(not(target_os = "linux"))]
fn cpu_secs(pid: u32) -> Anyhow<f64> {
    parse_cputime(ps(pid, "time=")?.trim())
}

/// `[[DD-]HH:]MM:SS[.ss]`, as `ps -o time` prints it.
#[cfg(not(target_os = "linux"))]
fn parse_cputime(s: &str) -> Anyhow<f64> {
    let (days, hms) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<f64>()?, r),
        None => (0.0, s),
    };
    let mut secs = 0.0;
    for part in hms.split(':') {
        secs = secs * 60.0 + part.parse::<f64>()?;
    }
    Ok(days * 86400.0 + secs)
}

fn make_repo(root: &Path) -> Anyhow<PathBuf> {
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo)?;
    std::fs::write(repo.join("a.txt"), "a\n")?;
    for args in [
        &["init", "--initial-branch=main"][..],
        &["add", "."],
        &["commit", "-m", "first"],
    ] {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .output()?;
        anyhow::ensure!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(repo)
}

async fn state(unix: &Endpoint) -> Anyhow<StateView> {
    Ok(unix.request("GET", route::STATE, &[], None).await?.json()?)
}

async fn wait_for<T>(
    what: &str,
    limit: Duration,
    mut f: impl AsyncFnMut() -> Option<T>,
) -> Anyhow<T> {
    let until = Instant::now() + limit;
    loop {
        if let Some(v) = f().await {
            return Ok(v);
        }
        anyhow::ensure!(Instant::now() < until, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// WebSocket clients
// ---------------------------------------------------------------------------

/// Echoed bytes seen on the echo terminal.
#[derive(Default)]
struct Counter {
    n: AtomicU64,
    changed: Notify,
}

impl Counter {
    async fn above(&self, before: u64) {
        loop {
            let notified = self.changed.notified();
            if self.n.load(Ordering::SeqCst) > before {
                return;
            }
            notified.await;
        }
    }
}

struct Client {
    sink: Sink,
    bytes: Arc<AtomicU64>,
}

impl Client {
    /// Pairs, connects, says hello and subscribes to `terms` (and the echo
    /// terminal, whose output bytes go to the counter).
    async fn connect(
        unix: &Endpoint,
        tcp: &Endpoint,
        terms: &[TermId],
        echo: Option<(TermId, Arc<Counter>)>,
    ) -> Anyhow<Client> {
        let r = unix
            .json("POST", route::PAIR_START, &[], &PairStartReq::default())
            .await?;
        anyhow::ensure!((200..300).contains(&r.status), "pair start: {}", r.text());
        let code = r.json::<PairStartResp>()?.code;
        let r = tcp
            .json(
                "POST",
                route::PAIR_COMPLETE,
                &[],
                &PairCompleteReq {
                    code,
                    device_name: "load".into(),
                },
            )
            .await?;
        anyhow::ensure!(
            (200..300).contains(&r.status),
            "pair complete: {}",
            r.text()
        );
        let token = r.json::<PairCompleteResp>()?.token;

        let Endpoint::Tcp(addr) = tcp else {
            unreachable!()
        };
        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/stream")).await?;
        let (mut sink, mut stream) = ws.split();
        let send = async |sink: &mut Sink, m: &ClientMsg| -> Anyhow<()> {
            sink.send(Message::text(serde_json::to_string(m)?)).await?;
            Ok(())
        };
        send(
            &mut sink,
            &ClientMsg::Hello {
                client: ClientKind::Tui,
                since_rev: None,
                token: Some(token),
                info: Some("cmuxd load".into()),
                terms: vec![],
            },
        )
        .await?;
        let mut subs: HashSet<TermId> = terms.iter().copied().collect();
        if let Some((t, _)) = &echo {
            subs.insert(*t);
        }
        for term in subs {
            send(
                &mut sink,
                &ClientMsg::Subscribe {
                    term,
                    since_seq: None,
                },
            )
            .await?;
        }

        let bytes = Arc::new(AtomicU64::new(0));
        let counted = bytes.clone();
        tokio::spawn(async move {
            while let Some(Ok(m)) = stream.next().await {
                let Message::Binary(b) = m else { continue };
                let Ok(Frame::Output { term, data, .. }) = Frame::decode(&b) else {
                    continue;
                };
                counted.fetch_add(data.len() as u64, Ordering::Relaxed);
                if let Some((t, c)) = &echo
                    && term == *t
                {
                    let k = data.iter().filter(|&&b| b == b'k').count() as u64;
                    c.n.fetch_add(k, Ordering::SeqCst);
                    c.changed.notify_waiters();
                }
            }
        });
        Ok(Client { sink, bytes })
    }

    async fn input(&mut self, term: TermId, data: &[u8]) -> Anyhow<()> {
        let f = Frame::Input {
            term,
            data: data.to_vec(),
        };
        self.sink.send(Message::binary(f.encode())).await?;
        Ok(())
    }
}
