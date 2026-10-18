//! A child process on its own PTY. This is claude-mux's `pane.rs` without
//! drawing: spawn, a blocking reader thread, input, resize, kill and the exit
//! status. The hub owns the screen state.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::claude::spawn::is_session_env;
use anyhow::{Context, Result};
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

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

/// Input bytes queued for the writer thread at most. A child that stopped
/// reading would otherwise let a pasting client grow the queue without limit.
pub const INPUT_QUEUE_BYTES: usize = 1 << 20;

/// A running PTY child. Dropping it does not kill the child; call `kill`.
pub struct Pty {
    /// Input for the writer thread, which alone touches the PTY master's
    /// writer: a child that stops reading blocks that thread, nobody else.
    /// `None` once [`close`](Pty::close)d, which ends the writer thread.
    input: Mutex<Option<std::sync::mpsc::Sender<Vec<u8>>>>,
    /// Bytes sent to the writer thread and not yet written.
    queued: Arc<AtomicUsize>,
    master: Mutex<Option<Box<dyn MasterPty + Send>>>,
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
        for (k, _) in std::env::vars_os() {
            if k.to_str().is_some_and(is_session_env) {
                cmd.env_remove(k);
            }
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
        let queued = Arc::new(AtomicUsize::new(0));
        let written = queued.clone();
        std::thread::Builder::new()
            .name(format!("pty-in-{}", pid.unwrap_or(0)))
            .spawn(move || {
                while let Ok(bytes) = input_rx.recv() {
                    written.fetch_sub(bytes.len(), Ordering::Relaxed);
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
            input: Mutex::new(Some(input)),
            queued,
            master: Mutex::new(Some(pair.master)),
            killer: Mutex::new(killer),
            pid,
        })
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Queue input for the child; never blocks. Order is kept across callers.
    /// Returns false when the bytes were dropped: the child is gone, or it
    /// has stopped reading and [`INPUT_QUEUE_BYTES`] are already waiting (a
    /// write into an empty queue is always accepted, however large).
    pub fn write(&self, bytes: &[u8]) -> bool {
        let guard = self.input.lock().unwrap_or_else(|e| e.into_inner());
        let Some(tx) = guard.as_ref() else {
            return false;
        };
        let waiting = self.queued.load(Ordering::Relaxed);
        if waiting > 0 && waiting + bytes.len() > INPUT_QUEUE_BYTES {
            return false;
        }
        self.queued.fetch_add(bytes.len(), Ordering::Relaxed);
        if tx.send(bytes.to_vec()).is_err() {
            self.queued.fetch_sub(bytes.len(), Ordering::Relaxed);
            return false;
        }
        true
    }

    pub fn resize(&self, rows: u16, cols: u16) {
        if let Ok(m) = self.master.lock()
            && let Some(m) = m.as_ref()
        {
            let _ = m.resize(size(rows, cols));
        }
    }

    /// Release the master fd and end the writer thread once the child has
    /// exited. Later writes and resizes do nothing.
    pub fn close(&self) {
        self.input.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.master.lock().unwrap_or_else(|e| e.into_inner()).take();
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
