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
    let entry = pair(&url, &code, name.as_deref(), &device_name())?;
    let path = client_toml();
    let mut cfg = ClientConfig::load(&path)?;
    let n = entry.name.clone();
    cfg.add(entry);
    cfg.save(&path)?;
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
    if name == LOCAL {
        bail!("{LOCAL:?} is reserved for the local server; pick another with --name");
    }
    Ok(ServerEntry {
        name,
        url,
        token: resp.token,
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
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', ':'])
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
    let host = if host.is_empty() { "cmux".to_string() } else { host };
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
    writeln!(out, "Removed {name}. (Revoke the device on the server to invalidate its token.)")?;
    Ok(())
}

fn list(cfg: &ClientConfig, socket: &Path, out: &mut dyn Write) -> Result<()> {
    let default = cfg.default.as_deref();
    let mark = |n: &str| if default == Some(n) { "*" } else { " " };
    if socket.exists() {
        writeln!(out, "{} {LOCAL:<12} {} (this machine)", mark(LOCAL), socket.display())?;
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
        assert!(run(&path, &sock, &[], false).unwrap().contains("No servers"));

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
        assert_eq!(ClientConfig::load(&path).unwrap().default.as_deref(), Some("b"));
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
        assert_eq!(normalize_url("box.example.ts.net/").unwrap(), "https://box.example.ts.net");
        assert_eq!(normalize_url("http://127.0.0.1:7777").unwrap(), "http://127.0.0.1:7777");
        assert!(normalize_url("ftp://x").is_err());
        assert_eq!(host_of("https://box.ts.net:8443"), "box.ts.net");
    }

    #[test]
    fn pairing_posts_the_code_and_returns_the_token() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut buf = vec![0u8; 8192];
            let n = c.read(&mut buf).unwrap();
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
        let e = pair(&format!("http://{addr}"), " ABCD2345\n", None, "laptop (cmux)").unwrap();
        assert_eq!(e.name, "vps");
        assert_eq!(e.token, "secret-token");
        assert_eq!(e.url, format!("http://{addr}"));
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /v1/pair/complete "), "{req}");
        assert!(req.contains(r#""code":"ABCD2345""#), "{req}");
        assert!(req.contains("laptop (cmux)"), "{req}");
        assert!(!req.to_lowercase().contains("authorization"));
    }
}
