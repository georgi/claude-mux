use serde_json::{Value, json};

use super::*;

fn read(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let repo = t.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let repo = std::fs::canonicalize(repo).unwrap();
    let file = t.path().join(".claude.json");
    (t, file, repo)
}

#[test]
fn the_config_dir_wins_over_home() {
    let home = Path::new("/home/u");
    assert_eq!(
        claude_json_path(Some(&"/cfg".into()), home),
        Path::new("/cfg/.claude.json")
    );
    assert_eq!(
        claude_json_path(None, home),
        Path::new("/home/u/.claude.json")
    );
    assert_eq!(
        claude_json_path(Some(&"".into()), home),
        Path::new("/home/u/.claude.json")
    );
}

#[test]
fn a_missing_file_is_created() {
    let (_t, file, repo) = setup();
    assert!(trust_dir(&file, &repo).unwrap());
    let v = read(&file);
    assert_eq!(
        v["projects"][repo.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn other_keys_are_kept_and_the_mode_is_preserved() {
    let (_t, file, repo) = setup();
    let key = repo.to_str().unwrap();
    let before = json!({
        "numStartups": 12,
        "oauthAccount": {"emailAddress": "a@b.c"},
        "projects": {
            "/elsewhere": {"hasTrustDialogAccepted": false, "allowedTools": ["x"]},
            key: {"allowedTools": ["Bash"], "hasTrustDialogAccepted": false, "n": 1.5},
        },
    });
    std::fs::write(&file, before.to_string()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    assert!(trust_dir(&file, &repo).unwrap());
    let mut want = before;
    want["projects"][key]["hasTrustDialogAccepted"] = json!(true);
    assert_eq!(read(&file), want);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);
    }
    // No temp file is left behind.
    let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".tmp")), "{names:?}");
}

#[test]
fn an_already_trusted_dir_is_not_rewritten() {
    let (_t, file, repo) = setup();
    let key = repo.to_str().unwrap();
    // Odd formatting would change on a rewrite.
    let text = format!("{{ \"projects\": {{ {key:?}: {{\"hasTrustDialogAccepted\":true}} }} }}");
    std::fs::write(&file, &text).unwrap();
    assert!(!trust_dir(&file, &repo).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
}

#[test]
fn a_malformed_file_is_an_error_and_untouched() {
    let (_t, file, repo) = setup();
    for text in ["{not json", "[1,2]", r#"{"projects": 3}"#] {
        std::fs::write(&file, text).unwrap();
        assert!(trust_dir(&file, &repo).is_err(), "{text}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    }
}

#[cfg(unix)]
#[test]
fn a_symlinked_config_stays_a_symlink() {
    let (t, file, repo) = setup();
    let real = t.path().join("real.json");
    std::fs::write(&real, "{}").unwrap();
    std::os::unix::fs::symlink(&real, &file).unwrap();
    assert!(trust_dir(&file, &repo).unwrap());
    assert!(std::fs::symlink_metadata(&file).unwrap().is_symlink());
    assert_eq!(
        read(&real)["projects"][repo.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
}

#[test]
fn pre_trust_follows_the_childs_config_dir_and_survives_failure() {
    use crate::claude::spawn::{SpawnConfig, SpawnRequest, build_spawn};
    use cmux_core::ids::{AgentId, AgentPath, PlanId};
    use cmux_core::model::Level;

    let (t, _file, repo) = setup();
    let agent = AgentId::new(PlanId::new(2026, 10, 18, "x"), AgentPath::Executor);
    let cfg = SpawnConfig::new("/bin/cmuxd".into(), "/s".into(), t.path().join("data"));
    let req = SpawnRequest {
        agent: &agent,
        level: Level::Executor,
        cwd: &repo,
        session_id: None,
        resume: false,
        system_prompt: "",
        kickoff: "go",
    };
    let spec = build_spawn(&cfg, &req);
    let key = repo.to_str().unwrap();
    let env = |dir: &Path| vec![("CLAUDE_CONFIG_DIR".to_string(), dir.display().to_string())];

    // With CLAUDE_CONFIG_DIR: that dir's file, not home's.
    let (cdir, home) = (t.path().join("cfg"), t.path().join("home"));
    pre_trust(&spec, env(&cdir), &home);
    assert_eq!(
        read(&cdir.join(".claude.json"))["projects"][key]["hasTrustDialogAccepted"],
        true
    );
    assert!(!home.join(".claude.json").exists());

    // Without it: ~/.claude.json.
    pre_trust(&spec, Vec::new(), &home);
    assert_eq!(
        read(&home.join(".claude.json"))["projects"][key]["hasTrustDialogAccepted"],
        true
    );

    // A broken file is logged, left alone and does not panic.
    let bad = t.path().join("bad");
    std::fs::create_dir(&bad).unwrap();
    std::fs::write(bad.join(".claude.json"), "{oops").unwrap();
    pre_trust(&spec, env(&bad), &home);
    assert_eq!(
        std::fs::read_to_string(bad.join(".claude.json")).unwrap(),
        "{oops"
    );
}

#[test]
fn parallel_updates_are_all_kept() {
    let (t, file, _repo) = setup();
    let dirs: Vec<PathBuf> = (0..16)
        .map(|i| {
            let d = t.path().join(format!("w{i}"));
            std::fs::create_dir(&d).unwrap();
            std::fs::canonicalize(d).unwrap()
        })
        .collect();
    std::thread::scope(|s| {
        for d in &dirs {
            let file = &file;
            s.spawn(move || trust_dir(file, d).unwrap());
        }
    });
    let v = read(&file);
    for d in &dirs {
        assert_eq!(
            v["projects"][d.to_str().unwrap()]["hasTrustDialogAccepted"],
            true
        );
    }
    assert!(!t.path().join(".claude.json.lock").exists());
}

#[test]
fn claudes_lock_is_waited_for_and_a_stale_one_is_broken() {
    let (t, file, repo) = setup();
    let lock = t.path().join(".claude.json.lock");
    // A live lock: the write waits until "claude" lets go.
    std::fs::create_dir(&lock).unwrap();
    let released = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        s.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(200));
            std::fs::write(&file, r#"{"numStartups": 7}"#).unwrap();
            released.store(true, std::sync::atomic::Ordering::SeqCst);
            std::fs::remove_dir(&lock).unwrap();
        });
        assert!(trust_dir(&file, &repo).unwrap());
        assert!(released.load(std::sync::atomic::Ordering::SeqCst));
    });
    let v = read(&file);
    assert_eq!(v["numStartups"], 7, "claude's write under the lock is kept");
    assert_eq!(
        v["projects"][repo.to_str().unwrap()]["hasTrustDialogAccepted"],
        true
    );
    assert!(!lock.exists());

    // A stale lock (mtime older than proper-lockfile's 10 s) is broken.
    std::fs::create_dir(&lock).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    std::fs::File::open(&lock)
        .unwrap()
        .set_modified(old)
        .unwrap();
    std::fs::write(&file, "{}").unwrap();
    let start = std::time::Instant::now();
    assert!(trust_dir(&file, &repo).unwrap());
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
    assert!(!lock.exists());
}
