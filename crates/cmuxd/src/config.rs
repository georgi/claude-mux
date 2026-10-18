//! `config.toml` and the paths derived from it (architecture 5.4 and 9).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const DEFAULT_TCP: &str = "127.0.0.1:7878";

/// What `config.toml` may say. Every field is optional.
#[derive(Debug, Clone, Default, Deserialize)]
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
}

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    pub cert: PathBuf,
    pub key: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
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
