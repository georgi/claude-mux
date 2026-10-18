//! A minimal HTTP/1.1 client for the local socket and for tests.
//!
//! `cmuxd pair` and `cmuxd hook` use it to talk to a running server without
//! pulling in a full HTTP stack. One request per connection
//! (`Connection: close`), JSON bodies only.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpStream, UnixStream};

/// Where the server listens.
#[derive(Debug, Clone)]
pub enum Endpoint {
    Unix(PathBuf),
    Tcp(SocketAddr),
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    /// Header names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn json<T: DeserializeOwned>(&self) -> std::io::Result<T> {
        serde_json::from_slice(&self.body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

impl Endpoint {
    /// Sends one request. `headers` are extra headers (auth, idempotency).
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> std::io::Result<Response> {
        match self {
            Endpoint::Unix(p) => {
                exchange(UnixStream::connect(p).await?, method, path, headers, body).await
            }
            Endpoint::Tcp(a) => {
                exchange(TcpStream::connect(a).await?, method, path, headers, body).await
            }
        }
    }

    /// `request` with a JSON body.
    pub async fn json(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &impl Serialize,
    ) -> std::io::Result<Response> {
        let bytes = serde_json::to_vec(body)?;
        let mut h = headers.to_vec();
        h.push(("Content-Type", "application/json"));
        self.request(method, path, &h, Some(&bytes)).await
    }
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    mut s: S,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> std::io::Result<Response> {
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: cmuxd\r\nConnection: close\r\n");
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    let body = body.unwrap_or_default();
    if !body.is_empty() || method != "GET" {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await?;
    s.write_all(body).await?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await?;
    parse_response(&raw)
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_string())
}

fn parse_response(raw: &[u8]) -> std::io::Result<Response> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| bad("incomplete response"))?;
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| bad("non-utf8 headers"))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| bad("bad status line"))?;
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let rest = &raw[split + 4..];
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked"));
    let body = if chunked {
        dechunk(rest)?
    } else {
        rest.to_vec()
    };
    Ok(Response {
        status,
        headers,
        body,
    })
}

fn dechunk(mut data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let eol = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or_else(|| bad("bad chunk"))?;
        let size = std::str::from_utf8(&data[..eol])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next()?.trim(), 16).ok())
            .ok_or_else(|| bad("bad chunk size"))?;
        data = &data[eol + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err(bad("short chunk"));
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_chunked() {
        let r = parse_response(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\nX-A: b\r\n\r\nhi")
            .unwrap();
        assert_eq!(
            (r.status, r.text().as_str(), r.header("x-a")),
            (201, "hi", Some("b"))
        );
        let r = parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.text(), "abcde");
        assert!(parse_response(b"garbage").is_err());
    }
}
