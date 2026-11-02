//! Runtime status file shared with desktop shells (Omarchy bar widget).
//!
//! The engine owns the lock state in memory; a bar widget lives in a
//! different process (Quickshell) and cannot query it directly.  Writing a
//! small JSON snapshot next to the state directory gives any external UI a
//! stable, read-only view without adding an IPC daemon.
//!
//! The file is rewritten roughly once per second and on every state change.
//! Readers must treat a stale `updatedAt` as "daemon not running": the file
//! is never deleted, so a crashed process cannot look alive.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const SCHEMA: &str = "stasis.status/v1";

/// A rest session the engine currently owns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    /// `active` or `waiting`.
    pub phase: String,
    #[serde(rename = "lockAt")]
    pub lock_at: String,
    #[serde(rename = "unlockAt")]
    pub unlock_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub schema: String,
    /// Always true in a file the engine wrote; readers must still check
    /// freshness because a dead process leaves the last write behind.
    pub running: bool,
    pub pid: u32,
    pub locked: bool,
    #[serde(rename = "unlockMode")]
    pub unlock_mode: bool,
    #[serde(rename = "passwordLen")]
    pub password_len: usize,
    pub message: String,
    pub ok: bool,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(rename = "eventsDir")]
    pub events_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfo>,
    /// When the most recent rest ended.  The bar counts the current work
    /// stretch from here, so a rest visibly restarts the clock.
    #[serde(rename = "lastRestEnd", skip_serializing_if = "Option::is_none")]
    pub last_rest_end: Option<String>,
}

impl Status {
    /// Build a snapshot for the current engine state.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        locked: bool,
        unlock_mode: bool,
        password_len: usize,
        message: impl Into<String>,
        ok: bool,
        events_dir: &Path,
        session: Option<SessionInfo>,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            schema: SCHEMA.into(),
            running: true,
            pid: std::process::id(),
            locked,
            unlock_mode,
            password_len,
            message: message.into(),
            ok,
            updated_at: iso(now),
            events_dir: events_dir.display().to_string(),
            session,
            last_rest_end: None,
        }
    }

    /// Atomically replace the status file, owner-readable only.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&tmp)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&tmp, perms)?;
        }
        fs::rename(&tmp, path)
    }

    /// Read a status file.  A missing or malformed file is `None`.
    pub fn read(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// True when the engine wrote this file recently enough to be alive.
    pub fn is_fresh(&self, max_age: chrono::Duration) -> bool {
        let Ok(updated) = DateTime::parse_from_rfc3339(&self.updated_at) else {
            return false;
        };
        let age = Utc::now().signed_duration_since(updated.with_timezone(&Utc));
        age >= chrono::Duration::zero() && age <= max_age
    }
}

pub fn iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Standard location for the status file.
pub fn default_path() -> PathBuf {
    crate::config::Config::status_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample(now: DateTime<Utc>) -> Status {
        Status::new(
            true,
            false,
            3,
            "locked",
            true,
            Path::new("/tmp/events"),
            Some(SessionInfo {
                session_id: "s1".into(),
                phase: "active".into(),
                lock_at: iso(now),
                unlock_at: iso(now + chrono::Duration::minutes(10)),
            }),
            now,
        )
    }

    #[test]
    fn roundtrip_and_permissions() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("status.json");
        let status = sample(Utc::now());
        status.write(&path).unwrap();

        let back = Status::read(&path).unwrap();
        assert_eq!(back, status);
        assert_eq!(back.schema, SCHEMA);
        assert!(back.session.is_some());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn freshness_rejects_old_and_missing_files() {
        let now = Utc::now();
        let fresh = sample(now);
        assert!(fresh.is_fresh(chrono::Duration::seconds(5)));

        let stale = sample(now - chrono::Duration::minutes(5));
        assert!(!stale.is_fresh(chrono::Duration::seconds(5)));

        let tmp = TempDir::new().unwrap();
        assert!(Status::read(&tmp.path().join("absent.json")).is_none());
    }
}
