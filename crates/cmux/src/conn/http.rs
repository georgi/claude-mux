//! Blocking REST client for `/v1`.
//!
//! Remote servers go through `ureq` (rustls) with a bearer token. The local
//! server is reached over its Unix socket with a tiny HTTP/1.1 client written
//! here, because `ureq` has no Unix socket transport. Both send an
//! `Idempotency-Key` on mutations and retry a failed transfer with the same
//! key, so a flaky link cannot act twice.

use std::fmt;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use cmux_proto::api::{AUTHORIZATION, ErrorBody, IDEMPOTENCY_KEY};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::servers::Endpoint;

const TIMEOUT: Duration = Duration::from_secs(30);
/// Extra attempts after a transport error on a mutation (same key each time).
const RETRIES: usize = 2;

#[derive(Debug)]
pub enum HttpError {
    /// Could not reach the server or the transfer broke off.
    Transport(String),
    /// A non-2xx answer. `body` is set when the server sent an [`ErrorBody`].
    Api {
        status: u16,
        body: Option<ErrorBody>,
        text: String,
    },
    /// A 2xx answer that does not match the expected type.
    Decode(String),
}

impl HttpError {
    pub fn status(&self) -> Option<u16> {
        match self {
            HttpError::Api { status, .. } => Some(*status),
            _ => None,
        }
    }
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Transport(e) => write!(f, "connection failed: {e}"),
            HttpError::Api {
                body: Some(b),
                status,
                ..
            } => write!(f, "{} (HTTP {status})", b.message),
            HttpError::Api { status, text, .. } => write!(f, "HTTP {status}: {text}"),
            HttpError::Decode(e) => write!(f, "unexpected response: {e}"),
        }
    }
}

impl std::error::Error for HttpError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct Http {
    endpoint: Endpoint,
    agent: ureq::Agent,
}

impl Http {
    pub fn new(endpoint: Endpoint) -> Http {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(TIMEOUT))
            .build()
            .into();
        Http { endpoint, agent }
    }

    /// For `/pair/complete`, which needs no token.
    pub fn anonymous(url: &str) -> Http {
        Http::new(Endpoint::Remote {
            url: url.to_string(),
            token: String::new(),
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, HttpError> {
        decode(self.request("GET", path, None, None)?)
    }

    /// POST with a fresh idempotency key.
    pub fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, HttpError> {
        self.send_json("POST", path, body, &new_key())
    }

    /// POST that reuses `key`, for a caller that retries on its own (a form
    /// resubmitted after a lost answer).
    pub fn post_keyed<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        key: &str,
    ) -> Result<T, HttpError> {
        self.send_json("POST", path, body, key)
    }

    pub fn put<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, HttpError> {
        self.send_json("PUT", path, body, &new_key())
    }

    pub fn patch<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, HttpError> {
        self.send_json("PATCH", path, body, &new_key())
    }

    /// DELETE; the answer body is ignored.
    pub fn delete(&self, path: &str) -> Result<(), HttpError> {
        self.request("DELETE", path, None, Some(&new_key()))
            .map(|_| ())
    }

    fn send_json<B: Serialize, T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        body: &B,
        key: &str,
    ) -> Result<T, HttpError> {
        let bytes = serde_json::to_vec(body).map_err(|e| HttpError::Decode(e.to_string()))?;
        decode(self.request(method, path, Some(bytes), Some(key))?)
    }

    /// One request. A non-2xx answer is an [`HttpError::Api`]. Mutations
    /// (those with an idempotency key) are retried on transport errors.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
        idem: Option<&str>,
    ) -> Result<Response, HttpError> {
        let attempts = if idem.is_some() { 1 + RETRIES } else { 1 };
        let mut last = None;
        for _ in 0..attempts {
            let r = match &self.endpoint {
                Endpoint::Unix(sock) => unix_request(sock, method, path, body.as_deref(), idem),
                Endpoint::Remote { url, token } => {
                    self.tcp_request(url, token, method, path, body.as_deref(), idem)
                }
            };
            match r {
                Ok(resp) => return check(resp),
                Err(e @ HttpError::Transport(_)) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("at least one attempt"))
    }

    fn tcp_request(
        &self,
        base: &str,
        token: &str,
        method: &str,
        path: &str,
        body: Option<&[u8]>,
        idem: Option<&str>,
    ) -> Result<Response, HttpError> {
        let url = format!("{}{}", base.trim_end_matches('/'), path);
        let mut req = ureq::http::Request::builder().method(method).uri(&url);
        if !token.is_empty() {
            req = req.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(k) = idem {
            req = req.header(IDEMPOTENCY_KEY, k);
        }
        if body.is_some() {
            req = req.header("Content-Type", "application/json");
        }
        let req = req
            .body(body.map(<[u8]>::to_vec).unwrap_or_default())
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        let resp = self
            .agent
            .run(req)
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        let status = resp.status().as_u16();
        let body = resp
            .into_body()
            .read_to_vec()
            .map_err(|e| HttpError::Transport(e.to_string()))?;
        Ok(Response { status, body })
    }
}

pub fn new_key() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn check(resp: Response) -> Result<Response, HttpError> {
    if (200..300).contains(&resp.status) {
        return Ok(resp);
    }
    Err(HttpError::Api {
        status: resp.status,
        body: serde_json::from_slice(&resp.body).ok(),
        text: String::from_utf8_lossy(&resp.body).chars().take(300).collect(),
    })
}

fn decode<T: DeserializeOwned>(resp: Response) -> Result<T, HttpError> {
    // `()` and empty answers (204): decode from JSON null.
    let bytes: &[u8] = if resp.body.is_empty() {
        b"null"
    } else {
        &resp.body
    };
    serde_json::from_slice(bytes).map_err(|e| HttpError::Decode(e.to_string()))
}

// ---------------------------------------------------------------------------
// HTTP/1.1 over a Unix socket
// ---------------------------------------------------------------------------

fn unix_request(
    sock: &Path,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    idem: Option<&str>,
) -> Result<Response, HttpError> {
    let te = |e: std::io::Error| HttpError::Transport(format!("{}: {e}", sock.display()));
    let mut s = UnixStream::connect(sock).map_err(te)?;
    s.set_read_timeout(Some(TIMEOUT)).map_err(te)?;
    s.set_write_timeout(Some(TIMEOUT)).map_err(te)?;
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n");
    if let Some(k) = idem {
        head.push_str(&format!("{IDEMPOTENCY_KEY}: {k}\r\n"));
    }
    if let Some(b) = body {
        head.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    } else if method != "GET" {
        head.push_str("Content-Length: 0\r\n");
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).map_err(te)?;
    if let Some(b) = body {
        s.write_all(b).map_err(te)?;
    }
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(te)?;
    parse_response(&raw).map_err(HttpError::Transport)
}

/// Parses a complete HTTP/1.1 response read until EOF.
pub fn parse_response(raw: &[u8]) -> Result<Response, String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("truncated response")?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| "bad response header")?;
    let rest = &raw[split + 4..];
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("bad status line {status_line:?}"))?;
    let mut chunked = false;
    let mut length = None;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") {
            chunked = true;
        } else if k == "content-length" {
            length = v.parse::<usize>().ok();
        }
    }
    let body = if chunked {
        decode_chunked(rest)?
    } else if let Some(n) = length {
        if rest.len() < n {
            return Err("truncated response body".into());
        }
        rest[..n].to_vec()
    } else {
        rest.to_vec()
    };
    Ok(Response { status, body })
}

fn decode_chunked(mut buf: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let eol = buf
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("truncated chunked body")?;
        let size_str = std::str::from_utf8(&buf[..eol]).map_err(|_| "bad chunk size")?;
        let size_str = size_str.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_str, 16).map_err(|_| "bad chunk size")?;
        buf = &buf[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if buf.len() < size + 2 {
            return Err("truncated chunked body".into());
        }
        out.extend_from_slice(&buf[..size]);
        buf = &buf[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn parses_content_length_chunked_and_eof_bodies() {
        let r = parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nhi").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hi"[..]));
        let r = parse_response(
            b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x=1\r\nde\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (201, &b"abcde"[..]));
        let r = parse_response(b"HTTP/1.1 404 Not Found\r\n\r\nnope").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (404, &b"nope"[..]));
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nhi").is_err());
        assert!(parse_response(b"garbage").is_err());
    }

    /// Serves `answers` in order on a fresh socket; returns the raw requests.
    fn serve(
        answers: Vec<Vec<u8>>,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::thread::JoinHandle<Vec<String>>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        let l = UnixListener::bind(&path).unwrap();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for a in answers {
                let (mut c, _) = l.accept().unwrap();
                let mut buf = vec![0u8; 8192];
                let n = c.read(&mut buf).unwrap();
                seen.push(String::from_utf8_lossy(&buf[..n]).into_owned());
                if !a.is_empty() {
                    c.write_all(&a).unwrap();
                }
            }
            seen
        });
        (dir, path, h)
    }

    #[test]
    fn unix_get_and_post_with_idempotency_key() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\n\r\n{\"a\":42}".to_vec();
        let (_d, path, h) = serve(vec![ok.clone(), ok]);
        let http = Http::new(Endpoint::Unix(path));
        #[derive(serde::Deserialize, Serialize)]
        struct A {
            a: u32,
        }
        let a: A = http.get("/v1/x").unwrap();
        assert_eq!(a.a, 42);
        let b: A = http.post("/v1/y", &A { a: 1 }).unwrap();
        assert_eq!(b.a, 42);
        let seen = h.join().unwrap();
        assert!(seen[0].starts_with("GET /v1/x HTTP/1.1\r\n"), "{}", seen[0]);
        assert!(!seen[0].contains(IDEMPOTENCY_KEY));
        assert!(seen[1].starts_with("POST /v1/y HTTP/1.1\r\n"));
        assert!(seen[1].contains("Idempotency-Key: "), "{}", seen[1]);
        assert!(seen[1].ends_with("{\"a\":1}"));
    }

    #[test]
    fn api_errors_decode_and_transport_errors_retry_with_same_key() {
        let err = br#"{"code":"forbidden","message":"error: read-only"}"#;
        let mut resp = format!(
            "HTTP/1.1 403 Forbidden\r\nContent-Length: {}\r\n\r\n",
            err.len()
        )
        .into_bytes();
        resp.extend_from_slice(err);
        // First attempt: the server hangs up without answering; second: 403.
        let (_d, path, h) = serve(vec![Vec::new(), resp]);
        let http = Http::new(Endpoint::Unix(path));
        let e = http
            .post::<_, serde_json::Value>("/v1/z", &serde_json::json!({}))
            .unwrap_err();
        assert_eq!(e.status(), Some(403));
        assert!(e.to_string().contains("read-only"), "{e}");
        let seen = h.join().unwrap();
        let key = |s: &str| {
            s.lines()
                .find(|l| l.starts_with("Idempotency-Key"))
                .unwrap()
                .to_string()
        };
        assert_eq!(key(&seen[0]), key(&seen[1]));
    }

    #[test]
    fn unreachable_socket_is_a_transport_error() {
        let http = Http::new(Endpoint::Unix("/nonexistent/cmux.sock".into()));
        let e = http.get::<serde_json::Value>("/v1/state").unwrap_err();
        assert!(matches!(e, HttpError::Transport(_)), "{e}");
    }
}
