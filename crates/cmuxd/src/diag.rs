//! Diagnostics: one JSON object per line in `<data>/diagnostics/cmuxd.jsonl`,
//! rotated at 5 MiB with one old file kept (`cmuxd.jsonl.1`).
//!
//! The log never holds prompts, message bodies, tool inputs or tokens: callers
//! log ids, counts and error text only (architecture 9). [`DiagLayer`] feeds
//! `tracing` events into it.

use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

pub const MAX_BYTES: u64 = 5 * 1024 * 1024;

struct Sink {
    path: PathBuf,
    file: File,
    size: u64,
    max: u64,
}

/// A rotating JSONL writer. Cheap to clone.
#[derive(Clone)]
pub struct Diag(Arc<Mutex<Sink>>);

impl Diag {
    /// Opens (creating) `<data_dir>/diagnostics/cmuxd.jsonl`.
    pub fn open(data_dir: &Path) -> io::Result<Diag> {
        Self::open_with(data_dir, MAX_BYTES)
    }

    pub fn open_with(data_dir: &Path, max: u64) -> io::Result<Diag> {
        let dir = data_dir.join("diagnostics");
        crate::config::ensure_private_dir(&dir)?;
        let path = dir.join("cmuxd.jsonl");
        let (file, size) = open_append(&path)?;
        Ok(Diag(Arc::new(Mutex::new(Sink {
            path,
            file,
            size,
            max,
        }))))
    }

    /// Writes `{"ts":..,"level":..,"event":..,...fields}`.
    pub fn log(&self, level: &str, event: &str, fields: Value) {
        let mut obj = Map::new();
        obj.insert("ts".into(), json!(now_ms()));
        obj.insert("level".into(), json!(level));
        obj.insert("event".into(), json!(event));
        if let Value::Object(f) = fields {
            for (k, v) in f {
                obj.entry(k).or_insert(v);
            }
        }
        let mut line = Value::Object(obj).to_string();
        line.push('\n');
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if s.size + line.len() as u64 > s.max {
            let _ = s.rotate();
        }
        if s.file.write_all(line.as_bytes()).is_ok() {
            s.size += line.len() as u64;
        }
    }

    pub fn path(&self) -> PathBuf {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .path
            .clone()
    }
}

impl Sink {
    fn rotate(&mut self) -> io::Result<()> {
        let old = self.path.with_extension("jsonl.1");
        fs::rename(&self.path, old)?;
        let (file, size) = open_append(&self.path)?;
        self.file = file;
        self.size = size;
        Ok(())
    }
}

fn open_append(path: &Path) -> io::Result<(File, u64)> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    let size = file.metadata()?.len();
    Ok((file, size))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A `tracing` layer that writes events to a [`Diag`].
pub struct DiagLayer(pub Diag);

#[derive(Default)]
struct Fields {
    message: String,
    rest: Map<String, Value>,
}

impl Visit for Fields {
    fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
        if f.name() == "message" {
            let _ = write!(self.message, "{v:?}");
        } else {
            self.rest.insert(f.name().into(), json!(format!("{v:?}")));
        }
    }
    fn record_str(&mut self, f: &Field, v: &str) {
        if f.name() == "message" {
            self.message.push_str(v);
        } else {
            self.rest.insert(f.name().into(), json!(v));
        }
    }
    fn record_u64(&mut self, f: &Field, v: u64) {
        self.rest.insert(f.name().into(), json!(v));
    }
    fn record_i64(&mut self, f: &Field, v: i64) {
        self.rest.insert(f.name().into(), json!(v));
    }
    fn record_bool(&mut self, f: &Field, v: bool) {
        self.rest.insert(f.name().into(), json!(v));
    }
}

impl<S: Subscriber> Layer<S> for DiagLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let meta = event.metadata();
        let mut f = Fields::default();
        event.record(&mut f);
        f.rest.insert("target".into(), json!(meta.target()));
        f.rest.insert("msg".into(), json!(f.message));
        self.0.log(
            &meta.level().as_str().to_ascii_lowercase(),
            "log",
            Value::Object(f.rest),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn writes_json_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let d = Diag::open(tmp.path()).unwrap();
        d.log("info", "started", json!({"port": 7878, "event": "ignored"}));
        let text = fs::read_to_string(d.path()).unwrap();
        let v: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["event"], "started");
        assert_eq!(v["port"], 7878);
        assert!(v["ts"].as_u64().unwrap() > 0);
    }

    #[test]
    fn rotates_and_keeps_two_files() {
        let tmp = tempfile::tempdir().unwrap();
        let d = Diag::open_with(tmp.path(), 500).unwrap();
        for i in 0..100 {
            d.log("info", "tick", json!({"i": i, "pad": "x".repeat(20)}));
        }
        let dir = tmp.path().join("diagnostics");
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, ["cmuxd.jsonl", "cmuxd.jsonl.1"]);
        for n in &names {
            assert!(fs::metadata(dir.join(n)).unwrap().len() <= 500, "{n}");
        }
        // The newest line is in the current file.
        let cur = fs::read_to_string(dir.join("cmuxd.jsonl")).unwrap();
        assert!(cur.contains("\"i\":99"));
    }

    #[test]
    fn tracing_events_land_in_the_log() {
        let tmp = tempfile::tempdir().unwrap();
        let d = Diag::open(tmp.path()).unwrap();
        let sub = tracing_subscriber::registry().with(DiagLayer(d.clone()));
        tracing::subscriber::with_default(sub, || {
            tracing::warn!(plan = "P-1", n = 3u64, "slow disk");
        });
        let text = fs::read_to_string(d.path()).unwrap();
        let v: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(v["level"], "warn");
        assert_eq!(v["msg"], "slow disk");
        assert_eq!(v["plan"], "P-1");
        assert_eq!(v["n"], 3);
    }
}
