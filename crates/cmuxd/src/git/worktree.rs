//! Task worktrees: `<repo>/.worktrees/<task-id>` on branch `claude/<task-id>`.
//!
//! The procedure (architecture 7.1):
//! 1. reuse the task's worktree if it exists;
//! 2. reuse any checkout of the branch;
//! 3. otherwise `git worktree add -b <branch> <path> origin/<base>`, or add
//!    without `-b` when the branch already exists;
//! 4. keep `.worktrees/` in `.git/info/exclude`.
//!
//! Everything shells out to `git`. `WorktreePool` runs the blocking work on
//! tokio's blocking pool, at most 2 operations at a time.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;

use tokio::sync::Semaphore;

/// Concurrent git operations (architecture section 10).
pub const MAX_CONCURRENT: usize = 2;

/// The directory under the repo that holds task worktrees, and its exclude line.
pub const WORKTREE_DIR: &str = ".worktrees";

/// What `CreateWorktree` asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeRequest {
    pub repo_path: PathBuf,
    /// Directory name under `.worktrees/` (the task id, `T-0003`).
    pub name: String,
    /// Base branch; the worktree starts at `origin/<base>`.
    pub base: String,
    /// Branch to check out (`claude/T-0003`).
    pub branch: String,
}

/// How the worktree came to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Step 1: the task's own directory existed.
    ReusedDir,
    /// Step 2: the branch was already checked out somewhere.
    ReusedBranch,
    /// Step 3: created, on a new branch.
    CreatedNewBranch,
    /// Step 3: created, on a branch that already existed.
    CreatedExistingBranch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The branch actually checked out there (`None` when detached).
    pub branch: Option<String>,
    pub origin: Origin,
    /// A reused worktree has uncommitted changes. It is left alone.
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitError(pub String);

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for GitError {}

type Result<T> = std::result::Result<T, GitError>;

/// Runs worktree operations on the blocking pool, 2 at a time.
#[derive(Debug, Clone)]
pub struct WorktreePool {
    permits: Arc<Semaphore>,
}

impl Default for WorktreePool {
    fn default() -> Self {
        Self::new(MAX_CONCURRENT)
    }
}

impl WorktreePool {
    pub fn new(max_concurrent: usize) -> Self {
        WorktreePool {
            permits: Arc::new(Semaphore::new(max_concurrent)),
        }
    }

    pub async fn ensure(&self, req: WorktreeRequest) -> Result<Worktree> {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| GitError("error: git pool closed".into()))?;
        tokio::task::spawn_blocking(move || ensure_worktree(&req))
            .await
            .map_err(|e| GitError(format!("error: git task failed: {e}")))?
    }
}

/// Creates or reuses the worktree. Blocking.
pub fn ensure_worktree(req: &WorktreeRequest) -> Result<Worktree> {
    let repo = &req.repo_path;
    let path = repo.join(WORKTREE_DIR).join(&req.name);
    ensure_excluded(repo)?;

    let listed = list_worktrees(repo)?;

    // 1. the task's own directory
    if path.exists() {
        let found = listed.iter().find(|w| same_path(&w.path, &path));
        let Some(found) = found else {
            return Err(GitError(format!(
                "error: {} exists but is not a worktree of {}",
                path.display(),
                repo.display()
            )));
        };
        return Ok(reuse(found, Origin::ReusedDir));
    }

    // 2. any checkout of the branch
    let full_ref = format!("refs/heads/{}", req.branch);
    if let Some(found) = listed
        .iter()
        .find(|w| w.branch.as_deref() == Some(full_ref.as_str()) && w.path.exists())
    {
        return Ok(reuse(found, Origin::ReusedBranch));
    }

    // 3. add
    std::fs::create_dir_all(repo.join(WORKTREE_DIR))
        .map_err(|e| GitError(format!("error: cannot create {WORKTREE_DIR}: {e}")))?;
    let path_arg = path.to_string_lossy().into_owned();
    let origin = if ref_exists(repo, &full_ref) {
        git(repo, &["worktree", "add", &path_arg, &req.branch])?;
        Origin::CreatedExistingBranch
    } else {
        let start = start_point(repo, &req.base)?;
        git(
            repo,
            &["worktree", "add", "-b", &req.branch, &path_arg, &start],
        )?;
        Origin::CreatedNewBranch
    };
    Ok(Worktree {
        path,
        branch: Some(req.branch.clone()),
        origin,
        dirty: false,
    })
}

fn reuse(found: &Listed, origin: Origin) -> Worktree {
    Worktree {
        path: found.path.clone(),
        branch: found
            .branch
            .as_deref()
            .map(|b| b.strip_prefix("refs/heads/").unwrap_or(b).to_string()),
        origin,
        dirty: is_dirty(&found.path),
    }
}

/// `origin/<base>` after a best-effort fetch, else the local `<base>`.
fn start_point(repo: &Path, base: &str) -> Result<String> {
    // Offline or without an origin this fails, and the last fetched state is used.
    let _ = git(repo, &["fetch", "--quiet", "origin", base]);
    let remote = format!("origin/{base}");
    if ref_exists(repo, &format!("refs/remotes/{remote}")) {
        Ok(remote)
    } else if ref_exists(repo, &format!("refs/heads/{base}")) {
        Ok(base.to_string())
    } else {
        Err(GitError(format!(
            "error: base branch {base} not found (tried {remote} and {base}) in {}",
            repo.display()
        )))
    }
}

/// Adds `.worktrees/` to `.git/info/exclude` unless it is there.
pub fn ensure_excluded(repo: &Path) -> Result<()> {
    let rel = git(repo, &["rev-parse", "--git-path", "info/exclude"])?;
    let rel = PathBuf::from(rel.trim());
    let file = if rel.is_absolute() {
        rel
    } else {
        repo.join(rel)
    };
    let line = format!("{WORKTREE_DIR}/");
    let old = std::fs::read_to_string(&file).unwrap_or_default();
    if old
        .lines()
        .any(|l| l.trim() == line || l.trim() == format!("/{line}"))
    {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(io_err(&file))?;
    }
    let mut new = old;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(&line);
    new.push('\n');
    std::fs::write(&file, new).map_err(io_err(&file))
}

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> GitError + '_ {
    move |e| GitError(format!("error: {}: {e}", path.display()))
}

struct Listed {
    path: PathBuf,
    /// Full ref name, e.g. `refs/heads/claude/T-0003`.
    branch: Option<String>,
}

fn list_worktrees(repo: &Path) -> Result<Vec<Listed>> {
    let out = git(repo, &["worktree", "list", "--porcelain"])?;
    let mut list = Vec::new();
    for block in out.split("\n\n") {
        let mut path = None;
        let mut branch = None;
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(b) = line.strip_prefix("branch ") {
                branch = Some(b.to_string());
            }
        }
        if let Some(path) = path {
            list.push(Listed { path, branch });
        }
    }
    Ok(list)
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn ref_exists(repo: &Path, full_ref: &str) -> bool {
    git(repo, &["rev-parse", "--verify", "--quiet", full_ref]).is_ok()
}

fn is_dirty(dir: &Path) -> bool {
    git(dir, &["status", "--porcelain"]).is_ok_and(|s| !s.trim().is_empty())
}

fn run(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // Never wait for credentials or an editor.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .output()
}

/// Runs `git -C dir args`; stdout on success, an `error:` message otherwise.
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = run(dir, args).map_err(|e| GitError(format!("error: cannot run git: {e}")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(GitError(format!(
            "error: git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        )))
    }
}

#[cfg(test)]
mod tests;
