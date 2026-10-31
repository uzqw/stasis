//! Bridge from rest-break's decision to a Stasis rest request.
//!
//! rest-break owns the fatigue model and already writes a status snapshot
//! (`rest-break --check`).  Its scheduling path talks to aide tools
//! (`request_rest` / `get_rest_sessions`) that the deployed aide build does
//! not expose, so until that is unified this subcommand turns the snapshot
//! into the same `rest.requested` event `stasis rest` writes.
//!
//! All decisions stay in rest-break: this module only checks that the
//! snapshot is fresh and due, that Stasis is not already resting, and that
//! the cooldown since the last rest has passed.  It is meant to run from a
//! timer once a minute.

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde_json::Value;

/// rest-break writes once a minute; older than this means "not running".
pub const STALE_AFTER: Duration = Duration::from_secs(180);
/// Do not start another rest sooner than this after the previous one began.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(30 * 60);
pub const MAX_MINUTES: i64 = 240;

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub minutes: u32,
    pub reason: String,
}

/// Read rest-break's status file.  Missing, unreadable or non-object JSON
/// means "no data" (the timer stays quiet rather than guessing).
pub fn read_status(path: &Path) -> Option<Value> {
    let bytes = fs::read(path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.is_object().then_some(value)
}

/// Newest result event (mtime).  A rest writes `rest.locked` at its start,
/// so this doubles as "when did the last rest begin".
pub fn newest_result(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    for entry in fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if newest.is_none_or(|t| modified > t) {
            newest = Some(modified);
        }
    }
    newest
}

/// Decide whether this tick should request a rest.
///
/// `locked` covers both an active lock and a session that is waiting to
/// lock; `last_result` is when the previous rest started.
pub fn decide(
    status: &Value,
    locked: bool,
    last_result: Option<SystemTime>,
    now: SystemTime,
    cooldown: Duration,
) -> Option<Decision> {
    if locked || status.get("due").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let minutes = status.get("restMinutes").and_then(Value::as_i64)?;
    if minutes <= 0 {
        return None;
    }
    let checked = status.get("checkedAt").and_then(Value::as_str)?;
    let checked = DateTime::parse_from_rfc3339(checked)
        .ok()?
        .with_timezone(&Utc);
    let now_dt: DateTime<Utc> = now.into();
    let age = now_dt.signed_duration_since(checked);
    if age < chrono::Duration::zero() || age.num_seconds() as u64 > STALE_AFTER.as_secs() {
        return None;
    }
    if let Some(last) = last_result
        && now.duration_since(last).unwrap_or(Duration::ZERO) < cooldown
    {
        return None;
    }
    Some(Decision {
        minutes: minutes.min(MAX_MINUTES) as u32,
        reason: status
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn at(secs: i64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs((1_700_000_000 + secs) as u64)
    }

    fn snapshot(checked_secs: i64, due: bool, minutes: i64) -> Value {
        let checked: DateTime<Utc> = at(checked_secs).into();
        serde_json::json!({
            "state": "ok",
            "due": due,
            "restMinutes": minutes,
            "reason": "fatigue_threshold",
            "checkedAt": checked.to_rfc3339(),
        })
    }

    #[test]
    fn schedules_when_due_and_fresh() {
        let d = decide(
            &snapshot(100, true, 15),
            false,
            None,
            at(130),
            DEFAULT_COOLDOWN,
        )
        .expect("due snapshot should schedule");
        assert_eq!(d.minutes, 15);
        assert_eq!(d.reason, "fatigue_threshold");
    }

    #[test]
    fn ignores_not_due_and_zero_minutes() {
        let now = at(130);
        assert!(
            decide(
                &snapshot(100, false, 15),
                false,
                None,
                now,
                DEFAULT_COOLDOWN
            )
            .is_none()
        );
        assert!(decide(&snapshot(100, true, 0), false, None, now, DEFAULT_COOLDOWN).is_none());
        assert!(decide(&snapshot(100, true, -5), false, None, now, DEFAULT_COOLDOWN).is_none());
    }

    #[test]
    fn ignores_stale_or_future_snapshots() {
        let now = at(1000);
        // Older than STALE_AFTER.
        assert!(decide(&snapshot(0, true, 15), false, None, now, DEFAULT_COOLDOWN).is_none());
        // Clock skew: rest-break claims a future check time.
        assert!(
            decide(
                &snapshot(2000, true, 15),
                false,
                None,
                now,
                DEFAULT_COOLDOWN
            )
            .is_none()
        );
    }

    #[test]
    fn never_schedules_on_top_of_a_rest() {
        assert!(
            decide(
                &snapshot(100, true, 15),
                true,
                None,
                at(130),
                DEFAULT_COOLDOWN
            )
            .is_none()
        );
    }

    #[test]
    fn respects_the_cooldown_since_the_last_rest() {
        let status = snapshot(100, true, 15);
        let now = at(130);
        // Last rest began 5 minutes ago; the default cooldown is 30.
        assert!(decide(&status, false, Some(at(130 - 300)), now, DEFAULT_COOLDOWN).is_none());
        // 31 minutes later it may schedule again.
        assert!(
            decide(
                &status,
                false,
                Some(at(130 - 31 * 60)),
                now,
                DEFAULT_COOLDOWN
            )
            .is_some()
        );
    }

    #[test]
    fn minutes_are_capped() {
        let d = decide(
            &snapshot(100, true, 999),
            false,
            None,
            at(130),
            DEFAULT_COOLDOWN,
        )
        .expect("due");
        assert_eq!(d.minutes, 240);
    }

    #[test]
    fn newest_result_picks_the_latest_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), b"{}").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        fs::write(dir.path().join("b.json"), b"{}").unwrap();
        let newest = newest_result(dir.path()).expect("files exist");
        let a = fs::metadata(dir.path().join("a.json"))
            .unwrap()
            .modified()
            .unwrap();
        assert!(newest > a);
        assert!(newest_result(&dir.path().join("missing")).is_none());
    }
}
