//! Read Stasis's event store to learn which rests actually happened.
//!
//! `--check` used to run without session knowledge, so a locked rest counted
//! as ordinary AFK (0.5x recovery) — or, when it was shorter than the AFK
//! timeout, as work.  The event store is the same protocol the scheduler
//! consumes, so read it directly instead of requiring aide's
//! `get_rest_sessions` endpoint, which the deployed build does not expose.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{iso, parse_time};

/// `STASIS_EVENTS_DIR` → `INPUT_LOCKER_EVENTS_DIR` → `$XDG_DATA_HOME/stasis/events`.
pub fn events_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("STASIS_EVENTS_DIR")
        .or_else(|| std::env::var_os("INPUT_LOCKER_EVENTS_DIR"))
    {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from(".local/share"));
    base.join("stasis").join("events")
}

struct Session {
    id: String,
    lock_at: Option<String>,
    unlock_at: Option<String>,
    phase: &'static str,
    last_error: String,
    segments: Vec<Value>,
}

/// Project `rest.requested` / `rest.locked` / `rest.observed` /
/// `rest.unlocked` into the session shape `observed_rest_intervals` expects.
/// Unreadable or unrelated files are skipped: this is a read-only fallback,
/// not a validator.
pub fn read_sessions(dir: &Path) -> Vec<Value> {
    let mut events = Vec::new();
    for sub in ["requests", "results"] {
        let Ok(entries) = fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(bytes) = fs::read(entry.path()) else {
                continue;
            };
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            let Some(session_id) = value.get("sessionId").and_then(Value::as_str) else {
                continue;
            };
            let Some(kind) = value.get("type").and_then(Value::as_str) else {
                continue;
            };
            let Some(recorded) = value
                .get("recordedAt")
                .and_then(Value::as_str)
                .and_then(|s| parse_time(s).ok())
            else {
                continue;
            };
            events.push((
                recorded,
                session_id.to_string(),
                kind.to_string(),
                value.get("data").cloned().unwrap_or(Value::Null),
            ));
        }
    }
    events.sort_by_key(|e| e.0);

    let mut sessions: BTreeMap<String, Session> = BTreeMap::new();
    for (recorded, session_id, kind, data) in events {
        let at = iso(recorded);
        let session = sessions
            .entry(session_id.clone())
            .or_insert_with(|| Session {
                id: session_id,
                lock_at: None,
                unlock_at: None,
                phase: "waiting",
                last_error: String::new(),
                segments: Vec::new(),
            });
        match kind.as_str() {
            "rest.requested" => {
                session.lock_at = data
                    .get("lockAt")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                session.unlock_at = data
                    .get("unlockAt")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                session.phase = "waiting";
            }
            "rest.locked" => {
                let locked_at = data
                    .get("lockedAt")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| at.clone());
                let through = data
                    .get("lockedThrough")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| locked_at.clone());
                session.segments.push(json!({
                    "lockedAt": locked_at,
                    "lockedThrough": through,
                    "unlockedAt": Value::Null,
                }));
                session.phase = "active";
            }
            "rest.observed" => {
                if let Some(through) = data.get("lockedThrough").and_then(Value::as_str)
                    && let Some(segment) = session.segments.last_mut()
                {
                    let previous = segment
                        .get("lockedThrough")
                        .and_then(Value::as_str)
                        .and_then(|s| parse_time(s).ok());
                    let incoming = parse_time(through).ok();
                    if let (Some(previous), Some(incoming)) = (previous, incoming)
                        && incoming > previous
                    {
                        segment["lockedThrough"] = json!(through);
                    }
                }
                // A heartbeat must not revive a session that already ended.
                if session.phase == "waiting" || session.phase == "active" {
                    session.phase = "active";
                }
            }
            "rest.unlocked" => {
                let unlocked = data
                    .get("unlockedAt")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| at.clone());
                let through = data
                    .get("lockedThrough")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| unlocked.clone());
                if let Some(segment) = session.segments.last_mut()
                    && segment.get("unlockedAt").is_none_or(Value::is_null)
                {
                    segment["lockedThrough"] = json!(through);
                    segment["unlockedAt"] = json!(unlocked);
                }
                session.phase = "ended";
            }
            "rest.failed" => {
                session.last_error = data
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("lock failed")
                    .to_string();
                if !data
                    .get("retryable")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    session.phase = "failed";
                }
            }
            "rest.expired" => session.phase = "ended",
            _ => {}
        }
    }

    sessions
        .into_values()
        .map(|s| {
            json!({
                "id": s.id,
                "phase": s.phase,
                "lockAt": s.lock_at,
                "unlockAt": s.unlock_at,
                "lastError": s.last_error,
                "segments": s.segments,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, sub: &str, name: &str, value: Value) {
        fs::create_dir_all(dir.join(sub)).unwrap();
        fs::write(
            dir.join(sub).join(name),
            serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
    }

    fn event(session: &str, kind: &str, recorded: &str, data: Value) -> Value {
        json!({
            "schema": "input-locker.event/v1",
            "eventId": format!("{session}-{kind}"),
            "sessionId": session,
            "type": kind,
            "recordedAt": recorded,
            "data": data,
        })
    }

    #[test]
    fn a_locked_rest_becomes_a_closed_segment() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "requests",
            "a.json",
            event(
                "s1",
                "rest.requested",
                "2026-11-02T01:00:00Z",
                json!({"lockAt": "2026-11-02T01:00:00Z", "unlockAt": "2026-11-02T01:10:00Z"}),
            ),
        );
        write(
            dir.path(),
            "results",
            "b.json",
            event(
                "s1",
                "rest.locked",
                "2026-11-02T01:00:01Z",
                json!({"lockedAt": "2026-11-02T01:00:01Z", "lockedThrough": "2026-11-02T01:00:01Z"}),
            ),
        );
        write(
            dir.path(),
            "results",
            "c.json",
            event(
                "s1",
                "rest.observed",
                "2026-11-02T01:05:00Z",
                json!({"lockedThrough": "2026-11-02T01:05:00Z"}),
            ),
        );
        write(
            dir.path(),
            "results",
            "d.json",
            event(
                "s1",
                "rest.unlocked",
                "2026-11-02T01:10:02Z",
                json!({"unlockedAt": "2026-11-02T01:10:02Z", "lockedThrough": "2026-11-02T01:10:02Z"}),
            ),
        );

        let sessions = read_sessions(dir.path());
        assert_eq!(sessions.len(), 1);
        let s = &sessions[0];
        assert_eq!(s["id"], "s1");
        assert_eq!(s["phase"], "ended");
        assert_eq!(s["segments"][0]["lockedAt"], "2026-11-02T01:00:01Z");
        assert_eq!(s["segments"][0]["lockedThrough"], "2026-11-02T01:10:02Z");
        assert_eq!(s["segments"][0]["unlockedAt"], "2026-11-02T01:10:02Z");
    }

    #[test]
    fn a_request_alone_stays_waiting() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "requests",
            "a.json",
            event(
                "s2",
                "rest.requested",
                "2026-11-02T01:00:00Z",
                json!({"lockAt": "2026-11-02T01:00:00Z", "unlockAt": "2026-11-02T01:10:00Z"}),
            ),
        );
        let sessions = read_sessions(dir.path());
        assert_eq!(sessions[0]["phase"], "waiting");
        assert_eq!(sessions[0]["segments"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn an_observed_after_unlock_does_not_revive_the_session() {
        let dir = tempfile::tempdir().unwrap();
        for (sub, name, kind, recorded, data) in [
            (
                "results",
                "a.json",
                "rest.locked",
                "2026-11-02T01:00:01Z",
                json!({}),
            ),
            (
                "results",
                "b.json",
                "rest.unlocked",
                "2026-11-02T01:10:00Z",
                json!({}),
            ),
            (
                "results",
                "c.json",
                "rest.observed",
                "2026-11-02T01:10:01Z",
                json!({"lockedThrough": "2026-11-02T01:10:01Z"}),
            ),
        ] {
            write(dir.path(), sub, name, event("s3", kind, recorded, data));
        }
        assert_eq!(read_sessions(dir.path())[0]["phase"], "ended");
    }

    #[test]
    fn failed_sessions_record_the_error() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "results",
            "a.json",
            event(
                "s4",
                "rest.failed",
                "2026-11-02T01:00:00Z",
                json!({"error": "no input devices", "retryable": false}),
            ),
        );
        let s = &read_sessions(dir.path())[0];
        assert_eq!(s["phase"], "failed");
        assert_eq!(s["lastError"], "no input devices");
    }

    #[test]
    fn a_missing_directory_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_sessions(&dir.path().join("nope")).is_empty());
    }
}
