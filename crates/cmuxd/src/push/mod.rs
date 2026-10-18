//! Notifications for new needs-you items and finished plans (architecture
//! 11.3): Web Push to each subscribed device that opted into the kind, and
//! the optional `ntfy` topic from `[push]` in `config.toml`.
//!
//! [`PushRunner`] takes `Effect::Notify` and `Effect::NotifyPlanDone` from the
//! actor and sends on a task, so the actor never waits on the network. A push
//! service that answers 404 or 410 has forgotten the subscription, so it is
//! dropped. Subscriptions live in `push.json` and the VAPID key in
//! `vapid.key`, both mode 0600.

pub mod crypto;
pub mod payload;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use cmux_core::engine::Effect;
use cmux_core::model::{State, Timestamp};
use cmux_proto::api::{NotifyKind, PushPayload, PushSubscribeReq};
use serde::{Deserialize, Serialize};

use crate::actor::EffectRunner;
use crate::config::PushConfig;
use crate::diag::Diag;
use crate::store::atomic_write;
use crypto::Vapid;

/// How long a push service keeps an undelivered message, in seconds.
const TTL_SECS: u64 = 3600;
const SEND_TIMEOUT: Duration = Duration::from_secs(15);
/// Without a `[push] subject`, the VAPID contact.
const DEFAULT_SUBJECT: &str = "mailto:cmuxd@users.noreply.github.com";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SubRec {
    endpoint: String,
    p256dh: String,
    auth: String,
    /// The device that subscribed (`local` on the socket).
    device: String,
    kinds: Vec<NotifyKind>,
    created: Timestamp,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    subscriptions: Vec<SubRec>,
}

/// What one [`Push::notify`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Sent {
    pub delivered: usize,
    /// Answered 404 or 410, so removed.
    pub dropped: usize,
    pub failed: usize,
    /// Whether the ntfy topic took it (`None` without a topic).
    pub ntfy: Option<bool>,
}

pub struct Push {
    vapid: Vapid,
    subject: String,
    ntfy: Option<String>,
    ntfy_token: Option<String>,
    /// For the ntfy `Click` header: the app.
    click: Option<String>,
    path: PathBuf,
    subs: Mutex<File>,
    http: reqwest::Client,
    diag: Diag,
}

impl Push {
    /// Loads `push.json` and the VAPID key, making the key on first start.
    pub fn open(
        data_dir: &Path,
        cfg: &PushConfig,
        public_url: Option<&str>,
        diag: Diag,
    ) -> std::io::Result<Push> {
        let vapid = Vapid::load_or_create(&data_dir.join("vapid.key"))?;
        let path = data_dir.join("push.json");
        let file = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e),
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .timeout(SEND_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(std::io::Error::other)?;
        let https_url = public_url.filter(|u| u.starts_with("https://"));
        Ok(Push {
            vapid,
            subject: cfg
                .subject
                .clone()
                .or_else(|| https_url.map(str::to_string))
                .unwrap_or_else(|| DEFAULT_SUBJECT.into()),
            ntfy: cfg.ntfy.clone().filter(|u| !u.is_empty()),
            ntfy_token: cfg.ntfy_token.clone(),
            click: public_url.map(|u| format!("{}/", u.trim_end_matches('/'))),
            path,
            subs: Mutex::new(file),
            http,
            diag,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, File> {
        self.subs.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, file: &File) -> std::io::Result<()> {
        atomic_write(&self.path, &serde_json::to_vec_pretty(file)?)
    }

    /// The `applicationServerKey`.
    pub fn public_key(&self) -> String {
        self.vapid.public_key()
    }

    /// Adds `req` for `device`, or replaces the subscription with its endpoint.
    pub fn subscribe(
        &self,
        device: &str,
        req: PushSubscribeReq,
        now: Timestamp,
    ) -> Result<(), String> {
        check_endpoint(&req.endpoint)?;
        let p256dh = decode(&req.keys.p256dh).filter(|k| k.len() == 65 && k[0] == 4);
        p256dh.ok_or("error: keys.p256dh is not a P-256 public key")?;
        decode(&req.keys.auth)
            .filter(|a| a.len() == 16)
            .ok_or("error: keys.auth is not 16 bytes")?;
        let mut kinds = req.kinds;
        kinds.sort_by_key(|k| k.as_str());
        kinds.dedup();
        let rec = SubRec {
            endpoint: req.endpoint,
            p256dh: req.keys.p256dh,
            auth: req.keys.auth,
            device: device.to_string(),
            kinds,
            created: now,
        };
        let mut g = self.lock();
        g.subscriptions.retain(|s| s.endpoint != rec.endpoint);
        g.subscriptions.push(rec);
        self.save(&g).map_err(|e| format!("error: {e}"))
    }

    /// Drops the subscription with `endpoint`; whether there was one.
    pub fn unsubscribe(&self, endpoint: &str) -> std::io::Result<bool> {
        self.remove(|s| s.endpoint == endpoint)
    }

    /// Drops a revoked device's subscriptions.
    pub fn drop_device(&self, device: &str) -> std::io::Result<bool> {
        self.remove(|s| s.device == device)
    }

    fn remove(&self, f: impl Fn(&SubRec) -> bool) -> std::io::Result<bool> {
        let mut g = self.lock();
        let before = g.subscriptions.len();
        g.subscriptions.retain(|s| !f(s));
        if g.subscriptions.len() == before {
            return Ok(false);
        }
        self.save(&g)?;
        Ok(true)
    }

    /// The endpoints that are subscribed (tests and `/status`).
    pub fn endpoints(&self) -> Vec<String> {
        self.lock()
            .subscriptions
            .iter()
            .map(|s| s.endpoint.clone())
            .collect()
    }

    /// Sends `p` to the ntfy topic and to every subscription that wants its kind.
    pub async fn notify(&self, p: &PushPayload) -> Sent {
        let subs: Vec<SubRec> = self
            .lock()
            .subscriptions
            .iter()
            .filter(|s| s.kinds.contains(&p.kind))
            .cloned()
            .collect();
        let json = serde_json::to_vec(p).unwrap_or_default();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let (ntfy, results) = tokio::join!(
            self.send_ntfy(p),
            futures_util::future::join_all(subs.iter().map(|s| self.send_web(s, &json, p, now))),
        );
        let mut sent = Sent {
            ntfy,
            ..Sent::default()
        };
        let mut gone = Vec::new();
        for (s, r) in subs.iter().zip(results) {
            match r {
                Ok(()) => sent.delivered += 1,
                Err(Failure::Gone(_)) => gone.push(s.endpoint.clone()),
                Err(Failure::Other(e)) => {
                    sent.failed += 1;
                    self.diag.log(
                        "warn",
                        "push_failed",
                        serde_json::json!({ "host": host(&s.endpoint), "error": e }),
                    );
                }
            }
        }
        if !gone.is_empty() {
            sent.dropped = gone.len();
            if let Err(e) = self.remove(|s| gone.contains(&s.endpoint)) {
                self.diag.log(
                    "error",
                    "push_save",
                    serde_json::json!({ "error": e.to_string() }),
                );
            }
            self.diag.log(
                "info",
                "push_dropped",
                serde_json::json!({ "count": gone.len() }),
            );
        }
        sent
    }

    async fn send_web(
        &self,
        s: &SubRec,
        json: &[u8],
        p: &PushPayload,
        now_secs: u64,
    ) -> Result<(), Failure> {
        let other = |e: &str| Failure::Other(e.to_string());
        let (Some(ua), Some(auth)) = (decode(&s.p256dh), decode(&s.auth)) else {
            return Err(other("bad subscription keys"));
        };
        let body = crypto::encrypt(json, &ua, &auth).map_err(|e| other(e.0))?;
        let authz = self
            .vapid
            .authorization(&s.endpoint, &self.subject, now_secs)
            .ok_or_else(|| other("bad endpoint"))?;
        let urgent = matches!(p.kind, NotifyKind::Permission | NotifyKind::Question);
        let resp = self
            .http
            .post(&s.endpoint)
            .header("Authorization", authz)
            .header("TTL", TTL_SECS.to_string())
            .header("Urgency", if urgent { "high" } else { "normal" })
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|e| Failure::Other(without_url(e)))?;
        match resp.status().as_u16() {
            200..=299 => Ok(()),
            code @ (404 | 410) => Err(Failure::Gone(code)),
            code => Err(Failure::Other(format!("HTTP {code}"))),
        }
    }

    async fn send_ntfy(&self, p: &PushPayload) -> Option<bool> {
        let url = self.ntfy.as_ref()?;
        let mut req = self
            .http
            .post(url)
            .header("Title", format!("cmux: {}", title(p.kind)))
            .header("Tags", p.kind.as_str())
            .header(
                "Priority",
                match p.kind {
                    NotifyKind::Permission | NotifyKind::Question => "high",
                    _ => "default",
                },
            )
            .body(p.summary.clone());
        if let Some(c) = &self.click {
            req = req.header("Click", c);
        }
        if let Some(t) = &self.ntfy_token {
            req = req.bearer_auth(t);
        }
        let ok = match req.send().await {
            Ok(r) if r.status().is_success() => true,
            Ok(r) => {
                self.diag.log(
                    "warn",
                    "ntfy_failed",
                    serde_json::json!({ "status": r.status().as_u16() }),
                );
                false
            }
            Err(e) => {
                self.diag.log(
                    "warn",
                    "ntfy_failed",
                    serde_json::json!({ "error": without_url(e) }),
                );
                false
            }
        };
        Some(ok)
    }
}

enum Failure {
    Gone(#[allow(dead_code)] u16),
    Other(String),
}

/// A request error without its URL: endpoints are capabilities, and ntfy
/// topics are secrets, so neither goes into the diagnostics log.
fn without_url(e: reqwest::Error) -> String {
    e.without_url().to_string()
}

fn host(url: &str) -> Option<String> {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

/// The notification title for a kind.
pub fn title(kind: NotifyKind) -> &'static str {
    match kind {
        NotifyKind::Permission => "Permission request",
        NotifyKind::Question => "Question",
        NotifyKind::PlanReview => "Plan ready for review",
        NotifyKind::Stalled => "Agent needs you",
        NotifyKind::CiFailing => "CI failing",
        NotifyKind::PlanDone => "Plan done",
    }
}

/// Browsers send URL-safe base64, some with padding.
fn decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .or_else(|_| URL_SAFE.decode(s))
        .ok()
}

/// The server POSTs to whatever a client subscribes, so only HTTPS is
/// taken; plain HTTP only to a loopback host (local tests, mock services).
fn check_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| "error: endpoint is not a URL")?;
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d == "localhost",
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => return Err("error: endpoint has no host".into()),
    };
    match url.scheme() {
        "https" => Ok(()),
        "http" if loopback => Ok(()),
        _ => Err("error: endpoint must be an https URL".into()),
    }
}

/// Sends the notify effects. Offered after the other runners.
pub struct PushRunner(pub Arc<Push>);

impl EffectRunner for PushRunner {
    fn run(&self, effect: &Effect, state: &State) -> bool {
        let p = match effect {
            Effect::Notify { item } => state
                .inbox
                .iter()
                .find(|i| i.id == *item)
                .and_then(payload::for_item),
            Effect::NotifyPlanDone { plan } => state
                .plans
                .iter()
                .find(|p| p.id == *plan)
                .map(|p| payload::for_plan_done(&p.id, &p.title)),
            _ => return false,
        };
        if let Some(p) = p {
            let push = self.0.clone();
            tokio::spawn(async move {
                push.notify(&p).await;
            });
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_endpoints_or_loopback_http_are_taken() {
        assert!(check_endpoint("https://fcm.googleapis.com/fcm/send/x").is_ok());
        assert!(check_endpoint("http://127.0.0.1:9/x").is_ok());
        assert!(check_endpoint("http://localhost/x").is_ok());
        assert!(check_endpoint("http://[::1]/x").is_ok());
        assert!(check_endpoint("http://10.0.0.1/x").is_err());
        assert!(check_endpoint("http://example.com/x").is_err());
        assert!(check_endpoint("file:///etc/passwd").is_err());
        assert!(check_endpoint("not a url").is_err());
    }

    #[test]
    fn keys_decode_with_or_without_padding() {
        assert_eq!(decode("AAE"), Some(vec![0, 1]));
        assert_eq!(decode("AAE="), Some(vec![0, 1]));
        assert_eq!(decode("!!"), None);
    }
}
