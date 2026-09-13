use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

#[cfg(not(test))]
use crossbeam_channel::bounded;
#[cfg(not(test))]
use std::thread;
use std::time::Duration;

pub const SCHEMA: &str = "input-locker.event/v1";
pub const REQUESTED: &str = "rest.requested";
pub const LOCKED: &str = "rest.locked";
pub const OBSERVED: &str = "rest.observed";
pub const UNLOCKED: &str = "rest.unlocked";
pub const FAILED: &str = "rest.failed";
pub const EXPIRED: &str = "rest.expired";
pub const TERMINAL: &[&str] = &[UNLOCKED, FAILED, EXPIRED];

/// Immutable event on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub schema: String,
    #[serde(rename = "eventId")]
    pub event_id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "recordedAt")]
    pub recorded_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "causationId")]
    pub causation_id: Option<String>,
    pub data: JsonValue,
}

impl Event {
    pub fn new(
        session_id: impl Into<String>,
        kind: impl Into<String>,
        data: JsonValue,
        now: DateTime<Utc>,
        causation_id: Option<String>,
    ) -> Self {
        Self {
            schema: SCHEMA.into(),
            event_id: Uuid::new_v4().to_string().replace('-', ""),
            session_id: session_id.into(),
            kind: kind.into(),
            recorded_at: now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            causation_id,
            data,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.schema.is_empty()
            && !self.event_id.is_empty()
            && !self.session_id.is_empty()
            && !self.kind.is_empty()
            && !self.recorded_at.is_empty()
    }
}

const MAX_READ_BATCH: usize = 500;

/// Event file store (requests + results).
pub struct EventStore {
    root: PathBuf,
}

impl EventStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
        }
    }

    pub fn requests_dir(&self) -> PathBuf {
        self.root.join("requests")
    }

    pub fn results_dir(&self) -> PathBuf {
        self.root.join("results")
    }

    pub fn ensure_dirs(&self) -> io::Result<()> {
        fs::create_dir_all(self.requests_dir())?;
        fs::create_dir_all(self.results_dir())?;
        Ok(())
    }

    /// Read all valid events from requests and results, sorted by filename.
    pub fn events(&self) -> Vec<Event> {
        let mut out = Vec::new();
        out.extend(self.read_dir(&self.requests_dir()));
        out.extend(self.read_dir(&self.results_dir()));
        out
    }

    /// Read events with a timeout to prevent blocking the engine thread
    /// indefinitely on slow or unresponsive storage.
    #[cfg(not(test))]
    pub fn events_nonblocking(&self, timeout: Duration) -> Vec<Event> {
        let root = self.root.clone();
        let (tx, rx) = bounded(1);
        thread::spawn(move || {
            let store = EventStore::new(&root);
            let _ = tx.send(store.events());
        });
        match rx.recv_timeout(timeout) {
            Ok(events) => events,
            Err(_) => {
                tracing::warn!(
                    "EventStore::events() timed out after {:?} on {}; returning empty",
                    timeout,
                    self.root.display()
                );
                Vec::new()
            }
        }
    }

    #[cfg(test)]
    pub fn events_nonblocking(&self, _timeout: Duration) -> Vec<Event> {
        self.events()
    }

    fn read_dir(&self, dir: &Path) -> Vec<Event> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        let mut paths: Vec<_> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
        paths.sort();

        let total = paths.len();
        if total > MAX_READ_BATCH {
            tracing::warn!(
                "event directory {} contains {} files; limiting to last {}",
                dir.display(),
                total,
                MAX_READ_BATCH
            );
            paths = paths.split_off(total - MAX_READ_BATCH);
        }

        for path in paths {
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            match Self::read_one(&path) {
                Ok(ev) if ev.is_valid() => out.push(ev),
                _ => {}
            }
        }
        out
    }

    const MAX_EVENT_SIZE: u64 = 1024 * 1024; // 1 MiB

    fn read_one(path: &Path) -> anyhow::Result<Event> {
        let meta = fs::metadata(path)?;
        if meta.len() > Self::MAX_EVENT_SIZE {
            anyhow::bail!("event file too large ({} bytes)", meta.len());
        }
        let text = fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Atomically write an event into the given directory.
    pub fn emit(
        &self,
        dir: &Path,
        session_id: impl Into<String>,
        kind: impl Into<String>,
        data: JsonValue,
        now: DateTime<Utc>,
        causation_id: Option<String>,
    ) -> io::Result<Event> {
        fs::create_dir_all(dir)?;
        let event = Event::new(session_id, kind, data, now, causation_id);
        let filename = format!("{}.json", event.event_id);
        let path = dir.join(&filename);
        let tmp = dir.join(format!(".tmp-{}", filename));
        let text = serde_json::to_string_pretty(&event).unwrap_or_default();
        fs::write(&tmp, text)?;
        // Best-effort fsync; ignore failure.
        let _ = Self::fsync_parent(&tmp);
        fs::rename(&tmp, &path)?;
        let _ = Self::fsync_parent(&path);
        Ok(event)
    }

    pub fn emit_result(
        &self,
        session_id: impl Into<String>,
        kind: impl Into<String>,
        data: JsonValue,
        now: DateTime<Utc>,
        causation_id: Option<String>,
    ) -> io::Result<Event> {
        self.emit(
            &self.results_dir(),
            session_id,
            kind,
            data,
            now,
            causation_id,
        )
    }

    fn fsync_parent(path: &Path) -> io::Result<()> {
        let parent = path.parent().unwrap_or(Path::new("."));
        let file = fs::File::open(parent)?;
        file.sync_all()
    }
}

// ------------------------------------------------------------------
// Command / ack file protocol (compatibility with aide)
// ------------------------------------------------------------------

/// Command record found on disk.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Command {
    pub cmd: String,
    pub id: String,
}

/// Ack record written back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Ack {
    pub cmd: String,
    pub id: String,
    pub result: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(rename = "ts")]
    pub ts: String,
}

/// Check for a pending command file, atomically move it to .processing,
/// execute it, and write ack.
pub fn poll_command(
    dir: impl AsRef<Path>,
    mut handler: impl FnMut(&Command) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    let dir = dir.as_ref();
    let cmd_path = dir.join("input-locker-command.json");
    let processing = dir.join("input-locker-command.json.processing");

    // Crash recovery: stale .processing from a previous crash
    if processing.exists() {
        let text = fs::read_to_string(&processing).unwrap_or_default();
        if let Ok(cmd) = serde_json::from_str::<Command>(&text) {
            let (result, error) = match handler(&cmd) {
                Ok(r) => (r, None),
                Err(e) => ("error".into(), Some(e.to_string())),
            };
            let ack = Ack {
                cmd: cmd.cmd.clone(),
                id: cmd.id.clone(),
                result,
                error,
                ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            };
            let ack_path = dir.join("input-locker-ack.json");
            let tmp = dir.join(".tmp-ack.json");
            if fs::write(&tmp, serde_json::to_string_pretty(&ack)?).is_ok() {
                let _ = fs::rename(&tmp, &ack_path);
            }
        }
        let _ = fs::remove_file(&processing);
        return Ok(());
    }

    // Atomic claim
    match fs::rename(&cmd_path, &processing) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    }

    let text = fs::read_to_string(&processing)?;
    let cmd: Command = serde_json::from_str(&text)?;

    let (result, error) = match handler(&cmd) {
        Ok(r) => (r, None),
        Err(e) => ("error".into(), Some(e.to_string())),
    };

    let ack = Ack {
        cmd: cmd.cmd.clone(),
        id: cmd.id.clone(),
        result,
        error,
        ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };

    let ack_path = dir.join("input-locker-ack.json");
    let tmp = dir.join(".tmp-ack.json");
    fs::write(&tmp, serde_json::to_string_pretty(&ack)?)?;
    fs::rename(&tmp, &ack_path)?;
    let _ = fs::remove_file(&processing);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn event_roundtrip() {
        let ev = Event::new("s1", REQUESTED, json!({"k":1}), Utc::now(), None);
        let s = serde_json::to_string(&ev).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(back.event_id, ev.event_id);
        assert_eq!(back.session_id, "s1");
        assert_eq!(back.kind, REQUESTED);
    }

    #[test]
    fn store_emit_and_read() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let now = Utc::now();
        store
            .emit_result("s1", LOCKED, json!({}), now, None)
            .unwrap();
        let evs = store.events();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, LOCKED);
    }

    #[test]
    fn invalid_event_ignored() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        fs::create_dir_all(store.requests_dir()).unwrap();
        fs::write(
            store.requests_dir().join("bad.json"),
            r#"{"schema":"bad","eventId":"","sessionId":"","type":"x","recordedAt":"","data":{}}"#,
        )
        .unwrap();
        let evs = store.events();
        assert!(evs.is_empty());
    }

    #[test]
    fn command_ack_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();

        fs::write(
            dir.join("input-locker-command.json"),
            r#"{"cmd":"lock","id":"abc"}"#,
        )
        .unwrap();

        poll_command(dir, |cmd| {
            assert_eq!(cmd.cmd, "lock");
            assert_eq!(cmd.id, "abc");
            Ok("ok".into())
        })
        .unwrap();

        let ack_text = fs::read_to_string(dir.join("input-locker-ack.json")).unwrap();
        let ack: Ack = serde_json::from_str(&ack_text).unwrap();
        assert_eq!(ack.id, "abc");
        assert_eq!(ack.result, "ok");
        assert!(!dir.join("input-locker-command.json.processing").exists());
    }

    #[test]
    fn command_not_found_is_noop() {
        let tmp = TempDir::new().unwrap();
        poll_command(tmp.path(), |_cmd| Ok("ok".into())).unwrap();
    }

    #[test]
    fn processing_crash_recovery() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path();
        fs::write(
            dir.join("input-locker-command.json.processing"),
            r#"{"cmd":"lock","id":"crash1"}"#,
        )
        .unwrap();

        poll_command(dir, |cmd| {
            assert_eq!(cmd.id, "crash1");
            Ok("recovered".into())
        })
        .unwrap();

        let ack_text = fs::read_to_string(dir.join("input-locker-ack.json")).unwrap();
        let ack: Ack = serde_json::from_str(&ack_text).unwrap();
        assert_eq!(ack.id, "crash1");
        assert_eq!(ack.result, "recovered");
        assert!(!dir.join("input-locker-command.json.processing").exists());
    }

    #[test]
    fn oversized_event_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        fs::create_dir_all(store.requests_dir()).unwrap();
        let big = "x".repeat(2 * 1024 * 1024);
        let payload = format!(
            r#"{{"schema":"input-locker.event/v1","eventId":"e1",\
"sessionId":"s1","type":"rest.requested","recordedAt":\
"2026-09-08T03:00:00Z","data":{{"big":"{big}"}}}}"#,
        );
        fs::write(store.requests_dir().join("huge.json"), payload).unwrap();
        let evs = store.events();
        assert!(evs.is_empty());
    }

    #[test]
    fn bad_schema_event_is_skipped() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        fs::create_dir_all(store.requests_dir()).unwrap();
        // Type error in a core field: "eventId" as number instead of string.
        fs::write(
            store.requests_dir().join("type_err.json"),
            r#"{"schema":"input-locker.event/v1","eventId":123,\
"sessionId":"s1","type":"rest.requested","recordedAt":\
"2026-09-08T03:00:00Z","data":{}}"#,
        )
        .unwrap();
        let evs = store.events();
        // Fails deserialization and is silently skipped
        assert!(evs.is_empty());
    }
}
