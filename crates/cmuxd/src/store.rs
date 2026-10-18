//! The data directory: exclusive lock, state load, atomic debounced saves and
//! the per-plan journal (architecture 5.4).
//!
//! ```text
//! <data>/cmuxd.lock               POSIX record lock; held for the life of the process
//! <data>/plans/<plan>/state.json  one `Plan`
//! <data>/plans/<plan>/events.jsonl
//! <data>/inbox.json               inbox items and the next item id
//! <data>/loose.json               rev, loose terminals, limits, next term id
//! <data>/events.jsonl             journal lines that belong to no plan
//! ```
//!
//! Everything is written temp + fsync + rename with mode 0600. Saves go through
//! a [`StoreWriter`] thread that coalesces them for 200 ms, so the engine
//! actor never waits for the disk.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use cmux_core::ids::{PlanId, TermId};
use cmux_core::model::{GlobalLimits, InboxItem, Plan, State, Terminal, Timeouts};
use serde::{Deserialize, Serialize};

pub const DEBOUNCE: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub enum StoreError {
    /// Another `cmuxd` holds the lock on this data directory.
    Locked(PathBuf),
    Io(PathBuf, io::Error),
    Corrupt(PathBuf, String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Locked(d) => write!(
                f,
                "another cmuxd is already running on {} (lock held)",
                d.display()
            ),
            StoreError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            StoreError::Corrupt(p, e) => write!(f, "{} is corrupt: {e}", p.display()),
        }
    }
}

impl std::error::Error for StoreError {}

/// The fields of `State` that belong to no plan, apart from the inbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Globals {
    pub rev: u64,
    pub loose: Vec<TermId>,
    pub terminals: BTreeMap<TermId, Terminal>,
    pub next_term_id: u32,
    pub global_limits: GlobalLimits,
    pub timeouts: Timeouts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxFile {
    pub inbox: Vec<InboxItem>,
    pub next_item_id: u64,
}

impl Globals {
    pub fn of(s: &State) -> Self {
        Globals {
            rev: s.rev,
            loose: s.loose.clone(),
            terminals: s.terminals.clone(),
            next_term_id: s.next_term_id,
            global_limits: s.global_limits,
            timeouts: s.timeouts,
        }
    }
}

/// Data dirs this process has open, by canonical path.
fn claimed() -> &'static Mutex<HashSet<PathBuf>> {
    static CLAIMED: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    CLAIMED.get_or_init(Mutex::default)
}

/// This process's claim on one data dir. Dropped after the lock file.
#[derive(Debug)]
struct Claim(PathBuf);

impl Claim {
    fn take(key: PathBuf) -> Option<Claim> {
        let mut set = claimed().lock().unwrap_or_else(PoisonError::into_inner);
        // Not `then_some(Claim(key))`: that would drop a Claim here, under the
        // lock, when the key is taken, and its Drop would deadlock.
        if set.insert(key.clone()) {
            Some(Claim(key))
        } else {
            None
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        claimed()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.0);
    }
}

/// Takes an exclusive record lock on the whole file, without blocking.
///
/// Unlike `flock(2)`, a record lock belongs to the process, not to the open
/// file. A child forked while we hold it (the git and PTY spawns in this
/// process, say) does not inherit it, so it cannot keep the directory locked
/// after we let go. With `flock` the child's copy of the fd did exactly that
/// until the child execed, which made reopens flake under load.
fn lock_exclusive(file: &File) -> io::Result<()> {
    // SAFETY: an all-zero `flock` is valid; the fields we use are set below.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_WRLCK as _;
    fl.l_whence = libc::SEEK_SET as _;
    // SAFETY: the fd is open and `fl` is live for the whole call.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &fl as *const libc::flock) };
    if rc == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// An open, locked data directory.
#[derive(Debug)]
pub struct Store {
    dir: PathBuf,
    /// Closing the file releases the lock. It is declared before `_claim`, so
    /// the lock is gone before this process lets another `Store` claim the dir.
    _lock: File,
    _claim: Claim,
}

impl Store {
    /// Creates the directory if needed and takes the exclusive lock.
    pub fn open(dir: &Path) -> Result<Store, StoreError> {
        let io_err = |p: &Path| {
            let p = p.to_path_buf();
            move |e| StoreError::Io(p, e)
        };
        crate::config::ensure_private_dir(dir).map_err(io_err(dir))?;
        // A second `Store` in this process is refused before the file is
        // touched: closing any fd on the file drops this process's record
        // locks on it, including the live one.
        let key = fs::canonicalize(dir).map_err(io_err(dir))?;
        let claim = Claim::take(key).ok_or_else(|| StoreError::Locked(dir.to_path_buf()))?;
        let lock_path = dir.join("cmuxd.lock");
        let mut lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(&lock_path)
            .map_err(io_err(&lock_path))?;
        if let Err(e) = lock_exclusive(&lock) {
            return if matches!(e.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)) {
                Err(StoreError::Locked(dir.to_path_buf()))
            } else {
                Err(StoreError::Io(lock_path, e))
            };
        }
        let _ = lock.set_len(0);
        let _ = writeln!(lock, "{}", std::process::id());
        Ok(Store {
            dir: dir.to_path_buf(),
            _lock: lock,
            _claim: claim,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn plan_dir(&self, plan: &PlanId) -> PathBuf {
        self.dir.join("plans").join(plan.as_str())
    }

    /// Reads everything. An empty directory gives a default `State`.
    pub fn load(&self) -> Result<State, StoreError> {
        load_dir(&self.dir)
    }

    pub fn save_plan(&self, plan: &Plan) -> Result<(), StoreError> {
        let path = self.plan_dir(&plan.id).join("state.json");
        write_json(&path, plan)
    }

    pub fn save_globals(&self, g: &Globals, inbox: &InboxFile) -> Result<(), StoreError> {
        write_json(&self.dir.join("loose.json"), g)?;
        write_json(&self.dir.join("inbox.json"), inbox)
    }

    /// Removes the plan's directory. A missing directory is not an error.
    pub fn delete_plan(&self, plan: &PlanId) -> Result<(), StoreError> {
        let dir = self.plan_dir(plan);
        match fs::remove_dir_all(&dir) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(StoreError::Io(dir, e)),
            _ => Ok(()),
        }
    }

    /// Appends one line to the plan's journal (or the global one).
    pub fn append_journal(&self, plan: Option<&PlanId>, line: &str) -> Result<(), StoreError> {
        let dir = plan.map_or_else(|| self.dir.clone(), |p| self.plan_dir(p));
        let path = dir.join("events.jsonl");
        let go = || -> io::Result<()> {
            crate::config::ensure_private_dir(&dir)?;
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&path)?;
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")
        };
        go().map_err(|e| StoreError::Io(path, e))
    }
}

/// [`Store::load`] without the lock, for a reader such as `cmuxd doctor`
/// while the server runs. Saves are atomic renames, so each file it reads is
/// whole.
pub fn load_dir(dir: &Path) -> Result<State, StoreError> {
    let mut st = State {
        data_dir: dir.to_path_buf(),
        ..State::default()
    };
    if let Some(g) = read_json::<Globals>(&dir.join("loose.json"))? {
        st.rev = g.rev;
        st.loose = g.loose;
        st.terminals = g.terminals;
        st.next_term_id = g.next_term_id;
        st.global_limits = g.global_limits;
        st.timeouts = g.timeouts;
    }
    if let Some(i) = read_json::<InboxFile>(&dir.join("inbox.json"))? {
        st.inbox = i.inbox;
        st.next_item_id = i.next_item_id;
    }
    let plans = dir.join("plans");
    let rd = match fs::read_dir(&plans) {
        Ok(rd) => rd,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(st),
        Err(e) => return Err(StoreError::Io(plans, e)),
    };
    for entry in rd {
        let entry = entry.map_err(|e| StoreError::Io(plans.clone(), e))?;
        if let Some(plan) = read_json::<Plan>(&entry.path().join("state.json"))? {
            st.plans.push(plan);
        }
    }
    // Plan ids start with their date, so this is creation order.
    st.plans.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    // A crash between the files must not let ids or rev run backwards.
    let newest = st.inbox.iter().map(|i| i.id.0 + 1).max().unwrap_or(0);
    st.next_item_id = st.next_item_id.max(newest);
    let newest = st.terminals.keys().map(|t| t.0 + 1).max().unwrap_or(0);
    st.next_term_id = st.next_term_id.max(newest);
    Ok(st)
}

fn read_json<T: for<'a> Deserialize<'a>>(path: &Path) -> Result<Option<T>, StoreError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| StoreError::Corrupt(path.to_path_buf(), e.to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(StoreError::Io(path.to_path_buf(), e)),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StoreError> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|e| StoreError::Corrupt(path.to_path_buf(), e.to_string()))?;
    atomic_write(path, &bytes).map_err(|e| StoreError::Io(path.to_path_buf(), e))
}

/// Temp file in the same directory, fsync, rename, fsync the directory. Mode 0600.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    crate::config::ensure_private_dir(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)?;
        // Make the rename itself durable.
        File::open(dir).and_then(|d| d.sync_all())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

// ---------------------------------------------------------------------------
// The writer thread
// ---------------------------------------------------------------------------

enum Msg {
    Plan(Box<Plan>),
    Globals(Box<(Globals, InboxFile)>),
    Journal(Option<PlanId>, String),
    DeletePlan(PlanId),
    Flush(mpsc::Sender<()>),
    /// Flush, release the data directory and end the thread.
    Close(mpsc::Sender<()>),
}

/// Debounced, asynchronous saves. Cheap to clone; the thread flushes and ends
/// when every clone is dropped. Call [`StoreWriter::flush`] where durability matters.
#[derive(Clone)]
pub struct StoreWriter {
    tx: mpsc::Sender<Msg>,
}

impl StoreWriter {
    pub fn start(store: Arc<Store>) -> StoreWriter {
        Self::start_with(store, DEBOUNCE)
    }

    pub fn start_with(store: Arc<Store>, debounce: Duration) -> StoreWriter {
        let (tx, rx) = mpsc::channel();
        // Detached: it flushes and exits when the last sender is dropped.
        std::thread::Builder::new()
            .name("cmuxd-store".into())
            .spawn(move || writer_loop(store, rx, debounce))
            .expect("spawn store thread");
        StoreWriter { tx }
    }

    /// Queues `plan` (the latest snapshot per plan wins within a debounce window).
    pub fn save_plan(&self, plan: Plan) {
        let _ = self.tx.send(Msg::Plan(Box::new(plan)));
    }

    pub fn save_globals(&self, g: Globals, inbox: InboxFile) {
        let _ = self.tx.send(Msg::Globals(Box::new((g, inbox))));
    }

    /// One journal line, appended as soon as the thread gets to it.
    pub fn journal(&self, plan: Option<PlanId>, line: String) {
        let _ = self.tx.send(Msg::Journal(plan, line));
    }

    /// Drops a queued snapshot of the plan and removes its directory, after
    /// everything queued before it.
    pub fn delete_plan(&self, plan: PlanId) {
        let _ = self.tx.send(Msg::DeletePlan(plan));
    }

    /// Flushes, then ends the writer thread and releases its hold on the
    /// store, so the data-dir lock is free once the caller's own handle goes.
    /// Later saves through any clone are dropped.
    pub fn close(&self) {
        let (ack, wait) = mpsc::channel();
        if self.tx.send(Msg::Close(ack)).is_ok() {
            let _ = wait.recv();
        }
    }

    /// Blocks until everything queued so far is on disk.
    pub fn flush(&self) {
        let (ack, wait) = mpsc::channel();
        if self.tx.send(Msg::Flush(ack)).is_ok() {
            let _ = wait.recv();
        }
    }
}

#[derive(Default)]
struct Pending {
    plans: HashMap<PlanId, Plan>,
    globals: Option<(Globals, InboxFile)>,
    since: Option<Instant>,
}

impl Pending {
    fn write(&mut self, store: &Store) {
        for (_, plan) in self.plans.drain() {
            if let Err(e) = store.save_plan(&plan) {
                tracing::error!(target: "store", "save plan failed: {e}");
            }
        }
        if let Some((g, i)) = self.globals.take()
            && let Err(e) = store.save_globals(&g, &i)
        {
            tracing::error!(target: "store", "save globals failed: {e}");
        }
        self.since = None;
    }
}

fn writer_loop(store: Arc<Store>, rx: mpsc::Receiver<Msg>, debounce: Duration) {
    let mut p = Pending::default();
    loop {
        let msg = match p.since {
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            Some(t) => rx.recv_timeout(debounce.saturating_sub(t.elapsed())),
        };
        match msg {
            Ok(Msg::Plan(plan)) => {
                p.since.get_or_insert_with(Instant::now);
                p.plans.insert(plan.id.clone(), *plan);
            }
            Ok(Msg::Globals(g)) => {
                p.since.get_or_insert_with(Instant::now);
                p.globals = Some(*g);
            }
            Ok(Msg::Journal(plan, line)) => {
                if let Err(e) = store.append_journal(plan.as_ref(), &line) {
                    tracing::error!(target: "store", "journal failed: {e}");
                }
            }
            Ok(Msg::DeletePlan(plan)) => {
                p.plans.remove(&plan);
                if let Err(e) = store.delete_plan(&plan) {
                    tracing::error!(target: "store", "delete plan failed: {e}");
                }
            }
            Ok(Msg::Flush(ack)) => {
                p.write(&store);
                let _ = ack.send(());
            }
            Ok(Msg::Close(ack)) => {
                p.write(&store);
                drop(store);
                let _ = ack.send(());
                return;
            }
            Err(RecvTimeoutError::Timeout) => p.write(&store),
            Err(RecvTimeoutError::Disconnected) => {
                p.write(&store);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmux_core::model::Repo;

    fn plan(title: &str) -> Plan {
        let repo = Repo {
            path: "/r".into(),
            base_branch: "main".into(),
            remote_url: None,
        };
        Plan::new(PlanId::new(2026, 10, 18, title), title, vec![repo])
    }

    #[test]
    fn round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(tmp.path()).unwrap());
        let mut st = State {
            data_dir: tmp.path().into(),
            rev: 7,
            next_item_id: 3,
            next_term_id: 2,
            ..State::default()
        };
        st.plans.push(plan("beta"));
        st.plans.push(plan("alpha"));
        let w = StoreWriter::start(store.clone());
        for p in &st.plans {
            w.save_plan(p.clone());
        }
        w.save_globals(
            Globals::of(&st),
            InboxFile {
                inbox: st.inbox.clone(),
                next_item_id: st.next_item_id,
            },
        );
        w.journal(Some(st.plans[0].id.clone()), r#"{"k":1}"#.into());
        w.close();
        drop(store);

        let store = Store::open(tmp.path()).unwrap();
        let back = store.load().unwrap();
        // Loaded in id order, not insertion order.
        assert_eq!(back.plans[0].title, "alpha");
        assert_eq!(back.plans[1].title, "beta");
        assert_eq!((back.rev, back.next_item_id, back.next_term_id), (7, 3, 2));
        assert_eq!(back.data_dir, tmp.path());
        let journal = store.plan_dir(&st.plans[0].id).join("events.jsonl");
        assert_eq!(fs::read_to_string(journal).unwrap(), "{\"k\":1}\n");
        // No temp files are left behind.
        let strays = fs::read_dir(tmp.path().join("plans").join(st.plans[0].id.as_str()))
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(strays, 0);
    }

    #[test]
    fn delete_plan_drops_the_queued_snapshot_and_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(tmp.path()).unwrap());
        let w = StoreWriter::start(store.clone());
        let (keep, gone) = (plan("keep"), plan("gone"));
        w.save_plan(keep.clone());
        w.save_plan(gone.clone());
        w.flush();
        w.journal(Some(gone.id.clone()), r#"{"k":1}"#.into());
        w.save_plan(gone.clone());
        w.delete_plan(gone.id.clone());
        w.flush();
        assert!(!store.plan_dir(&gone.id).exists());
        assert!(store.plan_dir(&keep.id).join("state.json").exists());
        let back = store.load().unwrap();
        assert_eq!(back.plans.len(), 1);
        assert_eq!(back.plans[0].id, keep.id);
    }

    #[test]
    fn second_open_is_refused_until_the_first_closes() {
        let tmp = tempfile::tempdir().unwrap();
        let first = Store::open(tmp.path()).unwrap();
        assert!(matches!(
            Store::open(tmp.path()),
            Err(StoreError::Locked(_))
        ));
        drop(first);
        Store::open(tmp.path()).unwrap();
    }

    #[test]
    fn a_forked_child_does_not_hold_the_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        // SAFETY: the child only sleeps and exits, both safe straight after fork.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe {
                libc::usleep(1_000_000);
                libc::_exit(0);
            }
        }
        drop(store);
        let reopened = Store::open(tmp.path());
        // Reap the child before asserting, so a failure leaves nothing behind.
        let mut status = 0;
        unsafe { libc::waitpid(pid, &mut status, 0) };
        reopened.unwrap();
    }

    #[test]
    fn saves_are_debounced() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(tmp.path()).unwrap());
        let w = StoreWriter::start_with(store.clone(), Duration::from_millis(150));
        let p = plan("x");
        let file = store.plan_dir(&p.id).join("state.json");
        w.save_plan(p);
        std::thread::sleep(Duration::from_millis(40));
        assert!(!file.exists(), "written before the debounce window ended");
        std::thread::sleep(Duration::from_millis(300));
        assert!(file.exists());
    }

    #[test]
    fn corrupt_state_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).unwrap();
        fs::write(tmp.path().join("loose.json"), "{nope").unwrap();
        assert!(matches!(store.load(), Err(StoreError::Corrupt(..))));
    }
}
