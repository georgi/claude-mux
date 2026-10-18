//! `cmuxd setup`: installs cmuxd as a systemd user service (architecture 5.5).
//!
//! It writes `~/.config/systemd/user/cmuxd.service`, enables and starts the
//! unit, turns on linger so the server survives logout, and checks that
//! `claude` and `gh` are installed and logged in. Every step is idempotent, so
//! it is safe to run twice. `--dry-run` writes and runs nothing that changes
//! the system; the read-only checks still run.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The result of one external command.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    /// The first non-empty line of stdout, else of stderr.
    pub fn first_line(&self) -> &str {
        self.stdout
            .lines()
            .chain(self.stderr.lines())
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
    }
}

/// Runs external commands. Tests swap in a fake.
pub trait Exec {
    /// `Err` when the program cannot be started (not found, say).
    fn run(&self, program: &str, args: &[&str]) -> Result<Output, String>;
}

/// Runs real processes.
pub struct System;

impl Exec for System {
    fn run(&self, program: &str, args: &[&str]) -> Result<Output, String> {
        let out = Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("cannot run {program}: {e}"))?;
        Ok(Output {
            ok: out.status.success(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

pub struct SetupOpts {
    pub home: PathBuf,
    /// The `cmuxd` binary the unit runs.
    pub exe: PathBuf,
    /// `--config` to pass to `serve`, when not the default.
    pub config: Option<PathBuf>,
    /// The PATH the service gets, so it finds `claude`, `gh` and `git`.
    pub path_env: String,
    pub claude_bin: String,
    pub gh_bin: String,
    pub dry_run: bool,
}

/// `~/.config/systemd/user/cmuxd.service`.
pub fn unit_path(home: &Path) -> PathBuf {
    home.join(".config/systemd/user/cmuxd.service")
}

/// The unit file text.
pub fn unit_file(exe: &Path, config: Option<&Path>, path_env: &str) -> String {
    let mut exec = quote(&exe.to_string_lossy());
    if let Some(c) = config {
        exec.push_str(" --config ");
        exec.push_str(&quote(&c.to_string_lossy()));
    }
    format!(
        "\
# Written by `cmuxd setup`. Run it again after you move the binary.
[Unit]
Description=cmux server

[Service]
Type=simple
ExecStart={exec} serve
Restart=on-failure
RestartSec=2
Environment={env}

[Install]
WantedBy=default.target
",
        env = quote(&format!("PATH={path_env}")),
    )
}

/// systemd quoting: double quotes when needed, with `\` and `"` escaped. A
/// `%` is a specifier in unit files and is doubled.
fn quote(s: &str) -> String {
    let s = s.replace('%', "%%");
    if s.chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\\')
    {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        s
    }
}

/// What a step did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    Ok,
    Skip,
    Fail,
}

/// A transcript of the steps.
#[derive(Default)]
pub struct Report {
    pub lines: Vec<(Mark, String)>,
}

impl Report {
    pub fn push(&mut self, mark: Mark, text: impl Into<String>) {
        self.lines.push((mark, text.into()));
    }

    pub fn failed(&self) -> usize {
        self.lines.iter().filter(|(m, _)| *m == Mark::Fail).count()
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for (mark, text) in &self.lines {
            let tag = match mark {
                Mark::Ok => "ok  ",
                Mark::Skip => "--  ",
                Mark::Fail => "FAIL",
            };
            let _ = writeln!(out, "[{tag}] {text}");
        }
        out
    }
}

/// Runs every step; a failed step does not stop the next one.
pub fn run(o: &SetupOpts, exec: &dyn Exec) -> Report {
    let mut r = Report::default();
    let changed = write_unit(o, &mut r);
    service(o, exec, changed, &mut r);
    check_claude(&o.claude_bin, exec, &mut r);
    check_gh(&o.gh_bin, exec, &mut r);
    r
}

/// Writes the unit when its text differs. Returns whether it changed.
fn write_unit(o: &SetupOpts, r: &mut Report) -> bool {
    let path = unit_path(&o.home);
    let text = unit_file(&o.exe, o.config.as_deref(), &o.path_env);
    match std::fs::read_to_string(&path) {
        Ok(old) if old == text => {
            r.push(Mark::Ok, format!("{} is up to date", path.display()));
            return false;
        }
        _ => {}
    }
    if o.dry_run {
        r.push(Mark::Skip, format!("would write {}", path.display()));
        return true;
    }
    let res = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, &text));
    match res {
        Ok(()) => r.push(Mark::Ok, format!("wrote {}", path.display())),
        Err(e) => r.push(Mark::Fail, format!("cannot write {}: {e}", path.display())),
    }
    true
}

fn service(o: &SetupOpts, exec: &dyn Exec, changed: bool, r: &mut Report) {
    let mut steps: Vec<(&str, Vec<&str>)> = vec![
        ("systemctl", vec!["--user", "daemon-reload"]),
        ("systemctl", vec!["--user", "enable", "--now", "cmuxd"]),
    ];
    // `enable --now` leaves a running service on its old unit.
    if changed {
        steps.push(("systemctl", vec!["--user", "try-restart", "cmuxd"]));
    }
    // Without an argument it applies to the calling user.
    steps.push(("loginctl", vec!["enable-linger"]));
    for (prog, args) in steps {
        let cmd = format!("{prog} {}", args.join(" "));
        if o.dry_run {
            r.push(Mark::Skip, format!("would run `{cmd}`"));
            continue;
        }
        match exec.run(prog, &args) {
            Ok(out) if out.ok => r.push(Mark::Ok, format!("ran `{cmd}`")),
            Ok(out) => r.push(Mark::Fail, format!("`{cmd}` failed: {}", out.first_line())),
            Err(e) => r.push(Mark::Fail, format!("`{cmd}`: {e}")),
        }
    }
}

pub fn check_claude(bin: &str, exec: &dyn Exec, r: &mut Report) {
    match exec.run(bin, &["--version"]) {
        Ok(out) if out.ok => r.push(Mark::Ok, format!("claude {}", out.first_line())),
        Ok(out) => {
            r.push(
                Mark::Fail,
                format!("`{bin} --version` failed: {}", out.first_line()),
            );
            return;
        }
        Err(e) => {
            r.push(Mark::Fail, format!("claude is not installed: {e}"));
            return;
        }
    }
    match exec.run(bin, &["auth", "status"]) {
        Ok(out) => match claude_login(&out.stdout) {
            Some(method) if out.ok => r.push(Mark::Ok, format!("claude is logged in ({method})")),
            _ => r.push(
                Mark::Fail,
                format!("claude is not logged in: run `{bin} auth login` as this user"),
            ),
        },
        Err(e) => r.push(Mark::Fail, format!("`{bin} auth status`: {e}")),
    }
}

/// The auth method from `claude auth status` JSON when logged in.
fn claude_login(stdout: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(stdout.trim()).ok()?;
    if v["loggedIn"].as_bool() != Some(true) {
        return None;
    }
    Some(
        v["authMethod"]
            .as_str()
            .unwrap_or("unknown method")
            .to_string(),
    )
}

pub fn check_gh(bin: &str, exec: &dyn Exec, r: &mut Report) {
    match exec.run(bin, &["auth", "status"]) {
        Ok(out) if out.ok => r.push(Mark::Ok, "gh is logged in"),
        Ok(out) => r.push(
            Mark::Fail,
            format!(
                "gh is not logged in ({}): run `{bin} auth login`",
                out.first_line()
            ),
        ),
        Err(e) => r.push(Mark::Fail, format!("gh is not installed: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Answers by program and first argument; records every call.
    #[derive(Default)]
    pub(crate) struct Fake {
        pub calls: RefCell<Vec<String>>,
    }

    impl Exec for Fake {
        fn run(&self, program: &str, args: &[&str]) -> Result<Output, String> {
            self.calls
                .borrow_mut()
                .push(format!("{program} {}", args.join(" ")));
            let ok = |s: &str| {
                Ok(Output {
                    ok: true,
                    stdout: s.into(),
                    stderr: String::new(),
                })
            };
            match (program, args.first().copied()) {
                ("claude", Some("--version")) => ok("2.1.0 (Claude Code)\n"),
                ("claude", Some("auth")) => ok(r#"{"loggedIn":true,"authMethod":"claude.ai"}"#),
                ("gh", _) => Ok(Output {
                    ok: false,
                    stdout: String::new(),
                    stderr: "You are not logged into any GitHub hosts.\n".into(),
                }),
                _ => ok(""),
            }
        }
    }

    fn opts(home: &Path, dry_run: bool) -> SetupOpts {
        SetupOpts {
            home: home.to_path_buf(),
            exe: "/usr/local/bin/cmuxd".into(),
            config: None,
            path_env: "/usr/local/bin:/usr/bin:/bin".into(),
            claude_bin: "claude".into(),
            gh_bin: "gh".into(),
            dry_run,
        }
    }

    #[test]
    fn unit_file_text() {
        insta::assert_snapshot!(unit_file(
            Path::new("/home/me/.local/bin/cmuxd"),
            None,
            "/home/me/.local/bin:/usr/local/bin:/usr/bin:/bin",
        ));
    }

    #[test]
    fn unit_file_quotes_spaces_and_percent() {
        let t = unit_file(
            Path::new("/opt/my apps/cmuxd"),
            Some(Path::new("/etc/cmux 100%.toml")),
            "/bin",
        );
        assert!(
            t.contains(r#"ExecStart="/opt/my apps/cmuxd" --config "/etc/cmux 100%%.toml" serve"#),
            "{t}"
        );
    }

    #[test]
    fn dry_run_writes_and_changes_nothing() {
        let home = tempfile::tempdir().unwrap();
        let fake = Fake::default();
        let r = run(&opts(home.path(), true), &fake);
        let text = r
            .render()
            .replace(&home.path().display().to_string(), "$HOME");
        insta::assert_snapshot!(text);
        assert!(!unit_path(home.path()).exists());
        assert!(!home.path().join(".config").exists());
        // Only the read-only checks ran.
        assert_eq!(
            *fake.calls.borrow(),
            ["claude --version", "claude auth status", "gh auth status"]
        );
        assert_eq!(r.failed(), 1);
    }

    #[test]
    fn second_run_leaves_the_unit_alone() {
        let home = tempfile::tempdir().unwrap();
        let first = Fake::default();
        run(&opts(home.path(), false), &first);
        let text = std::fs::read_to_string(unit_path(home.path())).unwrap();
        assert!(text.contains("ExecStart=/usr/local/bin/cmuxd serve"));
        assert!(
            first
                .calls
                .borrow()
                .contains(&"systemctl --user try-restart cmuxd".to_string())
        );

        let second = Fake::default();
        let r = run(&opts(home.path(), false), &second);
        assert!(r.render().contains("is up to date"), "{}", r.render());
        let calls = second.calls.borrow();
        assert!(calls.contains(&"systemctl --user enable --now cmuxd".to_string()));
        assert!(calls.contains(&"loginctl enable-linger".to_string()));
        assert!(!calls.iter().any(|c| c.contains("try-restart")));
        // A dry run after a real one reports nothing to write.
        let r = run(&opts(home.path(), true), &Fake::default());
        assert!(!r.render().contains("would write"));
    }

    #[test]
    fn claude_logged_out_is_a_failure() {
        assert_eq!(claude_login(r#"{"loggedIn":false}"#), None);
        assert_eq!(claude_login("not json"), None);
        assert_eq!(
            claude_login(r#"{"loggedIn":true,"authMethod":"api_key"}"#).as_deref(),
            Some("api_key")
        );
    }
}
