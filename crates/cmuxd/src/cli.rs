//! Client-side subcommands that talk to a running server.

use std::path::Path;

use cmux_proto::api::{ErrorBody, PairStartReq, PairStartResp, Scope, route};

use crate::client::Endpoint;

/// `cmuxd pair`: asks the running server (over its Unix socket) for a code and
/// prints it with a QR code of the pairing URL.
pub async fn pair(socket: &Path, scope: Option<Scope>) -> Result<(), String> {
    let ep = Endpoint::Unix(socket.to_path_buf());
    let resp = ep
        .json("POST", route::PAIR_START, &[], &PairStartReq { scope })
        .await
        .map_err(|e| {
            format!(
                "cannot reach cmuxd on {}: {e}\nIs `cmuxd serve` running?",
                socket.display()
            )
        })?;
    if resp.status != 200 {
        let msg = resp
            .json::<ErrorBody>()
            .map_or_else(|_| resp.text(), |e| e.message);
        return Err(format!("pairing failed ({}): {msg}", resp.status));
    }
    let p: PairStartResp = resp.json().map_err(|e| e.to_string())?;
    print!("{}", render_pairing(&p));
    Ok(())
}

pub fn render_pairing(p: &PairStartResp) -> String {
    let qr = qrcode::QrCode::new(p.url.as_bytes())
        .map(|q| {
            q.render::<qrcode::render::unicode::Dense1x2>()
                .quiet_zone(true)
                .build()
        })
        .unwrap_or_default();
    let mins = p.expires_in_secs / 60;
    let mut out = format!(
        "{qr}\n\nPairing code:  {}\nPairing URL:   {}\n",
        p.code, p.url
    );
    out.push_str(&format!(
        "Valid for {mins} minutes, single use. The device gets {:?} access.\n",
        p.scope
    ));
    if p.scope == Scope::Full {
        out.push_str(
            "A full-scope device can run shells and Claude as this user. Pair only devices you trust.\n",
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_text_has_code_url_and_qr() {
        let p = PairStartResp {
            code: "ABCD2345".into(),
            url: "http://127.0.0.1:7878/pair?code=ABCD2345".into(),
            expires_in_secs: 300,
            scope: Scope::Full,
        };
        let s = render_pairing(&p);
        assert!(s.contains("ABCD2345") && s.contains("5 minutes"));
        assert!(s.contains('█') || s.contains('▀') || s.contains('▄'));
        assert!(s.contains("shells"));
    }
}
