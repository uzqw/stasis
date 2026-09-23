use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use serde_json::json;

use crate::protocol::{EXPIRED, Event, EventStore, FAILED, LOCKED, OBSERVED, REQUESTED, UNLOCKED};

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn iso(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    Waiting,
    Active,
    Ended,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub locked_at: DateTime<Utc>,
    pub locked_through: DateTime<Utc>,
    pub unlocked_at: Option<DateTime<Utc>>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub session_id: String,
    pub lock_at: DateTime<Utc>,
    pub unlock_at: DateTime<Utc>,
    pub phase: Phase,
    pub segments: Vec<Segment>,
    pub last_error: String,
    pub updated_at: DateTime<Utc>,
}

fn priority(kind: &str) -> u8 {
    match kind {
        REQUESTED => 0,
        LOCKED => 1,
        OBSERVED => 2,
        UNLOCKED | FAILED | EXPIRED => 3,
        _ => 9,
    }
}

/// Replay all events into sessions.
pub fn project(events: &[Event], now: DateTime<Utc>) -> HashMap<String, Session> {
    let mut ordered: Vec<_> = events.iter().collect();
    ordered.sort_by(|a, b| {
        (&a.recorded_at, priority(&a.kind), &a.event_id).cmp(&(
            &b.recorded_at,
            priority(&b.kind),
            &b.event_id,
        ))
    });

    let mut sessions: HashMap<String, Session> = HashMap::new();

    for ev in ordered {
        let sid = ev.session_id.clone();
        match ev.kind.as_str() {
            REQUESTED => {
                if sessions.contains_key(&sid) {
                    continue;
                }
                let default = serde_json::Map::new();
                let data = ev.data.as_object().unwrap_or(&default);
                let lock_at = data
                    .get("lockAt")
                    .and_then(|v| v.as_str())
                    .and_then(parse_time)
                    .unwrap_or(now);
                let unlock_at = data
                    .get("unlockAt")
                    .and_then(|v| v.as_str())
                    .and_then(parse_time)
                    .unwrap_or(now);
                if lock_at >= unlock_at {
                    continue;
                }
                let updated_at = parse_time(&ev.recorded_at).unwrap_or(now);
                sessions.insert(
                    sid.clone(),
                    Session {
                        session_id: sid,
                        lock_at,
                        unlock_at,
                        phase: Phase::Waiting,
                        segments: Vec::new(),
                        last_error: String::new(),
                        updated_at,
                    },
                );
            }
            LOCKED => {
                let Some(s) = sessions.get_mut(&sid) else {
                    continue;
                };
                let default = serde_json::Map::new();
                let data = ev.data.as_object().unwrap_or(&default);
                let locked_at = data
                    .get("lockedAt")
                    .and_then(|v| v.as_str())
                    .and_then(parse_time)
                    .unwrap_or(s.updated_at);
                let locked_through = data
                    .get("lockedThrough")
                    .and_then(|v| v.as_str())
                    .and_then(parse_time)
                    .unwrap_or(locked_at);
                s.segments.push(Segment {
                    locked_at,
                    locked_through,
                    unlocked_at: None,
                    reason: None,
                });
                s.phase = Phase::Active;
                s.updated_at = parse_time(&ev.recorded_at).unwrap_or(now);
            }
            OBSERVED => {
                let Some(s) = sessions.get_mut(&sid) else {
                    continue;
                };
                let default = serde_json::Map::new();
                let data = ev.data.as_object().unwrap_or(&default);
                let through = data
                    .get("lockedThrough")
                    .and_then(|v| v.as_str())
                    .and_then(parse_time)
                    .unwrap_or(s.updated_at);
                // A heartbeat only extends the segment it belongs to.  It must
                // not revive a session that already ended: a stale `rest.observed`
                // delivered after `rest.unlocked` would otherwise flip the session
                // back to Active and make the engine re-lock after every unlock.
                if let Some(seg) = s.segments.last_mut()
                    && seg.unlocked_at.is_none()
                    && through > seg.locked_through
                {
                    seg.locked_through = through;
                    s.phase = Phase::Active;
                }
                s.updated_at = parse_time(&ev.recorded_at).unwrap_or(now);
            }
            UNLOCKED => {
                let Some(s) = sessions.get_mut(&sid) else {
                    continue;
                };
                let default = serde_json::Map::new();
                let data = ev.data.as_object().unwrap_or(&default);
                let unlocked = data
                    .get("unlockedAt")
                    .and_then(|v| v.as_str())
                    .and_then(parse_time)
                    .unwrap_or(s.updated_at);
                let through = data
                    .get("lockedThrough")
                    .and_then(|v| v.as_str())
                    .and_then(parse_time)
                    .unwrap_or(unlocked);
                let reason = data.get("reason").and_then(|v| v.as_str());
                if let Some(seg) = s.segments.last_mut()
                    && seg.unlocked_at.is_none()
                {
                    seg.locked_through = seg.locked_through.max(through);
                    seg.unlocked_at = Some(unlocked);
                    seg.reason = reason.map(|s| s.to_string());
                }
                s.phase = Phase::Ended;
                s.updated_at = parse_time(&ev.recorded_at).unwrap_or(now);
            }
            FAILED => {
                let Some(s) = sessions.get_mut(&sid) else {
                    continue;
                };
                let default = serde_json::Map::new();
                let data = ev.data.as_object().unwrap_or(&default);
                s.last_error = data
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("lock failed")
                    .to_string();
                if !data
                    .get("retryable")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    s.phase = Phase::Failed;
                }
                s.updated_at = parse_time(&ev.recorded_at).unwrap_or(now);
            }
            EXPIRED => {
                let Some(s) = sessions.get_mut(&sid) else {
                    continue;
                };
                s.phase = Phase::Skipped;
                s.updated_at = parse_time(&ev.recorded_at).unwrap_or(now);
            }
            _ => {}
        }
    }

    // Any still-waiting session whose window has passed is skipped.
    for s in sessions.values_mut() {
        if s.phase == Phase::Waiting && now >= s.unlock_at {
            s.phase = Phase::Skipped;
        }
    }

    sessions
}

/// Trait abstracting the physical lock/unlock operation.
pub trait LockOps {
    fn is_locked(&self) -> bool;
    fn start_lock(&mut self) -> anyhow::Result<()>;
    fn stop_lock(&mut self) -> anyhow::Result<()>;
}

/// Rest-session controller.  Owns the event store and schedule.
pub struct Controller {
    store: EventStore,
}

impl Controller {
    pub fn new(store: EventStore) -> Self {
        Self { store }
    }

    pub fn sessions(&self, now: DateTime<Utc>) -> HashMap<String, Session> {
        project(&self.store.events(), now)
    }

    /// Tick the schedule.  Returns zero or more status messages.
    pub fn tick(&mut self, locker: &mut impl LockOps, now: DateTime<Utc>) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        let sessions = self.sessions(now);
        let mut active: Vec<_> = sessions
            .values()
            .filter(|s| s.phase == Phase::Active)
            .cloned()
            .collect();
        active.sort_by_key(|s| s.lock_at);

        // Handle non-active sessions first (expired waiting requests)
        for s in sessions.values() {
            if s.phase == Phase::Waiting && now >= s.unlock_at {
                let _ = self.store.emit_result(
                    &s.session_id,
                    EXPIRED,
                    json!({"reason": "window_missed"}),
                    now,
                    None,
                );
            }
        }

        // Active sessions: reconcile desired state from events, not memory.
        for s in active {
            if !locker.is_locked() {
                if now < s.unlock_at {
                    // A session whose last segment is still open continues that
                    // segment.  Emitting another `rest.locked` here would invent a
                    // new rest segment — and when the backend grab flaps, one such
                    // event per tick.
                    let open_segment = s
                        .segments
                        .last()
                        .is_some_and(|seg| seg.unlocked_at.is_none());
                    if locker.start_lock().is_ok() {
                        if open_segment {
                            out.push(("计划休息已恢复锁定".into(), true));
                            continue;
                        }
                        match self.store.emit_result(
                            &s.session_id,
                            LOCKED,
                            json!({"lockedAt": iso(now), "lockedThrough": iso(now)}),
                            now,
                            None,
                        ) {
                            Ok(_) => out.push(("计划休息已恢复锁定".into(), true)),
                            Err(_) => {
                                let _ = locker.stop_lock();
                                let _ = self.store.emit_result(
                                    &s.session_id,
                                    FAILED,
                                    json!({
                                        "action": "lock",
                                        "error": "event write failed",
                                        "retryable": true,
                                    }),
                                    now,
                                    None,
                                );
                                out.push(("计划休息锁定失败".into(), false));
                            }
                        }
                    } else {
                        let _ = self.store.emit_result(
                            &s.session_id,
                            FAILED,
                            json!({"action": "lock", "error": "lock failed", "retryable": true}),
                            now,
                            None,
                        );
                        out.push(("计划休息锁定失败".into(), false));
                    }
                } else {
                    let through = s
                        .segments
                        .last()
                        .map(|seg| seg.locked_through)
                        .unwrap_or(now);
                    let _ = self.store.emit_result(
                        &s.session_id,
                        UNLOCKED,
                        json!({
                            "unlockedAt": iso(through),
                            "lockedThrough": iso(through),
                            "reason": "failure",
                        }),
                        now,
                        None,
                    );
                    out.push(("计划休息锁定意外结束".into(), false));
                }
                continue;
            }

            let last = s
                .segments
                .last()
                .map(|seg| seg.locked_through)
                .unwrap_or(s.lock_at);
            if now.signed_duration_since(last) >= Duration::seconds(30) {
                let _ = self.store.emit_result(
                    &s.session_id,
                    OBSERVED,
                    json!({"lockedThrough": iso(now)}),
                    now,
                    None,
                );
            }

            if now >= s.unlock_at {
                match self.store.emit_result(
                    &s.session_id,
                    UNLOCKED,
                    json!({
                        "unlockedAt": iso(now),
                        "lockedThrough": iso(now),
                        "reason": "scheduled",
                    }),
                    now,
                    None,
                ) {
                    Ok(_) => {
                        if locker.stop_lock().is_ok() {
                            out.push(("计划休息完成".into(), true));
                        } else {
                            let _ = self.store.emit_result(
                                &s.session_id,
                                FAILED,
                                json!({
                                    "action": "unlock",
                                    "error": "stop_lock failed",
                                    "retryable": true,
                                }),
                                now,
                                None,
                            );
                            out.push(("计划休息解锁失败，正在重试".into(), false));
                        }
                    }
                    Err(_) => {
                        let _ = locker.stop_lock();
                        out.push(("计划休息解锁失败，正在重试".into(), false));
                    }
                }
            }
        }

        // Waiting sessions that are due to start
        let mut waiting: Vec<_> = sessions
            .values()
            .filter(|s| s.phase == Phase::Waiting && now >= s.lock_at && now < s.unlock_at)
            .cloned()
            .collect();
        waiting.sort_by_key(|s| s.lock_at);
        for s in waiting {
            if locker.is_locked() {
                continue;
            }
            if locker.start_lock().is_ok() {
                match self.store.emit_result(
                    &s.session_id,
                    LOCKED,
                    json!({"lockedAt": iso(now), "lockedThrough": iso(now)}),
                    now,
                    None,
                ) {
                    Ok(_) => out.push(("计划休息已锁定".into(), true)),
                    Err(_) => {
                        let _ = locker.stop_lock();
                        let _ = self.store.emit_result(
                            &s.session_id,
                            FAILED,
                            json!({
                                "action": "lock",
                                "error": "event write failed",
                                "retryable": true,
                            }),
                            now,
                            None,
                        );
                        out.push(("计划休息锁定失败".into(), false));
                    }
                }
            } else {
                let _ = self.store.emit_result(
                    &s.session_id,
                    FAILED,
                    json!({"action": "lock", "error": "lock failed", "retryable": true}),
                    now,
                    None,
                );
                out.push(("计划休息锁定失败".into(), false));
            }
        }

        out
    }

    /// Password or admin unlock.  Returns (ok, message).
    pub fn unlock(
        &mut self,
        locker: &mut impl LockOps,
        reason: &str,
        now: DateTime<Utc>,
    ) -> (bool, String) {
        let sessions = self.sessions(now);
        let active: Vec<_> = sessions
            .values()
            .filter(|s| s.phase == Phase::Active)
            .cloned()
            .collect();
        if active.is_empty() {
            match locker.stop_lock() {
                Ok(()) => return (true, "no_rest_session".into()),
                Err(_) => return (false, "解锁失败，请重试".into()),
            }
        }
        let session = &active[0];
        // Emit event before physical unlock to prevent re-lock on restart
        match self.store.emit_result(
            &session.session_id,
            UNLOCKED,
            json!({"unlockedAt": iso(now), "lockedThrough": iso(now), "reason": reason}),
            now,
            None,
        ) {
            Ok(_) => match locker.stop_lock() {
                Ok(()) => {
                    let msg = if reason == "password" {
                        "休息提前结束"
                    } else {
                        "计划休息已解锁"
                    };
                    (true, msg.into())
                }
                Err(_) => {
                    let _ = self.store.emit_result(
                        &session.session_id,
                        FAILED,
                        json!({"action": "unlock", "error": "stop_lock failed", "retryable": true}),
                        now,
                        None,
                    );
                    (false, "解锁失败，请重试".into())
                }
            },
            Err(_) => {
                let _ = locker.stop_lock();
                (false, "解锁失败，请重试".into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    struct FakeLocker {
        locked: bool,
        fail_start: bool,
        fail_stop: bool,
    }

    impl LockOps for FakeLocker {
        fn is_locked(&self) -> bool {
            self.locked
        }
        fn start_lock(&mut self) -> anyhow::Result<()> {
            if self.fail_start {
                anyhow::bail!("fail");
            }
            self.locked = true;
            Ok(())
        }
        fn stop_lock(&mut self) -> anyhow::Result<()> {
            if self.fail_stop {
                anyhow::bail!("fail");
            }
            self.locked = false;
            Ok(())
        }
    }

    fn base() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-08T03:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn request(store: &EventStore, sid: &str, start_min: i64, dur_min: i64) {
        let b = base();
        store
            .emit(
                &store.requests_dir(),
                sid,
                REQUESTED,
                json!({
                    "lockAt": iso(b + Duration::minutes(start_min)),
                    "unlockAt": iso(b + Duration::minutes(start_min + dur_min)),
                    "minUnlockSeconds": 0,
                    "source": "rest-break",
                }),
                b,
                None,
            )
            .unwrap();
    }

    /// Force every later `emit_result` to fail on every platform: a regular
    /// file where the results directory belongs.  `events()` reads requests
    /// and results alike, so events already written move to `requests` and the
    /// session state survives.  Marking a directory read-only does not stop
    /// file creation on Windows, which made these three tests fail there.
    fn break_results(store: &EventStore) {
        let results = store.results_dir();
        fs::create_dir_all(store.requests_dir()).unwrap();
        if results.is_dir() {
            for entry in fs::read_dir(&results).unwrap() {
                let path = entry.unwrap().path();
                let name = path.file_name().unwrap().to_owned();
                fs::rename(&path, store.requests_dir().join(name)).unwrap();
            }
            fs::remove_dir(&results).unwrap();
        }
        fs::write(&results, b"").unwrap();
    }

    #[test]
    fn full_rest_locks_and_unlocks() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);

        let msgs = ctrl.tick(&mut locker, base());
        assert!(msgs.iter().any(|(m, _)| m == "计划休息已锁定"));
        assert!(locker.is_locked());

        let msgs = ctrl.tick(&mut locker, base() + Duration::minutes(10));
        assert!(msgs.iter().any(|(m, _)| m == "计划休息完成"));
        assert!(!locker.is_locked());

        let sessions = ctrl.sessions(base() + Duration::minutes(10));
        let s = &sessions["s1"];
        assert_eq!(s.phase, Phase::Ended);
        assert_eq!(s.segments[0].reason, Some("scheduled".into()));
    }

    #[test]
    fn password_unlock_allowed_immediately() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        let (ok, msg) = ctrl.unlock(&mut locker, "password", base() + Duration::seconds(1));
        assert!(ok);
        assert_eq!(msg, "休息提前结束");
        assert!(!locker.is_locked());
    }

    #[test]
    fn expired_request_skipped() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);

        let _msgs = ctrl.tick(&mut locker, base() + Duration::minutes(11));
        let sessions = ctrl.sessions(base() + Duration::minutes(11));
        assert_eq!(sessions["s1"].phase, Phase::Skipped);
    }

    #[test]
    fn restart_relocks_active() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        // Simulate restart: new controller, same store, locker starts unlocked
        let mut locker2 = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl2 = Controller::new(EventStore::new(tmp.path()));
        let msgs = ctrl2.tick(&mut locker2, base() + Duration::minutes(1));
        assert!(msgs.iter().any(|(m, _)| m == "计划休息已恢复锁定"));
        assert!(locker2.is_locked());
    }

    #[test]
    fn retryable_lock_failure_is_retried() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: true,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);

        let msgs = ctrl.tick(&mut locker, base());
        assert!(msgs.iter().any(|(m, _)| m == "计划休息锁定失败"));
        assert!(!locker.is_locked());
        // phase stays waiting because retryable
        let sessions = ctrl.sessions(base());
        assert_eq!(sessions["s1"].phase, Phase::Waiting);
    }

    #[test]
    fn overlapping_requests_are_serialized() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        request(&ctrl.store, "s2", 0, 10);

        let msgs = ctrl.tick(&mut locker, base());
        // Only one should lock because locker.is_locked() prevents re-lock
        let lock_count = msgs.iter().filter(|(m, _)| m == "计划休息已锁定").count();
        assert_eq!(lock_count, 1);
        assert!(locker.is_locked());
    }

    #[test]
    fn event_write_failure_rolls_back_lock() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        break_results(&ctrl.store);

        let msgs = ctrl.tick(&mut locker, base());

        assert!(msgs.iter().any(|(m, _)| m == "计划休息锁定失败"));
        assert!(!locker.is_locked());
    }

    #[test]
    fn unlock_emits_event_before_physical_stop() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        let (ok, msg) = ctrl.unlock(&mut locker, "password", base() + Duration::seconds(1));
        assert!(ok);
        assert_eq!(msg, "休息提前结束");
        assert!(!locker.is_locked());

        // Verify UNLOCKED event was written
        let events = ctrl.store.events();
        let unlocked_ev = events.iter().find(|e| e.kind == UNLOCKED);
        assert!(unlocked_ev.is_some());
    }

    #[test]
    fn restart_does_not_relock_after_password_unlock() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        ctrl.unlock(&mut locker, "password", base() + Duration::seconds(1));
        assert!(!locker.is_locked());

        // Simulate restart
        let mut locker2 = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl2 = Controller::new(EventStore::new(tmp.path()));
        let msgs = ctrl2.tick(&mut locker2, base() + Duration::minutes(1));
        assert!(!locker2.is_locked());
        assert!(!msgs.iter().any(|(m, _)| m == "计划休息已恢复锁定"));
    }

    #[test]
    fn unlock_event_write_failure_still_releases_lock() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        // The unlock event cannot be written: the lock must still be released.
        break_results(&ctrl.store);

        let (ok, _msg) = ctrl.unlock(&mut locker, "password", base() + Duration::seconds(1));
        assert!(!ok);
        assert!(
            !locker.is_locked(),
            "stop_lock must be called even when emit_result fails"
        );
    }

    #[test]
    fn scheduled_unlock_event_write_failure_still_releases_lock() {
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        // The scheduled unlock event cannot be written: the lock must still be
        // released.
        break_results(&ctrl.store);

        let msgs = ctrl.tick(&mut locker, base() + Duration::minutes(10));
        assert!(msgs.iter().any(|(m, _)| m == "计划休息解锁失败，正在重试"));
        assert!(
            !locker.is_locked(),
            "stop_lock must be called even when emit_result fails"
        );
    }

    #[test]
    fn late_heartbeat_does_not_revive_an_ended_session() {
        // Regression (2026-09-14): a `rest.observed` heartbeat arriving after
        // the unlock flipped the session back to Active, so the engine re-locked
        // one tick later and a manual unlock never stuck.
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        let (ok, _) = ctrl.unlock(&mut locker, "command", base() + Duration::seconds(1));
        assert!(ok);
        assert!(!locker.is_locked());

        ctrl.store
            .emit_result(
                "s1",
                OBSERVED,
                json!({"lockedThrough": iso(base() + Duration::seconds(2))}),
                base() + Duration::seconds(2),
                None,
            )
            .unwrap();

        let msgs = ctrl.tick(&mut locker, base() + Duration::seconds(3));
        assert!(!locker.is_locked(), "ended session must not be re-locked");
        assert!(!msgs.iter().any(|(m, _)| m == "计划休息已恢复锁定"));
    }

    #[test]
    fn backend_flap_resumes_without_a_duplicate_lock_event() {
        // Regression (2026-09-14): the grab backend kept dropping out of the
        // locked state and each reconciliation wrote another `rest.locked`
        // (8 events in 7 s). Resuming an open segment must not fabricate a new
        // segment: at most one `rest.locked` per rest session.
        let tmp = TempDir::new().unwrap();
        let store = EventStore::new(tmp.path());
        let mut locker = FakeLocker {
            locked: false,
            fail_start: false,
            fail_stop: false,
        };
        let mut ctrl = Controller::new(store);
        request(&ctrl.store, "s1", 0, 10);
        ctrl.tick(&mut locker, base());
        assert!(locker.is_locked());

        for secs in 1..=8 {
            locker.locked = false; // backend flap: the grab died
            let msgs = ctrl.tick(&mut locker, base() + Duration::seconds(secs));
            assert!(locker.is_locked(), "lock must be resumed");
            assert!(msgs.iter().any(|(m, _)| m == "计划休息已恢复锁定"));
        }

        let locks = ctrl
            .store
            .events()
            .iter()
            .filter(|e| e.kind == LOCKED)
            .count();
        assert_eq!(locks, 1, "resuming a lock must not emit a new rest.locked");
    }
}
