//! `config.toml` and the paths derived from it (architecture 5.4 and 9).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use cmux_core::model::{GlobalLimits, Limits, Timeouts};
use serde::{Deserialize, Serialize};

pub const DEFAULT_TCP: &str = "127.0.0.1:7878";

/// What `config.toml` may say. Every field is optional; an unknown key is an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Where plans, devices and diagnostics live. Default `~/.local/share/cmuxd`.
    pub data_dir: Option<PathBuf>,
    /// The Unix socket. Default `$XDG_RUNTIME_DIR/cmuxd/api.sock`.
    pub socket: Option<PathBuf>,
    /// TCP listen address. Default `127.0.0.1:7878`; `"off"` disables TCP.
    pub listen: Option<String>,
    /// Serve plain HTTP on a non-loopback address.
    pub allow_plain_http: bool,
    /// Shown in `GET /status` and used for pairing URLs when `public_url` is unset.
    pub name: Option<String>,
    /// The URL clients should use, as put in the pairing QR code.
    pub public_url: Option<String>,
    pub tls: Option<TlsConfig>,
    pub pairing: PairingConfig,
    pub agents: AgentsConfig,
    /// After a restart, resume the agents that were working (default on).
    pub auto_resume: bool,
    pub limits: LimitsConfig,
    pub timeouts: TimeoutsConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            data_dir: None,
            socket: None,
            listen: None,
            allow_plain_http: false,
            name: None,
            public_url: None,
            tls: None,
            pairing: PairingConfig::default(),
            agents: AgentsConfig::default(),
            auto_resume: true,
            limits: LimitsConfig::default(),
            timeouts: TimeoutsConfig::default(),
        }
    }
}

/// `[limits]` (architecture section 10). Plan limits apply to new plans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    pub tasks_per_plan: u32,
    pub subagents_per_task: u32,
    /// Claude processes on the whole server.
    pub live_processes: u32,
    /// Concurrent git operations (worktree creation).
    pub git_ops: u32,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        let (p, g) = (Limits::default(), GlobalLimits::default());
        LimitsConfig {
            tasks_per_plan: p.tasks_per_plan,
            subagents_per_task: p.subagents_per_task,
            live_processes: g.live_processes,
            git_ops: g.git_ops,
        }
    }
}

impl LimitsConfig {
    pub fn plan(&self) -> Limits {
        Limits {
            tasks_per_plan: self.tasks_per_plan.max(1),
            subagents_per_task: self.subagents_per_task,
        }
    }

    pub fn global(&self) -> GlobalLimits {
        GlobalLimits {
            live_processes: self.live_processes.max(1),
            git_ops: self.git_ops.max(1),
        }
    }
}

/// `[timeouts]`, in seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimeoutsConfig {
    /// `ask_user` and `ask_parent` questions expire after this.
    pub question_secs: u64,
    /// How long a `PermissionRequest` hook is held open for a remote answer.
    /// Unset means `[agents] permission_wait_secs`.
    pub permission_wait_secs: Option<u64>,
    /// A working agent with no progress for this long is stalled.
    pub stall_secs: u64,
    /// A task or sub-agent idle this long without a report needs a human.
    pub idle_without_report_secs: u64,
}

impl Default for TimeoutsConfig {
    fn default() -> Self {
        let t = Timeouts::default();
        TimeoutsConfig {
            question_secs: t.question / 1000,
            permission_wait_secs: None,
            stall_secs: t.stall / 1000,
            idle_without_report_secs: t.idle_without_report / 1000,
        }
    }
}

/// `[agents]`: how `claude` is started (architecture 7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsConfig {
    /// The `claude` binary (default `claude`, found on `PATH`).
    pub claude_bin: String,
    /// How long a `PermissionRequest` hook is held open for a remote answer.
    /// `[timeouts] permission_wait_secs` wins when set.
    pub permission_wait_secs: u64,
    /// Mark an agent's working directory trusted in Claude's global config
    /// (`$CLAUDE_CONFIG_DIR/.claude.json` or `~/.claude.json`) before starting
    /// it, so the first start does not wait on the "trust this folder" dialog.
    pub pre_trust: bool,
    pub executor: LevelConfig,
    pub task: LevelConfig,
    pub sub: LevelConfig,
}

impl Default for AgentsConfig {
    fn default() -> Self {
        AgentsConfig {
            claude_bin: "claude".into(),
            permission_wait_secs: 600,
            pre_trust: true,
            executor: LevelConfig::default(),
            task: LevelConfig::default(),
            sub: LevelConfig::default(),
        }
    }
}

/// `claude` options for one level. An absent `permission_mode` means `auto`
/// (not the Claude account's own default); an absent `model` means Claude's own.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LevelConfig {
    pub permission_mode: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PairingConfig {
    /// How long a code is valid.
    pub ttl_secs: u64,
    /// Attempts per minute on `/pair/complete`, across all callers.
    pub attempts_per_min: u32,
}

impl Default for PairingConfig {
    fn default() -> Self {
        PairingConfig {
            ttl_secs: 300,
            attempts_per_min: 5,
        }
    }
}

/// Command-line and environment overrides.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    pub config: Option<PathBuf>,
    pub data_dir: Option<PathBuf>,
    pub socket: Option<PathBuf>,
    pub listen: Option<String>,
}

#[derive(Debug)]
pub enum ConfigError {
    Read(PathBuf, std::io::Error),
    Parse(PathBuf, toml::de::Error),
    BadListen(String),
    /// Non-loopback plain HTTP without `allow_plain_http`.
    PlainHttp(SocketAddr),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Read(p, e) => write!(f, "cannot read {}: {e}", p.display()),
            ConfigError::Parse(p, e) => write!(f, "bad config {}: {e}", p.display()),
            ConfigError::BadListen(s) => {
                write!(f, "listen address {s:?} is not an ip:port (or \"off\")")
            }
            ConfigError::PlainHttp(a) => write!(
                f,
                "refusing to serve plain HTTP on non-loopback {a}; use TLS, a reverse proxy \
                 or tailscale serve, or set allow_plain_http = true"
            ),
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// `~/.config/cmuxd/config.toml` (or `$XDG_CONFIG_HOME/cmuxd/config.toml`).
    pub fn default_path() -> PathBuf {
        let base = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home().join(".config"));
        base.join("cmuxd").join("config.toml")
    }

    /// Reads the file (an absent default file is an empty config) and applies
    /// the overrides.
    pub fn load(o: &Overrides) -> Result<Config, ConfigError> {
        let (path, required) = match &o.config {
            Some(p) => (p.clone(), true),
            None => (Config::default_path(), false),
        };
        let mut cfg = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).map_err(|e| ConfigError::Parse(path.clone(), e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !required => Config::default(),
            Err(e) => return Err(ConfigError::Read(path, e)),
        };
        cfg.apply(o);
        Ok(cfg)
    }

    pub fn apply(&mut self, o: &Overrides) {
        if o.data_dir.is_some() {
            self.data_dir = o.data_dir.clone();
        }
        if o.socket.is_some() {
            self.socket = o.socket.clone();
        }
        if o.listen.is_some() {
            self.listen = o.listen.clone();
        }
    }

    /// The permission wait in seconds, at least 1.
    pub fn permission_wait_secs(&self) -> u64 {
        let t = self.timeouts.permission_wait_secs;
        t.unwrap_or(self.agents.permission_wait_secs).max(1)
    }

    /// The engine timers this config asks for.
    pub fn engine_timeouts(&self) -> Timeouts {
        let ms = |s: u64| s.max(1).saturating_mul(1000);
        let t = &self.timeouts;
        Timeouts {
            permission_wait: ms(self.permission_wait_secs()),
            question: ms(t.question_secs),
            idle_without_report: ms(t.idle_without_report_secs),
            stall: ms(t.stall_secs),
            ..Timeouts::default()
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(default_data_dir)
    }

    pub fn socket_path(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(default_socket)
    }

    pub fn server_name(&self) -> String {
        self.name.clone().unwrap_or_else(hostname)
    }

    /// The TCP address to bind, `None` when disabled. Refuses a non-loopback
    /// address over plain HTTP unless allowed.
    pub fn tcp_addr(&self) -> Result<Option<SocketAddr>, ConfigError> {
        let s = self.listen.as_deref().unwrap_or(DEFAULT_TCP);
        if s.eq_ignore_ascii_case("off") {
            return Ok(None);
        }
        let addr: SocketAddr = s.parse().map_err(|_| ConfigError::BadListen(s.into()))?;
        check_plain_http(addr, self.allow_plain_http || self.tls.is_some())?;
        Ok(Some(addr))
    }

    /// The base URL a pairing code is valid at.
    pub fn base_url(&self, bound: Option<SocketAddr>) -> String {
        if let Some(u) = &self.public_url {
            return u.trim_end_matches('/').to_string();
        }
        let scheme = if self.tls.is_some() { "https" } else { "http" };
        match bound {
            Some(a) => format!("{scheme}://{a}"),
            None => format!("{scheme}://{DEFAULT_TCP}"),
        }
    }
}

/// Plain HTTP is only for loopback unless the operator says otherwise.
pub fn check_plain_http(addr: SocketAddr, allowed: bool) -> Result<(), ConfigError> {
    if addr.ip().is_loopback() || allowed {
        Ok(())
    } else {
        Err(ConfigError::PlainHttp(addr))
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

pub fn default_data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local").join("share"));
    base.join("cmuxd")
}

pub fn default_socket() -> PathBuf {
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(d) if d.is_absolute() => d.join("cmuxd"),
        // macOS has no runtime dir; keep the per-user directory short because
        // socket paths are limited to about 100 bytes.
        _ => std::env::temp_dir().join(format!("cmuxd-{}", unsafe { libc::geteuid() })),
    };
    dir.join("api.sock")
}

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for `buf.len()` bytes; gethostname NUL-terminates or truncates.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    if rc == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        let s = String::from_utf8_lossy(&buf[..end]).into_owned();
        if !s.is_empty() {
            return s;
        }
    }
    "cmuxd".into()
}

/// Creates `dir` (and parents) with mode 0700 if it does not exist.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_overrides() {
        let mut c: Config = toml::from_str("").unwrap();
        assert_eq!(
            c.tcp_addr().unwrap(),
            Some("127.0.0.1:7878".parse().unwrap())
        );
        c.apply(&Overrides {
            data_dir: Some("/x".into()),
            listen: Some("off".into()),
            ..Default::default()
        });
        assert_eq!(c.data_dir(), PathBuf::from("/x"));
        assert_eq!(c.tcp_addr().unwrap(), None);
    }

    #[test]
    fn agents_section() {
        let c: Config = toml::from_str(
            "[agents]\nclaude_bin = \"/x/claude\"\n[agents.task]\nmodel = \"haiku\"",
        )
        .unwrap();
        assert_eq!(c.agents.claude_bin, "/x/claude");
        assert_eq!(c.agents.permission_wait_secs, 600);
        assert!(c.agents.pre_trust, "on by default");
        let off: Config = toml::from_str("[agents]\npre_trust = false").unwrap();
        assert!(!off.agents.pre_trust);
        assert_eq!(c.agents.task.model.as_deref(), Some("haiku"));
        assert!(c.agents.sub.model.is_none());
    }

    const FULL: &str = r#"
auto_resume = false

[limits]
tasks_per_plan = 3
subagents_per_task = 4
live_processes = 12
git_ops = 1

[timeouts]
question_secs = 1800
permission_wait_secs = 120
stall_secs = 600
idle_without_report_secs = 300
"#;

    #[test]
    fn limits_timeouts_and_auto_resume_round_trip() {
        let c: Config = toml::from_str(FULL).unwrap();
        assert!(!c.auto_resume);
        assert_eq!(
            c.limits,
            LimitsConfig {
                tasks_per_plan: 3,
                subagents_per_task: 4,
                live_processes: 12,
                git_ops: 1,
            }
        );
        assert_eq!(
            c.timeouts,
            TimeoutsConfig {
                question_secs: 1800,
                permission_wait_secs: Some(120),
                stall_secs: 600,
                idle_without_report_secs: 300,
            }
        );
        let again: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(again, c);
        // What the engine gets.
        let t = c.engine_timeouts();
        assert_eq!(
            (
                t.question,
                t.permission_wait,
                t.stall,
                t.idle_without_report
            ),
            (1_800_000, 120_000, 600_000, 300_000)
        );
        assert_eq!(t.steer, Timeouts::default().steer);
        assert_eq!(c.limits.plan().tasks_per_plan, 3);
        assert_eq!(c.limits.plan().subagents_per_task, 4);
        assert_eq!(c.limits.global().live_processes, 12);
        assert_eq!(c.limits.global().git_ops, 1);
    }

    #[test]
    fn new_keys_default_to_the_engine_defaults() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c, Config::default());
        assert!(c.auto_resume, "on by default");
        assert_eq!(c.engine_timeouts(), Timeouts::default());
        assert_eq!(c.limits.plan(), Limits::default());
        assert_eq!(c.limits.global(), GlobalLimits::default());
        assert_eq!(
            Timeouts::default().stall,
            15 * 60_000,
            "the stall default is 15 min"
        );
        // An empty config round-trips too.
        let again: Config = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(again, c);
        // `[agents] permission_wait_secs` still works; `[timeouts]` wins.
        let a: Config = toml::from_str("[agents]\npermission_wait_secs = 77").unwrap();
        assert_eq!(a.engine_timeouts().permission_wait, 77_000);
        let both: Config = toml::from_str(
            "[agents]\npermission_wait_secs = 77\n[timeouts]\npermission_wait_secs = 5",
        )
        .unwrap();
        assert_eq!(both.permission_wait_secs(), 5);
    }

    #[test]
    fn an_unknown_key_fails_to_load() {
        for bad in [
            "auto_resum = true",
            "[limits]\ntasks = 3",
            "[timeouts]\nstall = 600",
            "[timeouts]\nstall_mins = 10",
            "[agents]\nbogus = 1",
            "[pairing]\nttl = 1",
            "[tls]\ncert = \"c\"\nkey = \"k\"\nca = \"x\"",
        ] {
            assert!(toml::from_str::<Config>(bad).is_err(), "{bad:?} loaded");
        }
        // And through `Config::load`, as the server reads it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[limits]\nlive = 3\n").unwrap();
        let o = Overrides {
            config: Some(path.clone()),
            ..Overrides::default()
        };
        let e = Config::load(&o).unwrap_err();
        assert!(matches!(e, ConfigError::Parse(..)), "{e}");
        assert!(e.to_string().contains("live"), "{e}");
        std::fs::write(&path, FULL).unwrap();
        assert!(!Config::load(&o).unwrap().auto_resume);
    }

    #[test]
    fn the_readme_example_loads_with_the_defaults() {
        let readme = include_str!("../../../README.md");
        let start = readme.find("```toml\nlisten").expect("example") + "```toml\n".len();
        let text = &readme[start..start + readme[start..].find("```").unwrap()];
        let c: Config = toml::from_str(text).unwrap();
        assert_eq!(c.limits, LimitsConfig::default());
        assert_eq!(c.timeouts.stall_secs, TimeoutsConfig::default().stall_secs);
        assert!(c.auto_resume);
    }

    #[test]
    fn plain_http_needs_loopback_or_opt_in() {
        let mut c: Config = toml::from_str(r#"listen = "0.0.0.0:7878""#).unwrap();
        assert!(matches!(c.tcp_addr(), Err(ConfigError::PlainHttp(_))));
        c.allow_plain_http = true;
        assert!(c.tcp_addr().unwrap().is_some());
        let c: Config = toml::from_str(r#"listen = "[::1]:1""#).unwrap();
        assert!(c.tcp_addr().unwrap().is_some());
        let c: Config = toml::from_str(r#"listen = "nonsense""#).unwrap();
        assert!(matches!(c.tcp_addr(), Err(ConfigError::BadListen(_))));
        assert!(toml::from_str::<Config>("bogus = 1").is_err());
    }
}
