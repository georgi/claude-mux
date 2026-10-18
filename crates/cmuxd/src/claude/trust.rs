//! Marking an agent's working directory trusted in Claude's global config, so
//! its first start there does not stop at "Do you trust this folder?".
//!
//! The flag lives in `<config dir>/.claude.json` as
//! `projects["<abs path>"].hasTrustDialogAccepted`. `CLAUDE_CONFIG_DIR` moves the
//! file; without it the file is `~/.claude.json`.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Where Claude keeps its global config for a child that sees `config_dir` as
/// `CLAUDE_CONFIG_DIR` (when set and non-empty) and `home` as its home.
pub fn claude_json_path(config_dir: Option<&OsString>, home: &Path) -> PathBuf {
    match config_dir.filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join(".claude.json"),
        None => home.join(".claude.json"),
    }
}

/// Sets `projects[dir].hasTrustDialogAccepted = true` in the config file at
/// `file`, creating the file if it is missing. Every other key is kept. Nothing
/// is written when the flag is already true; returns whether a write happened.
/// An unreadable or malformed file is an error and is left alone.
pub fn trust_dir(file: &Path, dir: &Path) -> std::io::Result<bool> {
    // Claude keys projects by its real working directory.
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let key = dir.to_string_lossy().into_owned();
    // Write through a symlinked config rather than replacing the link.
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());

    let mut root = match std::fs::read_to_string(&file) {
        Ok(text) if text.trim().is_empty() => Value::Object(Map::new()),
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(invalid)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(e),
    };
    let Some(top) = root.as_object_mut() else {
        return Err(invalid("top level is not an object"));
    };
    let projects = top
        .entry("projects")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| invalid("\"projects\" is not an object"))?;
    let project = projects
        .entry(key)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| invalid("a project entry is not an object"))?;
    if project.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)) {
        return Ok(false);
    }
    project.insert("hasTrustDialogAccepted".into(), Value::Bool(true));

    let mut text = serde_json::to_string_pretty(&root).map_err(invalid)?;
    text.push('\n');
    write_keeping_mode(&file, text.as_bytes())?;
    Ok(true)
}

/// Trusts the spawn's working directory in the config file the child will use
/// (`CLAUDE_CONFIG_DIR` as the child sees it, given the inherited `base_env`).
/// A failure is logged and otherwise ignored: the worst case is the dialog.
pub fn pre_trust(
    spec: &super::spawn::SpawnSpec,
    base_env: impl IntoIterator<Item = (String, String)>,
    home: &Path,
) {
    let env = spec.final_env(base_env);
    let dir = env
        .iter()
        .find(|(k, _)| k == "CLAUDE_CONFIG_DIR")
        .map(|(_, v)| OsString::from(v));
    let file = claude_json_path(dir.as_ref(), home);
    match trust_dir(&file, &spec.cwd) {
        Ok(true) => {
            tracing::info!(target: "agents", "trusted {} in {}", spec.cwd.display(), file.display())
        }
        Ok(false) => {}
        Err(e) => tracing::warn!(
            target: "agents",
            "cannot pre-trust {} in {}: {e}",
            spec.cwd.display(),
            file.display()
        ),
    }
}

fn invalid(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e)
}

/// Temp file in the same directory, then a rename. The file keeps the mode of
/// the one it replaces (0600 for a new one: the config holds credentials).
fn write_keeping_mode(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = dir.join(format!(".{name}.cmuxd-{}.tmp", std::process::id()));
    let result = (|| {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o7777)
                .unwrap_or(0o600);
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests;
