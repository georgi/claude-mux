//! A pane: one process (usually `claude`) on its own PTY plus a VT emulator
//! that holds its screen state.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::Event;
use crate::tui::remote_term::RemoteTerm;

pub type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

const SCROLLBACK: usize = 10_000;

const SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_PLUGIN_DATA",
    "AI_AGENT",
];

/// vt100 callbacks. Answers terminal queries that the child sends and records
/// side-channel requests (title, bell, clipboard) for the main loop.
pub struct Callbacks {
    writer: Writer,
    pub title: Option<String>,
    pub bell: bool,
    pub clipboard: Vec<Vec<u8>>,
}

impl Callbacks {
    pub fn new(writer: Writer) -> Callbacks {
        Callbacks {
            writer,
            title: None,
            bell: false,
            clipboard: Vec::new(),
        }
    }

    fn reply(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }
}

impl vt100::Callbacks for Callbacks {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).into_owned());
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        // Re-emitted to the outer terminal as OSC 52 by the main loop.
        let mut seq = b"\x1b]52;".to_vec();
        seq.extend_from_slice(if ty.is_empty() { b"c" } else { ty });
        seq.push(b';');
        seq.extend_from_slice(data);
        seq.push(0x07);
        self.clipboard.push(seq);
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, c, first) {
            // DSR: cursor position report.
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                self.reply(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // DSR: device status OK.
            (None, 'n', 5) => self.reply(b"\x1b[0n"),
            // DA1: VT220 with ANSI color.
            (None, 'c', 0) => self.reply(b"\x1b[?62;22c"),
            // DA2.
            (Some(b'>'), 'c', 0) => self.reply(b"\x1b[>1;10;0c"),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Unknown,
    Busy,
    Idle,
    Other(String),
}

pub struct Pane {
    pub id: u64,
    pub parser: Arc<Mutex<vt100::Parser<Callbacks>>>,
    pub dirty: Arc<AtomicBool>,
    backend: Backend,
    pub pid: Option<u32>,
    /// Claude session id. Known up front for new and resumed sessions; updated
    /// from ~/.claude/sessions when Claude reports a different one (e.g. /clear).
    pub session_id: Option<String>,
    pub cwd: PathBuf,
    /// PTY size as (rows, cols).
    pub size: (u16, u16),
    pub title: Option<String>,
    pub status: Status,
    pub attention: bool,
    pub last_output: Instant,
    pub branch: Option<String>,
    pub kind: Kind,
}

/// Where a pane's bytes come from and go to.
enum Backend {
    /// A child process on a PTY of this machine.
    Local {
        writer: Writer,
        master: Box<dyn MasterPty + Send>,
        child: Box<dyn Child + Send + Sync>,
    },
    /// A terminal of a cmuxd server, streamed over the protocol.
    Remote(RemoteTerm),
}

/// What runs in a pane. Only Claude panes get session status and history links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Claude,
    Shell,
    /// Any other program. The string is its display label, e.g. "lazygit".
    Command(String),
}

pub struct SpawnSpec {
    pub kind: Kind,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
}

impl Pane {
    pub fn spawn(
        id: u64,
        spec: SpawnSpec,
        rows: u16,
        cols: u16,
        events: Sender<Event>,
    ) -> Result<Pane> {
        let size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = native_pty_system()
            .openpty(size)
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        cmd.cwd(&spec.cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("CMUX", "1");
        cmd.env_remove("TMUX");
        // When cmux itself runs inside a Claude session, do not leak that
        // session's identity into the children.
        for var in SESSION_ENV {
            cmd.env_remove(var);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to start {}", spec.program))?;
        // Drop our copy of the slave so reads on the master see EOF when the child exits.
        drop(pair.slave);

        let pid = child.process_id();
        let mut reader = pair.master.try_clone_reader()?;
        let writer: Writer = Arc::new(Mutex::new(pair.master.take_writer()?));

        let callbacks = Callbacks::new(writer.clone());
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows, cols, SCROLLBACK, callbacks,
        )));
        let dirty = Arc::new(AtomicBool::new(false));

        {
            let parser = parser.clone();
            let dirty = dirty.clone();
            std::thread::Builder::new()
                .name(format!("pty-{id}"))
                .spawn(move || {
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                parser.lock().unwrap().process(&buf[..n]);
                                // Coalesce: only one pending notification per tab.
                                if !dirty.swap(true, Ordering::AcqRel)
                                    && events.send(Event::Output(id)).is_err()
                                {
                                    break;
                                }
                            }
                        }
                    }
                    let _ = events.send(Event::Exited(id));
                })?;
        }

        let branch = git_branch(&spec.cwd);
        Ok(Pane {
            id,
            parser,
            dirty,
            backend: Backend::Local {
                writer,
                master: pair.master,
                child,
            },
            pid,
            session_id: spec.session_id,
            kind: spec.kind,
            cwd: spec.cwd,
            size: (rows, cols),
            title: None,
            status: Status::Unknown,
            attention: false,
            last_output: Instant::now(),
            branch,
        })
    }

    /// A pane showing a terminal of a cmuxd server. The parser and dirty flag
    /// are the ones the connection thread feeds; draw code reads them as it
    /// does for a local PTY.
    pub fn remote(id: u64, term: RemoteTerm, kind: Kind, cwd: PathBuf) -> Pane {
        Pane {
            id,
            parser: term.parser.clone(),
            dirty: term.dirty.clone(),
            size: term.size(),
            backend: Backend::Remote(term),
            pid: None,
            session_id: None,
            kind,
            cwd,
            title: None,
            status: Status::Unknown,
            attention: false,
            last_output: Instant::now(),
            branch: None,
        }
    }

    /// The remote terminal behind this pane, if it is one.
    pub fn remote_term(&self) -> Option<&RemoteTerm> {
        match &self.backend {
            Backend::Remote(t) => Some(t),
            Backend::Local { .. } => None,
        }
    }

    pub fn write(&self, bytes: &[u8]) {
        match &self.backend {
            Backend::Local { writer, .. } => {
                if let Ok(mut w) = writer.lock() {
                    let _ = w.write_all(bytes);
                    let _ = w.flush();
                }
            }
            Backend::Remote(t) => t.write(bytes),
        }
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        match &mut self.backend {
            Backend::Local { master, .. } => {
                let _ = master.resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                });
                self.parser
                    .lock()
                    .unwrap()
                    .screen_mut()
                    .set_size(rows, cols);
            }
            Backend::Remote(t) => t.resize(rows, cols),
        }
    }

    /// Ends a local child. A remote pane only lets go of its terminal (the
    /// agent keeps running on the server; stopping it is a REST call).
    pub fn kill(&mut self) {
        match &mut self.backend {
            Backend::Local { child, .. } => {
                let _ = child.kill();
            }
            Backend::Remote(t) => t.detach(),
        }
    }

    pub fn has_exited(&mut self) -> bool {
        match &mut self.backend {
            Backend::Local { child, .. } => matches!(child.try_wait(), Ok(Some(_))),
            Backend::Remote(t) => t.exited(),
        }
    }

    pub fn is_claude(&self) -> bool {
        self.kind == Kind::Claude
    }

    /// Display name: the terminal title the program sets, else the directory
    /// name (Claude) or the program label.
    pub fn name(&self) -> String {
        match &self.kind {
            Kind::Claude => {}
            Kind::Shell | Kind::Command(_) => {
                if let Some(t) = self
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    return t.to_string();
                }
                return match &self.kind {
                    Kind::Command(label) => label.clone(),
                    _ => std::env::var("SHELL")
                        .ok()
                        .and_then(|s| s.rsplit('/').next().map(str::to_string))
                        .unwrap_or_else(|| "shell".into()),
                };
            }
        }
        if let Some(t) = &self.title {
            let t = t.trim_start_matches(|c: char| !c.is_alphanumeric()).trim();
            if !t.is_empty() && t != "Claude Code" {
                return t.to_string();
            }
        }
        self.cwd
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into())
    }
}

/// Read the current branch from .git/HEAD, walking up from `dir`.
pub fn git_branch(dir: &std::path::Path) -> Option<String> {
    let mut d = Some(dir);
    while let Some(p) = d {
        let git = p.join(".git");
        let head_path = if git.is_dir() {
            git.join("HEAD")
        } else if git.is_file() {
            // Worktree: "gitdir: <path>"
            let s = std::fs::read_to_string(&git).ok()?;
            PathBuf::from(s.strip_prefix("gitdir:")?.trim()).join("HEAD")
        } else {
            d = p.parent();
            continue;
        };
        let head = std::fs::read_to_string(head_path).ok()?;
        let head = head.trim();
        return Some(match head.strip_prefix("ref: refs/heads/") {
            Some(b) => b.to_string(),
            None => head.chars().take(8).collect(),
        });
    }
    None
}
