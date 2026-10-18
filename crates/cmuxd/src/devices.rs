//! Paired devices, web sessions and pairing codes (architecture 9).
//!
//! Tokens are 256 random bits; only their SHA-256 is stored (in
//! `devices.json`, mode 0600). A cookie session is a second random value that
//! maps to a device and dies with it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use cmux_core::model::Timestamp;
use cmux_proto::api::{DeviceView, Scope};
use rand::{Rng as _, RngExt as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::actor::Clock;
use crate::store::atomic_write;

/// Cookie sessions last this long without being renewed by a new login.
pub const SESSION_TTL_MS: u64 = 30 * 24 * 3600 * 1000;
/// `last_seen` is written to disk at most this often.
const SEEN_SAVE_MS: u64 = 60_000;
const MAX_PENDING_CODES: usize = 8;
/// Without 0, O, 1, I, which are easy to misread.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
pub const CODE_LEN: usize = 8;

pub fn hash_token(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 256 random bits, URL-safe base64.
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    rand::rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

// ---------------------------------------------------------------------------
// Devices and sessions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DeviceRec {
    id: String,
    name: String,
    scope: Scope,
    /// Hex SHA-256 of the token.
    token_hash: String,
    created: Timestamp,
    last_seen: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRec {
    /// Hex SHA-256 of the cookie value.
    hash: String,
    device: String,
    expires: Timestamp,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    devices: Vec<DeviceRec>,
    sessions: Vec<SessionRec>,
}

/// Who a credential belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAuth {
    pub id: String,
    pub scope: Scope,
}

#[derive(Debug)]
struct Inner {
    file: File,
    /// When `last_seen` was last written to disk.
    saved_at: Timestamp,
}

pub struct Devices {
    path: PathBuf,
    clock: Clock,
    inner: Mutex<Inner>,
}

impl Devices {
    /// Loads `<data_dir>/devices.json` (absent means no devices).
    pub fn open(data_dir: &Path, clock: Clock) -> std::io::Result<Devices> {
        let path = data_dir.join("devices.json");
        let file = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => File::default(),
            Err(e) => return Err(e),
        };
        let saved_at = clock.now_ms();
        Ok(Devices {
            path,
            clock,
            inner: Mutex::new(Inner { file, saved_at }),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self, inner: &mut Inner) -> std::io::Result<()> {
        let bytes = serde_json::to_vec_pretty(&inner.file)?;
        inner.saved_at = self.clock.now_ms();
        atomic_write(&self.path, &bytes)
    }

    /// Creates a device and returns its id and the token, which is not kept.
    pub fn create(&self, name: &str, scope: Scope) -> std::io::Result<(String, String)> {
        let token = random_token();
        let mut id = random_token();
        id.truncate(10);
        let now = self.clock.now_ms();
        let mut g = self.lock();
        g.file.devices.push(DeviceRec {
            id: id.clone(),
            name: name.trim().chars().take(80).collect(),
            scope,
            token_hash: hex(&hash_token(&token)),
            created: now,
            last_seen: Some(now),
        });
        self.save(&mut g)?;
        Ok((id, token))
    }

    /// The device a bearer token belongs to; marks it seen.
    pub fn authenticate(&self, token: &str) -> Option<DeviceAuth> {
        let want = hex(&hash_token(token));
        let mut g = self.lock();
        let rec = g
            .file
            .devices
            .iter()
            .find(|d| ct_eq(&d.token_hash, &want))?;
        let auth = DeviceAuth {
            id: rec.id.clone(),
            scope: rec.scope,
        };
        self.seen(&mut g, &auth.id);
        Some(auth)
    }

    /// The device a cookie value belongs to, if the session is live.
    pub fn authenticate_session(&self, cookie: &str) -> Option<DeviceAuth> {
        let want = hex(&hash_token(cookie));
        let now = self.clock.now_ms();
        let mut g = self.lock();
        let s = g
            .file
            .sessions
            .iter()
            .find(|s| ct_eq(&s.hash, &want) && s.expires > now)?;
        let id = s.device.clone();
        let scope = g.file.devices.iter().find(|d| d.id == id)?.scope;
        self.seen(&mut g, &id);
        Some(DeviceAuth { id, scope })
    }

    fn seen(&self, g: &mut Inner, id: &str) {
        let now = self.clock.now_ms();
        if let Some(d) = g.file.devices.iter_mut().find(|d| d.id == id) {
            d.last_seen = Some(now);
        }
        if now.saturating_sub(g.saved_at) >= SEEN_SAVE_MS {
            let _ = self.save(g);
        }
    }

    /// Trades a device for a new cookie value.
    pub fn create_session(&self, device: &str) -> std::io::Result<String> {
        let cookie = random_token();
        let now = self.clock.now_ms();
        let mut g = self.lock();
        g.file.sessions.retain(|s| s.expires > now);
        g.file.sessions.push(SessionRec {
            hash: hex(&hash_token(&cookie)),
            device: device.into(),
            expires: now + SESSION_TTL_MS,
        });
        self.save(&mut g)?;
        Ok(cookie)
    }

    pub fn drop_session(&self, cookie: &str) {
        let want = hex(&hash_token(cookie));
        let mut g = self.lock();
        g.file.sessions.retain(|s| !ct_eq(&s.hash, &want));
        let _ = self.save(&mut g);
    }

    /// A device's display name.
    pub fn name_of(&self, id: &str) -> Option<String> {
        let g = self.lock();
        g.file
            .devices
            .iter()
            .find(|d| d.id == id)
            .map(|d| d.name.clone())
    }

    pub fn list(&self, current: Option<&str>) -> Vec<DeviceView> {
        self.lock()
            .file
            .devices
            .iter()
            .map(|d| DeviceView {
                id: d.id.clone(),
                name: d.name.clone(),
                scope: d.scope,
                created: d.created,
                last_seen: d.last_seen,
                current: current == Some(d.id.as_str()),
            })
            .collect()
    }

    /// Removes a device and its sessions. False if there is no such device.
    pub fn revoke(&self, id: &str) -> std::io::Result<bool> {
        let mut g = self.lock();
        let before = g.file.devices.len();
        g.file.devices.retain(|d| d.id != id);
        g.file.sessions.retain(|s| s.device != id);
        let found = g.file.devices.len() != before;
        if found {
            self.save(&mut g)?;
        }
        Ok(found)
    }
}

// ---------------------------------------------------------------------------
// Pairing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    /// Wrong, expired or already used. One message for all three.
    Invalid,
    RateLimited {
        retry_after_secs: u64,
    },
}

struct PendingCode {
    code: String,
    scope: Scope,
    expires: Timestamp,
}

/// Single-use codes with a global attempt limit.
pub struct Pairing {
    clock: Clock,
    ttl_ms: u64,
    max_attempts: usize,
    pending: Mutex<Vec<PendingCode>>,
    attempts: Mutex<VecDeque<Timestamp>>,
}

const WINDOW_MS: u64 = 60_000;

impl Pairing {
    pub fn new(clock: Clock, ttl_secs: u64, attempts_per_min: u32) -> Pairing {
        Pairing {
            clock,
            ttl_ms: ttl_secs * 1000,
            max_attempts: attempts_per_min as usize,
            pending: Mutex::new(Vec::new()),
            attempts: Mutex::new(VecDeque::new()),
        }
    }

    pub fn ttl_secs(&self) -> u64 {
        self.ttl_ms / 1000
    }

    /// A new code for a device of `scope`. Old codes beyond a handful are dropped.
    pub fn start(&self, scope: Scope) -> String {
        let mut rng = rand::rng();
        let code: String = (0..CODE_LEN)
            .map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char)
            .collect();
        let now = self.clock.now_ms();
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        p.retain(|c| c.expires > now);
        if p.len() >= MAX_PENDING_CODES {
            p.remove(0);
        }
        p.push(PendingCode {
            code: code.clone(),
            scope,
            expires: now + self.ttl_ms,
        });
        code
    }

    /// Spends a code. Every call counts against the attempt limit, right or wrong.
    pub fn complete(&self, code: &str) -> Result<Scope, PairError> {
        let now = self.clock.now_ms();
        {
            let mut a = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
            while a
                .front()
                .is_some_and(|t| now.saturating_sub(*t) >= WINDOW_MS)
            {
                a.pop_front();
            }
            if a.len() >= self.max_attempts {
                let oldest = a.front().copied().unwrap_or(now);
                let wait = (WINDOW_MS - now.saturating_sub(oldest)).div_ceil(1000);
                return Err(PairError::RateLimited {
                    retry_after_secs: wait.max(1),
                });
            }
            a.push_back(now);
        }
        let norm: String = code
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let mut p = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        p.retain(|c| c.expires > now);
        match p.iter().position(|c| ct_eq(&c.code, &norm)) {
            Some(i) => Ok(p.remove(i).scope),
            None => Err(PairError::Invalid),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn pairing(clock: &Clock) -> Pairing {
        Pairing::new(clock.clone(), 300, 5)
    }

    #[test]
    fn code_is_single_use_and_forgiving_about_case() {
        let clock = Clock::default();
        let p = pairing(&clock);
        let code = p.start(Scope::Read);
        assert_eq!(code.len(), CODE_LEN);
        let spaced = format!("{}-{}", &code[..4], code[4..].to_lowercase());
        assert_eq!(p.complete(&spaced), Ok(Scope::Read));
        assert_eq!(p.complete(&code), Err(PairError::Invalid));
    }

    #[test]
    fn code_expires() {
        let clock = Clock::default();
        let p = pairing(&clock);
        let code = p.start(Scope::Full);
        clock.advance(Duration::from_secs(299));
        let other = p.start(Scope::Full);
        clock.advance(Duration::from_secs(2));
        assert_eq!(p.complete(&code), Err(PairError::Invalid));
        assert_eq!(p.complete(&other), Ok(Scope::Full));
    }

    #[test]
    fn attempts_are_limited_per_minute() {
        let clock = Clock::default();
        let p = pairing(&clock);
        let code = p.start(Scope::Full);
        for _ in 0..5 {
            assert_eq!(p.complete("WRONG123"), Err(PairError::Invalid));
        }
        // Even the right code is refused now.
        let Err(PairError::RateLimited { retry_after_secs }) = p.complete(&code) else {
            panic!("expected rate limit")
        };
        assert!((1..=60).contains(&retry_after_secs));
        clock.advance(Duration::from_secs(61));
        assert_eq!(p.complete(&code), Ok(Scope::Full));
    }

    #[test]
    fn devices_persist_and_revoke() {
        let tmp = tempfile::tempdir().unwrap();
        let clock = Clock::default();
        let d = Devices::open(tmp.path(), clock.clone()).unwrap();
        let (id, token) = d.create("phone", Scope::Read).unwrap();
        assert_eq!(
            d.authenticate(&token),
            Some(DeviceAuth {
                id: id.clone(),
                scope: Scope::Read
            })
        );
        assert_eq!(d.authenticate("nope"), None);
        let cookie = d.create_session(&id).unwrap();
        // Only hashes are on disk.
        let raw = std::fs::read_to_string(tmp.path().join("devices.json")).unwrap();
        assert!(!raw.contains(&token) && !raw.contains(&cookie));

        let d2 = Devices::open(tmp.path(), clock.clone()).unwrap();
        assert!(d2.authenticate(&token).is_some());
        assert!(d2.authenticate_session(&cookie).is_some());
        assert!(d2.list(Some(&id))[0].current);
        clock.advance(Duration::from_millis(SESSION_TTL_MS + 1));
        assert!(d2.authenticate_session(&cookie).is_none());
        assert!(d2.revoke(&id).unwrap());
        assert!(!d2.revoke(&id).unwrap());
        assert!(d2.authenticate(&token).is_none());
    }
}
