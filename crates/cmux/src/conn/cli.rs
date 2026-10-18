//! `cmux login`, `cmux logout` and `cmux servers`.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use cmux_proto::api::{PairCompleteReq, PairCompleteResp, route};

use super::http::Http;
use super::servers::{ClientConfig, LOCAL, ServerEntry, client_toml, local_socket};

pub const LOGIN_USAGE: &str = "\
USAGE:
    cmux login <URL> [--name NAME] [--code CODE]

Pairs this machine with a cmuxd server. Run `cmuxd pair` (or press the pair key
in the web app) on the server to get a one-time code, then enter it here.
The token is stored in ~/.config/cmux/client.toml (mode 0600).
";

pub const SERVERS_USAGE: &str = "\
USAGE:
    cmux servers                  List paired servers
    cmux servers default NAME     Connect to NAME by default
    cmux servers remove NAME      Forget a server (same as `cmux logout NAME`)
    cmux logout [NAME]            Forget a server (default: the default server)
";

/// Entry point for `cmux login ARGS...`.
pub fn login(args: &[String]) -> Result<()> {
    let mut url = None;
    let mut name = None;
    let mut code = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{LOGIN_USAGE}");
                return Ok(());
            }
            "--name" => name = Some(it.next().context("--name needs a value")?.clone()),
            "--code" => code = Some(it.next().context("--code needs a value")?.clone()),
            s if s.starts_with('-') => bail!("unknown option {s}\n\n{LOGIN_USAGE}"),
            u if url.is_none() => url = Some(u.to_string()),
            _ => bail!("unexpected argument\n\n{LOGIN_USAGE}"),
        }
    }
    let Some(url) = url else {
        bail!("missing URL\n\n{LOGIN_USAGE}");
    };
    let code = match code {
        Some(c) => c,
        None => {
            eprint!("Pairing code: ");
            std::io::stderr().flush()?;
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            line
        }
    };
    if let Some(w) = cleartext_warning(&normalize_url(&url)?) {
        eprintln!("{w}");
    }
    // Load first: a broken config should not cost the one-time code.
    let path = client_toml();
    let mut cfg = ClientConfig::load(&path)?;
    let mut entry = pair(&url, &code, name.as_deref(), &device_name())?;
    // The server's name is only known now and the code is spent, so a clash
    // is resolved by renaming rather than by failing.
    let taken = rename_clash(&cfg, &mut entry, name.is_some());
    let n = entry.name.clone();
    cfg.add(entry);
    cfg.save(&path)?;
    if let Some(old) = taken {
        let why = if old == LOCAL {
            "is reserved for the local server"
        } else {
            "is already paired with another server"
        };
        eprintln!("note: {old:?} {why}; stored as {n:?}");
    }
    println!("Paired with {n}. `cmux servers` lists the paired servers.");
    Ok(())
}

/// Completes pairing against `url` and returns the entry to store.
pub fn pair(url: &str, code: &str, name: Option<&str>, device: &str) -> Result<ServerEntry> {
    let url = normalize_url(url)?;
    let code = code.trim().to_string();
    if code.is_empty() {
        bail!("no pairing code entered");
    }
    // Checked before the code is spent. A name the server picks is renamed by
    // `rename_clash` instead.
    if name == Some(LOCAL) {
        bail!("{LOCAL:?} is reserved for the local server; pick another with --name");
    }
    let resp: PairCompleteResp = Http::anonymous(&url)
        .post(
            route::PAIR_COMPLETE,
            &PairCompleteReq {
                code,
                device_name: device.to_string(),
            },
        )
        .with_context(|| format!("pairing with {url} failed"))?;
    let name = match name {
        Some(n) => n.to_string(),
        None if resp.server.name.is_empty() => host_of(&url),
        None => resp.server.name.clone(),
    };
    Ok(ServerEntry {
        name,
        url,
        token: resp.token,
    })
}

/// Keeps a login from silently replacing another server's entry: a name the
/// server chose that is taken by a different URL becomes `NAME-2`, `NAME-3`...
/// (an explicit `--name` may replace). The reserved `local` is renamed the
/// same way. A candidate already paired with this URL is reused, so a repeated
/// login refreshes its token. Returns the name that was taken.
fn rename_clash(
    cfg: &ClientConfig,
    entry: &mut ServerEntry,
    explicit_name: bool,
) -> Option<String> {
    let clashes = |name: &str| cfg.get(name).is_some_and(|old| old.url != entry.url);
    let reserved = entry.name == LOCAL;
    if explicit_name || (!reserved && !clashes(&entry.name)) {
        return None;
    }
    let old = entry.name.clone();
    entry.name = (2..)
        .map(|n| format!("{old}-{n}"))
        .find(|c| c != LOCAL && !clashes(c))
        .expect("an unused name exists");
    Some(old)
}

/// A warning for an `http://` URL that is not loopback: the token would cross
/// the network in cleartext.
fn cleartext_warning(url: &str) -> Option<String> {
    let host = host_of(url);
    let loopback = host == "localhost" || host == "[::1]" || host.starts_with("127.");
    (url.starts_with("http://") && !loopback).then(|| {
        format!("warning: {url} is not https; the pairing code and token travel in cleartext")
    })
}

fn normalize_url(u: &str) -> Result<String> {
    let u = u.trim().trim_end_matches('/');
    let u = if u.contains("://") {
        u.to_string()
    } else {
        format!("https://{u}")
    };
    if !(u.starts_with("https://") || u.starts_with("http://")) {
        bail!("{u}: the URL must start with https:// (or http://)");
    }
    Ok(u)
}

fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    // An IPv6 literal keeps its brackets and colons: `[::1]`.
    if rest.starts_with('[')
        && let Some(end) = rest.find(']')
    {
        return rest[..=end].to_string();
    }
    rest.split(['/', ':'])
        .next()
        .unwrap_or("server")
        .to_string()
}

fn device_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for `buf.len()` bytes; gethostname NUL-terminates or truncates.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len() - 1) };
    let host = if rc == 0 {
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        String::new()
    };
    let host = if host.is_empty() {
        "cmux".to_string()
    } else {
        host
    };
    format!("{host} (cmux)")
}

/// Entry point for `cmux servers ARGS...` and `cmux logout ARGS...`.
pub fn servers(args: &[String], logout: bool) -> Result<()> {
    let path = client_toml();
    let mut out = std::io::stdout().lock();
    servers_in(&path, &local_socket(), args, logout, &mut out)
}

pub fn servers_in(
    path: &Path,
    socket: &Path,
    args: &[String],
    logout: bool,
    out: &mut dyn Write,
) -> Result<()> {
    let mut cfg = ClientConfig::load(path)?;
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match (logout, args.as_slice()) {
        (_, ["-h" | "--help"]) => write!(out, "{SERVERS_USAGE}")?,
        (false, []) | (false, ["list"]) => list(&cfg, socket, out)?,
        (false, ["default", name]) => {
            cfg.set_default(name)?;
            cfg.save(path)?;
            writeln!(out, "Default server: {name}")?;
        }
        (false, ["remove", name]) => remove(&mut cfg, path, name, out)?,
        (true, [name]) => remove(&mut cfg, path, name, out)?,
        (true, []) => match cfg.default.clone() {
            Some(d) if d != LOCAL => remove(&mut cfg, path, &d, out)?,
            _ => bail!("no default server to log out of; name one: cmux logout NAME"),
        },
        _ => bail!("{SERVERS_USAGE}"),
    }
    Ok(())
}

fn remove(cfg: &mut ClientConfig, path: &Path, name: &str, out: &mut dyn Write) -> Result<()> {
    if !cfg.remove(name) {
        bail!("no server named {name:?} (see `cmux servers`)");
    }
    cfg.save(path)?;
    writeln!(
        out,
        "Removed {name}. (Revoke the device on the server to invalidate its token.)"
    )?;
    Ok(())
}

fn list(cfg: &ClientConfig, socket: &Path, out: &mut dyn Write) -> Result<()> {
    let default = cfg.default.as_deref();
    let mark = |n: &str| if default == Some(n) { "*" } else { " " };
    if socket.exists() {
        writeln!(
            out,
            "{} {LOCAL:<12} {} (this machine)",
            mark(LOCAL),
            socket.display()
        )?;
    }
    for s in &cfg.servers {
        writeln!(out, "{} {:<12} {}", mark(&s.name), s.name, s.url)?;
    }
    if !socket.exists() && cfg.servers.is_empty() {
        writeln!(out, "No servers. Pair one with `cmux login <url>`.")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    fn run(path: &Path, sock: &Path, args: &[&str], logout: bool) -> Result<String> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        servers_in(path, sock, &args, logout, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn servers_list_default_remove() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("client.toml");
        let sock = dir.path().join("none.sock");
        assert!(
            run(&path, &sock, &[], false)
                .unwrap()
                .contains("No servers")
        );

        let mut cfg = ClientConfig::default();
        for n in ["a", "b"] {
            cfg.add(ServerEntry {
                name: n.into(),
                url: format!("https://{n}.example"),
                token: "t".into(),
            });
        }
        cfg.save(&path).unwrap();
        let listing = run(&path, &sock, &["list"], false).unwrap();
        assert!(listing.contains("* a"), "{listing}");
        assert!(!listing.contains("t\n"), "tokens must not be printed");

        run(&path, &sock, &["default", "b"], false).unwrap();
        assert_eq!(
            ClientConfig::load(&path).unwrap().default.as_deref(),
            Some("b")
        );
        assert!(run(&path, &sock, &["default", "zzz"], false).is_err());

        run(&path, &sock, &["remove", "b"], false).unwrap();
        let cfg = ClientConfig::load(&path).unwrap();
        assert_eq!(cfg.default.as_deref(), Some("a"));
        // logout with no name removes the default.
        run(&path, &sock, &[], true).unwrap();
        assert!(ClientConfig::load(&path).unwrap().servers.is_empty());
        assert!(run(&path, &sock, &["nope"], true).is_err());
    }

    #[test]
    fn url_normalizing() {
        assert_eq!(
            normalize_url("box.example.ts.net/").unwrap(),
            "https://box.example.ts.net"
        );
        assert_eq!(
            normalize_url("http://127.0.0.1:7777").unwrap(),
            "http://127.0.0.1:7777"
        );
        assert!(normalize_url("ftp://x").is_err());
        assert_eq!(host_of("https://box.ts.net:8443"), "box.ts.net");
        assert_eq!(host_of("http://[::1]:8080/x"), "[::1]");
    }

    #[test]
    fn pairing_posts_the_code_and_returns_the_token() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = vec![0u8; 8192];
            // Headers and body may arrive in separate packets.
            let mut n = 0;
            while !String::from_utf8_lossy(&buf[..n]).contains('}') {
                n += c.read(&mut buf[n..]).unwrap();
            }
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let body = r#"{"device_id":"d1","token":"secret-token","scope":"full","server":{"version":"1","name":"vps"}}"#;
            write!(
                c,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
            req
        });
        let e = pair(
            &format!("http://{addr}"),
            " ABCD2345\n",
            None,
            "laptop (cmux)",
        )
        .unwrap();
        assert_eq!(e.name, "vps");
        assert_eq!(e.token, "secret-token");
        assert_eq!(e.url, format!("http://{addr}"));
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /v1/pair/complete "), "{req}");
        assert!(req.contains(r#""code":"ABCD2345""#), "{req}");
        assert!(req.contains("laptop (cmux)"), "{req}");
        assert!(!req.to_lowercase().contains("authorization"));
    }

    #[test]
    fn login_does_not_replace_another_server_without_name() {
        let entry = |name: &str, url: &str| ServerEntry {
            name: name.into(),
            url: url.into(),
            token: "t".into(),
        };
        let mut cfg = ClientConfig::default();
        cfg.add(entry("vps", "https://a.example"));
        cfg.add(entry("vps-2", "https://c.example"));
        // Same name, other URL: renamed unless --name was given.
        let mut other = entry("vps", "https://b.example");
        assert_eq!(rename_clash(&cfg, &mut other, false), Some("vps".into()));
        assert_eq!(other.name, "vps-3");
        let mut other = entry("vps", "https://b.example");
        assert_eq!(rename_clash(&cfg, &mut other, true), None);
        assert_eq!(other.name, "vps");
        // Re-pairing the same URL refreshes the token.
        let mut same = entry("vps", "https://a.example");
        assert_eq!(rename_clash(&cfg, &mut same, false), None);
        let mut new = entry("new", "https://b.example");
        assert_eq!(rename_clash(&cfg, &mut new, false), None);
    }

    #[test]
    fn relogin_reuses_the_name_already_paired_with_the_url() {
        let entry = |name: &str, url: &str, token: &str| ServerEntry {
            name: name.into(),
            url: url.into(),
            token: token.into(),
        };
        let mut cfg = ClientConfig::default();
        cfg.add(entry("vps", "https://a.example", "old"));
        cfg.add(entry("vps-2", "https://b.example", "old"));
        // The server is stored as vps-2; logging into it again reuses that name.
        let mut again = entry("vps", "https://b.example", "new");
        assert_eq!(rename_clash(&cfg, &mut again, false), Some("vps".into()));
        assert_eq!(again.name, "vps-2");
        cfg.add(again);
        assert_eq!(cfg.servers.len(), 2);
        assert_eq!(cfg.get("vps-2").unwrap().token, "new");
    }

    #[test]
    fn a_server_named_local_is_renamed() {
        let cfg = ClientConfig::default();
        let mut e = ServerEntry {
            name: LOCAL.into(),
            url: "https://a.example".into(),
            token: "t".into(),
        };
        assert_eq!(rename_clash(&cfg, &mut e, false), Some(LOCAL.into()));
        assert_eq!(e.name, "local-2");
        // With --name the reserved name is refused before the code is spent.
        assert!(pair("https://a.example", "ABCD", Some(LOCAL), "d").is_err());
    }

    #[test]
    fn cleartext_warning_skips_https_and_loopback() {
        assert!(cleartext_warning("http://box.example:8080").is_some());
        assert!(cleartext_warning("https://box.example").is_none());
        assert!(cleartext_warning("http://127.0.0.1:9").is_none());
        assert!(cleartext_warning("http://localhost:9").is_none());
        assert!(cleartext_warning("http://[::1]:9").is_none());
        assert!(cleartext_warning("http://[2001:db8::1]:9").is_some());
    }
}
