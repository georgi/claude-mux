use std::path::{Path, PathBuf};
use std::process::Command;

use super::*;

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
}

/// A clone of a local bare origin with one commit on `main`, pushed.
fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let origin = root.join("origin.git");
    let repo = root.join("repo");
    std::fs::create_dir_all(&origin).unwrap();
    sh(&origin, &["init", "--bare", "--initial-branch=main"]);
    std::fs::create_dir_all(&repo).unwrap();
    sh(&repo, &["init", "--initial-branch=main"]);
    sh(
        &repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    sh(&repo, &["add", "."]);
    sh(&repo, &["commit", "-m", "first"]);
    sh(&repo, &["push", "-u", "origin", "main"]);
    Fixture { _tmp: tmp, repo }
}

fn req(f: &Fixture) -> WorktreeRequest {
    WorktreeRequest {
        repo_path: f.repo.clone(),
        name: "T-0003".into(),
        base: "main".into(),
        branch: "claude/T-0003".into(),
    }
}

fn exclude(f: &Fixture) -> String {
    std::fs::read_to_string(f.repo.join(".git/info/exclude")).unwrap()
}

#[test]
fn creates_a_worktree_on_a_new_branch_from_origin() {
    let f = fixture();
    // Origin moves ahead of the local main; the worktree must start at origin/main.
    let other = f.repo.parent().unwrap().join("other");
    sh(
        f.repo.parent().unwrap(),
        &["clone", "origin.git", other.to_str().unwrap()],
    );
    std::fs::write(other.join("b.txt"), "b\n").unwrap();
    sh(&other, &["add", "."]);
    sh(&other, &["commit", "-m", "second"]);
    sh(&other, &["push", "origin", "main"]);

    let wt = ensure_worktree(&req(&f)).unwrap();
    assert_eq!(wt.origin, Origin::CreatedNewBranch);
    assert_eq!(wt.path, f.repo.join(".worktrees/T-0003"));
    assert_eq!(wt.branch.as_deref(), Some("claude/T-0003"));
    assert!(!wt.dirty);
    assert!(wt.path.join("a.txt").exists());
    assert!(wt.path.join("b.txt").exists(), "fetched origin/main");
    assert_eq!(
        sh(&wt.path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "claude/T-0003"
    );
    // The main checkout stays clean because of the exclude.
    assert_eq!(sh(&f.repo, &["status", "--porcelain"]).trim(), "");
}

#[test]
fn writes_the_exclude_once() {
    let f = fixture();
    ensure_worktree(&req(&f)).unwrap();
    ensure_worktree(&req(&f)).unwrap();
    assert_eq!(exclude(&f).matches(".worktrees/").count(), 1);
}

#[test]
fn exclude_keeps_existing_lines_and_adds_a_missing_newline() {
    let f = fixture();
    let file = f.repo.join(".git/info/exclude");
    std::fs::write(&file, "*.log").unwrap();
    ensure_excluded(&f.repo).unwrap();
    assert_eq!(exclude(&f), "*.log\n.worktrees/\n");
}

#[test]
fn exclude_is_created_when_info_is_missing() {
    let f = fixture();
    std::fs::remove_dir_all(f.repo.join(".git/info")).unwrap();
    ensure_excluded(&f.repo).unwrap();
    assert_eq!(exclude(&f), ".worktrees/\n");
}

#[test]
fn reuses_the_task_directory() {
    let f = fixture();
    let first = ensure_worktree(&req(&f)).unwrap();
    let second = ensure_worktree(&req(&f)).unwrap();
    assert_eq!(second.origin, Origin::ReusedDir);
    assert_eq!(second.path, first.path);
    assert_eq!(second.branch.as_deref(), Some("claude/T-0003"));
}

#[test]
fn dirty_reuse_keeps_the_changes_and_reports_them() {
    let f = fixture();
    let wt = ensure_worktree(&req(&f)).unwrap();
    std::fs::write(wt.path.join("a.txt"), "changed\n").unwrap();
    std::fs::write(wt.path.join("new.txt"), "new\n").unwrap();
    let again = ensure_worktree(&req(&f)).unwrap();
    assert_eq!(again.origin, Origin::ReusedDir);
    assert!(again.dirty);
    assert_eq!(
        std::fs::read_to_string(wt.path.join("a.txt")).unwrap(),
        "changed\n"
    );
    assert!(wt.path.join("new.txt").exists());
}

#[test]
fn reuses_a_checkout_of_the_branch_elsewhere() {
    let f = fixture();
    let elsewhere = f.repo.parent().unwrap().join("elsewhere");
    sh(
        &f.repo,
        &[
            "worktree",
            "add",
            "-b",
            "claude/T-0003",
            elsewhere.to_str().unwrap(),
            "origin/main",
        ],
    );
    let wt = ensure_worktree(&req(&f)).unwrap();
    assert_eq!(wt.origin, Origin::ReusedBranch);
    assert_eq!(wt.path, elsewhere);
    assert_eq!(wt.branch.as_deref(), Some("claude/T-0003"));
    assert!(!f.repo.join(".worktrees/T-0003").exists());
}

#[test]
fn existing_branch_is_added_without_dash_b() {
    let f = fixture();
    sh(&f.repo, &["branch", "claude/T-0003", "origin/main"]);
    std::fs::write(f.repo.join("a.txt"), "ahead\n").unwrap();
    sh(&f.repo, &["commit", "-am", "ahead"]);
    sh(&f.repo, &["branch", "-f", "claude/T-0003", "HEAD"]);

    let wt = ensure_worktree(&req(&f)).unwrap();
    assert_eq!(wt.origin, Origin::CreatedExistingBranch);
    assert_eq!(
        std::fs::read_to_string(wt.path.join("a.txt")).unwrap(),
        "ahead\n",
        "the existing branch is kept, not reset to the base"
    );
}

#[test]
fn a_removed_worktree_directory_is_recreated_on_its_branch() {
    let f = fixture();
    let wt = ensure_worktree(&req(&f)).unwrap();
    std::fs::remove_dir_all(&wt.path).unwrap();
    sh(&f.repo, &["worktree", "prune"]);
    let again = ensure_worktree(&req(&f)).unwrap();
    assert_eq!(again.origin, Origin::CreatedExistingBranch);
    assert!(again.path.join("a.txt").exists());
}

#[test]
fn falls_back_to_the_local_base_without_origin() {
    let f = fixture();
    sh(&f.repo, &["remote", "remove", "origin"]);
    let wt = ensure_worktree(&req(&f)).unwrap();
    assert_eq!(wt.origin, Origin::CreatedNewBranch);
}

#[test]
fn unknown_base_is_an_error_naming_it() {
    let f = fixture();
    let mut r = req(&f);
    r.base = "nope".into();
    let err = ensure_worktree(&r).unwrap_err();
    assert!(
        err.0.starts_with("error: base branch nope not found"),
        "{err}"
    );
}

#[test]
fn a_stray_directory_is_not_clobbered() {
    let f = fixture();
    let dir = f.repo.join(".worktrees/T-0003");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keep"), "x").unwrap();
    let err = ensure_worktree(&req(&f)).unwrap_err();
    assert!(err.0.contains("is not a worktree"), "{err}");
    assert!(dir.join("keep").exists());
}

#[test]
fn a_non_repo_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let r = WorktreeRequest {
        repo_path: tmp.path().to_path_buf(),
        name: "T-0001".into(),
        base: "main".into(),
        branch: "claude/T-0001".into(),
    };
    assert!(ensure_worktree(&r).unwrap_err().0.starts_with("error:"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pool_runs_requests_and_returns_its_permits() {
    let pool = WorktreePool::default();
    let fixtures: Vec<Fixture> = (0..5).map(|_| fixture()).collect();
    let mut tasks = Vec::new();
    for f in &fixtures {
        let pool = pool.clone();
        let r = req(f);
        tasks.push(tokio::spawn(async move { pool.ensure(r).await }));
    }
    for t in tasks {
        let wt = t.await.unwrap().unwrap();
        assert_eq!(wt.origin, Origin::CreatedNewBranch);
    }
    assert_eq!(pool.permits.available_permits(), MAX_CONCURRENT);
}
