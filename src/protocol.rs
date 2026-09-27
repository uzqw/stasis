use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

pub const SCHEMA: &str = "input-locker.event/v1";
pub const REQUESTED: &str = "rest.requested";
pub const LOCKED: &str = "rest.locked";
pub const OBSERVED: &str = "rest.observed";
pub const UNLOCKED: &str = "rest.unlocked";
pub const FAILED: &str = "rest.failed";
pub const EXPIRED: &str = "rest.expired";

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
    /// Files dropped by the last capped read, so the warning fires on change
    /// instead of once per tick (it is read ~1/s while locked).
    truncated_at: AtomicUsize,
    /// Parsed events keyed by path, invalidated on mtime or size change.
    /// `events()` runs once per engine tick over directories that can hold
    /// 1000+ files; without this cache every tick re-opens and re-parses all
    /// of them, which AV scanning can stretch into seconds on Windows.
    /// Entries for deleted files linger (a few KB); filenames are random
    /// UUIDs, so a stale entry can never alias a new event.
    cache: Mutex<HashMap<PathBuf, CacheEntry>>,
}

/// Cached parse result for one event file.
type CacheEntry = (SystemTime, u64, Option<Event>);

impl EventStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            truncated_at: AtomicUsize::new(0),
            cache: Mutex::new(HashMap::new()),
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

    /// Read the newest valid events from one directory.
    ///
    /// Over-long directories are capped to the most recent [`MAX_READ_BATCH`]
    /// files **by modification time**. Event filenames are random UUIDs, so a
    /// cap by filename keeps an arbitrary half of the history: recent events
    /// (including the one just written) may be invisible to projection, which
    /// makes lock/unlock decisions nondeterministic.
    fn read_dir(&self, dir: &Path) -> Vec<Event> {
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return out;
        };
        let mut paths: Vec<(SystemTime, u64, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some((meta.modified().ok()?, meta.len(), e.path()))
            })
            .collect();
        paths.sort();

        let total = paths.len();
        if total > MAX_READ_BATCH {
            let dropped = total - MAX_READ_BATCH;
            if self.truncated_at.swap(dropped, Ordering::Relaxed) != dropped {
                tracing::warn!(
                    "event directory {} holds {} files; keeping the newest {} and dropping {}",
                    dir.display(),
                    total,
                    MAX_READ_BATCH,
                    dropped
                );
            }
            paths.drain(..dropped);
        }

        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        for (mtime, len, path) in paths {
            let ev = match cache.get(&path) {
                Some((t, l, cached)) if *t == mtime && *l == len => cached.clone(),
                _ => {
                    let parsed = Self::read_one(&path).ok().filter(|e| e.is_valid());
                    cache.insert(path.clone(), (mtime, len, parsed.clone()));
                    parsed
                }
            };
            if let Some(ev) = ev {
                out.push(ev);
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

    /// Directory budgets for the non-compactable files.  [`MAX_READ_BATCH`]
    /// caps one read, so the on-disk count is budgeted back below it; a
    /// directory that only ever grew would eventually have the read cap drop a
    /// live `rest.locked` and leave the machine locked past its schedule.
    const PRUNE_TRIGGER: usize = MAX_READ_BATCH;
    const PRUNE_TARGET: usize = 300;

    /// Bound `dir`, in two stages:
    ///
    /// 1. **Compact** the periodic `rest.observed` heartbeat: a session keeps
    ///    only its newest one.  The projection only ever extends
    ///    `lockedThrough` from the latest heartbeat, so older files are no-ops
    ///    — the same "keep the latest value per key" contract as a Kafka
    ///    compacted topic.  This is what stops one long rest from flooding the
    ///    directory (and the read cap from ever seeing it).
    /// 2. **Budget** what is left: past [`Self::PRUNE_TRIGGER`], drop the
    ///    oldest files down to [`Self::PRUNE_TARGET`].
    ///
    /// A file whose session is in `keep` is never budgeted away: the
    /// projection replays the directories, so dropping a live `rest.locked`
    /// (or the `rest.requested` behind it) would make the engine forget a lock
    /// it owns.  Oldest-first also keeps the recent finished sessions that
    /// rest-break reads through aide for its fatigue and cooldown windows.
    pub fn prune(&self, dir: &Path, keep: &HashSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<(SystemTime, PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
            // A writer's in-flight temp file is not a finished event.
            .filter(|e| !e.file_name().to_string_lossy().starts_with(".tmp-"))
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        paths.sort();

        struct Candidate {
            path: PathBuf,
            session: Option<String>,
            observed: bool,
        }
        let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
        let candidates: Vec<Candidate> = paths
            .into_iter()
            .map(|(_mtime, path)| {
                let event = match cache.get(&path) {
                    Some((_, _, cached)) => cached.clone(),
                    None => Self::read_one(&path).ok().filter(|e| e.is_valid()),
                };
                Candidate {
                    session: event.as_ref().map(|e| e.session_id.clone()),
                    observed: event.as_ref().is_some_and(|e| e.kind == OBSERVED),
                    path,
                }
            })
            .collect();

        // Stage 1: every heartbeat but the newest one of its session.
        let mut drop = vec![false; candidates.len()];
        let mut newest: HashMap<&str, usize> = HashMap::new();
        for (i, c) in candidates.iter().enumerate() {
            if c.observed
                && let Some(session) = c.session.as_deref()
                && let Some(previous) = newest.insert(session, i)
            {
                drop[previous] = true;
            }
        }
        let compacted = drop.iter().filter(|d| **d).count();

        // Stage 2: still too many?  Drop the oldest sessions, `keep` intact.
        let remaining = candidates.len() - compacted;
        if remaining > Self::PRUNE_TRIGGER {
            let mut excess = remaining - Self::PRUNE_TARGET;
            for (i, c) in candidates.iter().enumerate() {
                if excess == 0 {
                    break;
                }
                if drop[i] || c.session.as_deref().is_some_and(|s| keep.contains(s)) {
                    continue;
                }
                drop[i] = true;
                excess -= 1;
            }
        }

        let mut removed = 0;
        for (i, c) in candidates.iter().enumerate() {
            if drop[i] && fs::remove_file(&c.path).is_ok() {
                cache.remove(&c.path);
                removed += 1;
            }
        }
        if removed > 0 {
            tracing::info!(
                "pruned {removed} events from {} ({compacted} superseded heartbeats)",
                dir.display()
            );
        }
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

/// Write the ack record next to the command file, atomically.
fn write_ack(dir: &Path, cmd: &Command, result: String, error: Option<String>) -> io::Result<()> {
    let ack = Ack {
        cmd: cmd.cmd.clone(),
        id: cmd.id.clone(),
        result,
        error,
        ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    let tmp = dir.join(".tmp-ack.json");
    fs::write(&tmp, serde_json::to_string_pretty(&ack)?)?;
    fs::rename(&tmp, dir.join("input-locker-ack.json"))
}

/// Check for a pending command file, atomically claim it, execute it,
/// and write the ack.  A stale `.processing` left by a previous crash is
/// treated as claimed and re-executed idempotently.
pub fn poll_command(
    dir: impl AsRef<Path>,
    mut handler: impl FnMut(&Command) -> anyhow::Result<String>,
) -> anyhow::Result<()> {
    let dir = dir.as_ref();
    let cmd_path = dir.join("input-locker-command.json");
    let processing = dir.join("input-locker-command.json.processing");

    // Atomic claim: fresh command file, or stale .processing from a crash.
    if !processing.exists() {
        match fs::rename(&cmd_path, &processing) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        }
    }

    if let Ok(text) = fs::read_to_string(&processing)
        && let Ok(cmd) = serde_json::from_str::<Command>(&text)
    {
        let (result, error) = match handler(&cmd) {
            Ok(r) => (r, None),
            Err(e) => ("error".into(), Some(e.to_string())),
        };
        write_ack(dir, &cmd, result, error)?;
    }

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
    fn newest_events_survive_the_batch_cap() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        fs::create_dir_all(store.results_dir()).unwrap();

        // Fill the directory past the cap with older events.
        let old = concat!(
            r#"{"schema":"input-locker.event/v1","eventId":"old","sessionId":"s1","#,
            r#""type":"rest.observed","recordedAt":"2026-09-08T03:00:00Z","data":{}}"#
        );
        for i in 0..MAX_READ_BATCH + 20 {
            fs::write(store.results_dir().join(format!("{i:040}.json")), old).unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(30));

        // The event just written must be visible: filenames are random UUIDs in
        // production, so a name-ordered cap would drop it at random.
        let now = Utc::now();
        store
            .emit_result("s1", LOCKED, json!({}), now, None)
            .unwrap();

        let evs = store.events();
        assert_eq!(evs.len(), MAX_READ_BATCH);
        assert!(evs.iter().any(|e| e.kind == LOCKED));
    }

    #[test]
    fn prune_compacts_superseded_heartbeats() {
        // One rest emits a `rest.observed` every 30 s; only the newest one is
        // ever read, so the rest must be compacted away.
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        fs::create_dir_all(store.results_dir()).unwrap();
        let event = |id: &str, kind: &str, sid: &str| {
            format!(
                concat!(
                    r#"{{"schema":"input-locker.event/v1","eventId":"{}","#,
                    r#""sessionId":"{}","type":"{}","#,
                    r#""recordedAt":"2026-09-08T03:00:00Z","data":{{}}}}"#
                ),
                id, sid, kind
            )
        };
        for i in 0..MAX_READ_BATCH + 20 {
            let path = store.results_dir().join(format!("o{i:040}.json"));
            fs::write(path, event(&format!("o{i}"), OBSERVED, "s1")).unwrap();
        }
        fs::write(
            store.results_dir().join("locked.json"),
            event("locked", LOCKED, "s1"),
        )
        .unwrap();

        store.prune(&store.results_dir(), &HashSet::new());

        let names: Vec<String> = fs::read_dir(store.results_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "heartbeats collapse to one: {names:?}");
        // The newest heartbeat (highest index, sorted last) is the survivor.
        assert!(names.contains(&format!("o{:040}.json", MAX_READ_BATCH + 19)));
        assert!(names.contains(&"locked.json".to_string()));
    }

    #[test]
    fn prune_budgets_old_sessions_but_keeps_live() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        fs::create_dir_all(store.results_dir()).unwrap();
        let event = |sid: &str| {
            format!(
                concat!(
                    r#"{{"schema":"input-locker.event/v1","eventId":"{}","#,
                    r#""sessionId":"{}","type":"rest.locked","#,
                    r#""recordedAt":"2026-09-08T03:00:00Z","data":{{}}}}"#
                ),
                Uuid::new_v4(),
                sid
            )
        };
        // File 0 is the oldest and belongs to the session still holding the lock.
        for i in 0..MAX_READ_BATCH + 20 {
            let sid = if i == 0 { "live" } else { "done" };
            fs::write(
                store.results_dir().join(format!("{i:040}.json")),
                event(sid),
            )
            .unwrap();
        }
        let keep: HashSet<String> = ["live".to_string()].into_iter().collect();
        store.prune(&store.results_dir(), &keep);

        let names: Vec<String> = fs::read_dir(store.results_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 300);
        assert!(
            names.contains(&format!("{:040}.json", 0)),
            "the live session's oldest file must survive the prune"
        );
    }

    #[test]
    fn changed_event_file_is_reloaded() {
        // events() caches parsed files by (mtime, len); a rewritten file must
        // be re-read instead of serving the stale event.
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        fs::create_dir_all(store.results_dir()).unwrap();
        let path = store.results_dir().join("ev.json");
        let mk = |kind: &str, pad: &str| {
            format!(
                concat!(
                    r#"{{"schema":"input-locker.event/v1","eventId":"e1","#,
                    r#""sessionId":"s1","type":"{}","#,
                    r#""recordedAt":"2026-09-08T03:00:00Z","#,
                    r#""data":{{"pad":"{}"}}}}"#
                ),
                kind, pad
            )
        };
        fs::write(&path, mk(LOCKED, "")).unwrap();
        assert_eq!(store.events()[0].kind, LOCKED);

        std::thread::sleep(std::time::Duration::from_millis(30));
        fs::write(&path, mk(OBSERVED, "longer")).unwrap();
        let evs = store.events();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, OBSERVED);
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
