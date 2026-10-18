//! A child process on its own PTY. This is claude-mux's `pane.rs` without
//! drawing: spawn, a blocking reader thread, input, resize, kill and the exit
//! status. The hub owns the screen state.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Variables of an enclosing Claude session that must not leak into children.
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

/// What to run and how.
#[derive(Debug, Clone)]
pub struct PtySpec {
    pub program: String,
    pub args: Vec<String>,
    /// Extra environment, applied after the defaults (`TERM`, `COLORTERM`).
    pub env: Vec<(String, String)>,
    /// Variables to remove from the inherited environment.
    pub env_remove: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub rows: u16,
    pub cols: u16,
}

impl PtySpec {
    /// A spec with the default 24x80 size, no extra env and the inherited cwd.
    pub fn new(program: impl Into<String>, args: &[&str]) -> Self {
        PtySpec {
            program: program.into(),
            args: args.iter().map(|a| a.to_string()).collect(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: None,
            rows: 24,
            cols: 80,
        }
    }
}

/// How the child ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: u32,
    pub signal: Option<String>,
}

/// A running PTY child. Dropping it does not kill the child; call `kill`.
pub struct Pty {
    /// Input for the writer thread, which alone touches the PTY master's
    /// writer: a child that stops reading blocks that thread, nobody else.
    input: std::sync::mpsc::Sender<Vec<u8>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    pid: Option<u32>,
}

impl Pty {
    /// Spawn `spec`. `on_output` is called from the reader thread for each
    /// chunk the child writes; `on_exit` is called once from the same thread
    /// after EOF, once the child has been reaped.
    pub fn spawn(
        spec: &PtySpec,
        mut on_output: impl FnMut(&[u8]) + Send + 'static,
        on_exit: impl FnOnce(ExitInfo) + Send + 'static,
    ) -> Result<Pty> {
        let pair = native_pty_system()
            .openpty(size(spec.rows, spec.cols))
            .context("openpty failed")?;

        let mut cmd = CommandBuilder::new(&spec.program);
        cmd.args(&spec.args);
        if let Some(cwd) = &spec.cwd {
            cmd.cwd(cwd);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env_remove("TMUX");
        for var in SESSION_ENV {
            cmd.env_remove(var);
        }
        for var in &spec.env_remove {
            cmd.env_remove(var);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let mut child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to start {}", spec.program))?;
        // Drop our slave copy so reads on the master see EOF when the child exits.
        drop(pair.slave);

        let pid = child.process_id();
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let (input, input_rx) = std::sync::mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name(format!("pty-in-{}", pid.unwrap_or(0)))
            .spawn(move || {
                while let Ok(bytes) = input_rx.recv() {
                    if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                        // The child is gone; drop what is still queued.
                        break;
                    }
                }
            })?;

        std::thread::Builder::new()
            .name(format!("pty-{}", pid.unwrap_or(0)))
            .spawn(move || {
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => on_output(&buf[..n]),
                    }
                }
                on_exit(match child.wait() {
                    Ok(s) => ExitInfo {
                        code: s.exit_code(),
                        signal: s.signal().map(str::to_string),
                    },
                    Err(_) => ExitInfo {
                        code: 1,
                        signal: None,
                    },
                });
            })?;

        Ok(Pty {
            input,
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            pid,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Queue input for the child; never blocks. Order is kept across callers.
    /// Errors (child gone) are ignored.
    pub fn write(&self, bytes: &[u8]) {
        let _ = self.input.send(bytes.to_vec());
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        if let Ok(m) = self.master.lock() {
            let _ = m.resize(size(rows, cols));
        }
    }

    pub fn kill(&self) {
        if let Ok(mut k) = self.killer.lock() {
            let _ = k.kill();
        }
    }
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}
