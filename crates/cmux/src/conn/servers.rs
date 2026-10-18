//! `~/.config/cmux/client.toml`: the servers this machine is paired with, and
//! which one is the default. Tokens live here, so the file is mode 0600.
//!
//! The local `cmuxd` is not listed: when its Unix socket exists it is used
//! without a token (the server checks the uid).

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// The name `cmux -s` accepts for the local socket.
pub const LOCAL: &str = "local";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    pub name: String,
    pub url: String,
    pub token: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, rename = "server", skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<ServerEntry>,
}

/// Where to reach a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// The local `cmuxd`: HTTP over a Unix socket, no token.
    Unix(PathBuf),
    /// `http(s)://host:port` with a bearer token.
    Remote { url: String, token: String },
}

/// A server picked by [`ClientConfig::resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    pub name: String,
    pub endpoint: Endpoint,
}

impl ClientConfig {
    pub fn load(path: &Path) -> Result<ClientConfig> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                toml::from_str(&s).with_context(|| format!("{}: bad client.toml", path.display()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ClientConfig::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        write_private(path, text.as_bytes())
    }

    pub fn get(&self, name: &str) -> Option<&ServerEntry> {
        self.servers.iter().find(|s| s.name == name)
    }

    /// Adds or replaces a server. The first server becomes the default.
    pub fn add(&mut self, entry: ServerEntry) {
        match self.servers.iter_mut().find(|s| s.name == entry.name) {
            Some(slot) => *slot = entry,
            None => {
                if self.default.is_none() {
                    self.default = Some(entry.name.clone());
                }
                self.servers.push(entry);
            }
        }
    }

    /// Removes a server; the default moves to the first remaining one.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.servers.len();
        self.servers.retain(|s| s.name != name);
        if self.default.as_deref() == Some(name) {
            self.default = self.servers.first().map(|s| s.name.clone());
        }
        self.servers.len() != before
    }

    pub fn set_default(&mut self, name: &str) -> Result<()> {
        if name != LOCAL && self.get(name).is_none() {
            bail!("no server named {name:?} (see `cmux servers`)");
        }
        self.default = Some(name.to_string());
        Ok(())
    }

    /// Picks the server to connect to: the named one, else the default, else
    /// the local socket when it exists, else the only configured server.
    pub fn resolve(&self, name: Option<&str>, socket: &Path) -> Result<Server> {
        let local = || Server {
            name: LOCAL.into(),
            endpoint: Endpoint::Unix(socket.to_path_buf()),
        };
        let remote = |e: &ServerEntry| Server {
            name: e.name.clone(),
            endpoint: Endpoint::Remote {
                url: e.url.clone(),
                token: e.token.clone(),
            },
        };
        if let Some(n) = name.or(self.default.as_deref()) {
            if n == LOCAL {
                return Ok(local());
            }
            return match self.get(n) {
                Some(e) => Ok(remote(e)),
                None => bail!("no server named {n:?} (see `cmux servers`)"),
            };
        }
        if socket.exists() {
            return Ok(local());
        }
        match self.servers.as_slice() {
            [only] => Ok(remote(only)),
            [] => bail!(
                "no server: cmuxd is not running locally ({} is missing) and none is paired; \
                 run `cmux login <url>`",
                socket.display()
            ),
            _ => bail!(
                "several servers are paired; pick one with -s NAME or `cmux servers default NAME`"
            ),
        }
    }
}

/// Whether something listens on the Unix socket `path`.
pub fn socket_alive(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Decides between remote and local mode. `Ok(None)` means run claude
/// locally (claude-mux as before): no `-s`, no default server in
/// `client.toml`, and nothing listening on the local socket. `socket_alive`
/// is injected for tests.
pub fn pick(
    cfg: &ClientConfig,
    name: Option<&str>,
    force_local: bool,
    socket: &Path,
    socket_alive: impl Fn(&Path) -> bool,
) -> Result<Option<Server>> {
    if force_local {
        return Ok(None);
    }
    if name.is_some() || cfg.default.is_some() {
        return cfg.resolve(name, socket).map(Some);
    }
    if socket_alive(socket) {
        return Ok(Some(Server {
            name: LOCAL.into(),
            endpoint: Endpoint::Unix(socket.to_path_buf()),
        }));
    }
    Ok(None)
}

/// Writes `bytes` to `path` with mode 0600, atomically (temp file + rename).
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `$CMUX_CONFIG_DIR`, else `$XDG_CONFIG_HOME/cmux`, else `~/.config/cmux`.
pub fn config_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CMUX_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".config")
        });
    base.join("cmux")
}

pub fn client_toml() -> PathBuf {
    config_dir().join("client.toml")
}

/// The file holding the view state of one server.
pub fn view_path(dir: &Path, server: &str) -> PathBuf {
    let safe: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("view-{safe}.toml"))
}

/// The local server's socket: `$CMUXD_SOCKET`, else the path `cmuxd` defaults
/// to (`$XDG_RUNTIME_DIR/cmuxd/api.sock`, or a per-uid temp directory).
pub fn local_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("CMUXD_SOCKET").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        Some(d) if d.is_absolute() => d.join("cmuxd"),
        _ => std::env::temp_dir().join(format!("cmuxd-{}", unsafe { libc::geteuid() })),
    };
    dir.join("api.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_chooses_remote_or_local_mode() {
        let sock = Path::new("/run/cmuxd/api.sock");
        let mut cfg = ClientConfig::default();
        // Nothing configured, no daemon: today's local behaviour.
        assert_eq!(pick(&cfg, None, false, sock, |_| false).unwrap(), None);
        // A live local socket is used without any pairing.
        let s = pick(&cfg, None, false, sock, |_| true).unwrap().unwrap();
        assert_eq!(s.name, LOCAL);
        // --local wins over everything.
        assert_eq!(pick(&cfg, None, true, sock, |_| true).unwrap(), None);
        // -s must name a known server.
        assert!(pick(&cfg, Some("vps"), false, sock, |_| false).is_err());
        cfg.add(ServerEntry {
            name: "vps".into(),
            url: "https://vps".into(),
            token: "t".into(),
        });
        let s = pick(&cfg, Some("vps"), false, sock, |_| false)
            .unwrap()
            .unwrap();
        assert_eq!(s.name, "vps");
        // The first paired server became the default.
        let s = pick(&cfg, None, false, sock, |_| false).unwrap().unwrap();
        assert_eq!(s.name, "vps");
        // -s local reaches the socket even with a different default.
        let s = pick(&cfg, Some(LOCAL), false, sock, |_| false)
            .unwrap()
            .unwrap();
        assert_eq!(s.endpoint, Endpoint::Unix(sock.to_path_buf()));
    }

    fn entry(name: &str) -> ServerEntry {
        ServerEntry {
            name: name.into(),
            url: format!("https://{name}.example"),
            token: format!("tok-{name}"),
        }
    }

    #[test]
    fn round_trip_and_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("client.toml");
        let mut c = ClientConfig::default();
        c.add(entry("vps"));
        c.add(entry("box"));
        c.save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert_eq!(ClientConfig::load(&path).unwrap(), c);
        assert_eq!(c.default.as_deref(), Some("vps"));
        // Saving again keeps the mode.
        c.save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[server]]"), "{text}");
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let c = ClientConfig::load(&dir.path().join("none.toml")).unwrap();
        assert_eq!(c, ClientConfig::default());
    }

    #[test]
    fn add_replaces_and_remove_moves_default() {
        let mut c = ClientConfig::default();
        c.add(entry("a"));
        c.add(entry("b"));
        let mut a2 = entry("a");
        a2.token = "new".into();
        c.add(a2);
        assert_eq!(c.servers.len(), 2);
        assert_eq!(c.get("a").unwrap().token, "new");
        assert!(c.remove("a"));
        assert_eq!(c.default.as_deref(), Some("b"));
        assert!(!c.remove("a"));
        assert!(c.set_default("zzz").is_err());
        c.set_default(LOCAL).unwrap();
    }

    #[test]
    fn resolve_order() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("api.sock");
        let mut c = ClientConfig::default();
        assert!(c.resolve(None, &sock).is_err());

        c.add(entry("a"));
        c.default = None;
        // Single server, no socket: use it.
        assert_eq!(c.resolve(None, &sock).unwrap().name, "a");
        // The local socket wins over an unset default.
        std::fs::write(&sock, b"").unwrap();
        assert_eq!(
            c.resolve(None, &sock).unwrap().endpoint,
            Endpoint::Unix(sock.clone())
        );
        // An explicit default wins over the socket.
        c.default = Some("a".into());
        assert_eq!(c.resolve(None, &sock).unwrap().name, "a");
        // -s wins over everything.
        assert_eq!(c.resolve(Some(LOCAL), &sock).unwrap().name, LOCAL);
        assert!(c.resolve(Some("nope"), &sock).is_err());
    }

    #[test]
    fn view_file_names_are_safe() {
        let p = view_path(Path::new("/c"), "my/box:1");
        assert_eq!(p, PathBuf::from("/c/view-my_box_1.toml"));
    }
}
