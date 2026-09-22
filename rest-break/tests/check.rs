//! Assertion ports from aide/rest-break/check_test.go; names retain traceability.
use chrono::{Duration, SecondsFormat, TimeZone, Utc};
use rest_break::*;
use serde_json::{Value, json};
fn now() -> Time {
    Utc.with_ymd_and_hms(2026, 9, 6, 2, 0, 0).unwrap()
}
fn event_at(now: Time, start: f64, end: f64, status: &str) -> Event {
    Event {
        timestamp: (now + Duration::nanoseconds((start * 60e9) as i64))
            .to_rfc3339_opts(SecondsFormat::Micros, false),
        duration: (end - start) * 60.0,
        data: EventData {
            status: status.into(),
        },
    }
}
fn e(start: f64, end: f64) -> Event {
    event_at(now(), start, end, "not-afk")
}
fn afk(start: f64, end: f64) -> Event {
    event_at(now(), start, end, "afk")
}
fn planned(ago: i64, minutes: i64) -> Value {
    json!({"ts":iso(now()-Duration::minutes(ago)), "minutes":minutes})
}
fn result_at(
    now: Time,
    events: &[Event],
    reason: &str,
    work: f64,
    rest: i32,
    history: Value,
) -> Decision {
    let d = check(&serde_json::to_value(events).unwrap(), &history, now);
    assert_eq!(d.due, rest > 0, "{d:?}");
    assert_eq!(d.rest_minutes, rest, "{d:?}");
    assert_eq!(d.checked_at, iso(now));
    assert!(d.reason.contains(reason), "{d:?}; expected {reason}");
    if work >= 0.0 {
        assert!(
            (d.work_minutes - work).abs() <= 0.001,
            "{d:?}; expected {work}"
        );
    }
    d
}
fn result(events: &[Event], reason: &str, work: f64, rest: i32) -> Decision {
    result_at(now(), events, reason, work, rest, json!([]))
}
fn close(got: f64, want: f64) {
    assert!((got - want).abs() <= 0.001, "got {got}, want {want}");
}
fn aw(now: Time, work: f64) -> Vec<Event> {
    vec![
        event_at(now, -work - 10.0, -work, "afk"),
        event_at(now, -work, 0.0, "not-afk"),
    ]
}
#[test]
fn empty_and_current_afk() {
    result(&[], "no_data", -1.0, 0);
    result(
        &[e(-180.0, -2.0), afk(-2.0, 20.0)],
        "currently_afk",
        -1.0,
        0,
    );
    result(
        &[e(-180.0, 0.0), afk(0.0, 0.0)],
        "zero_duration_event",
        -1.0,
        0,
    );
}
#[test]
fn zero_duration_residue_ignored() {
    result(
        &[e(-180.0, -60.0), afk(-60.0, -60.0), e(-59.99, 0.0)],
        "",
        179.99,
        15,
    );
}
#[test]
fn zero_duration_last_with_coverage() {
    result(
        &[e(-180.0, -60.0), e(-60.0, 0.0), afk(0.0, 0.0), e(0.0, 5.0)],
        "fatigue_threshold",
        180.0,
        15,
    );
}
#[test]
fn short_afk_reduces_fatigue() {
    // 普通 AFK 恢复 0.5 倍：65 分钟工作 − 15 分钟 AFK×0.5 = 57.5 疲劳，
    // 但首个 AFK 在疲劳为 0 时恢复被 max(0) 截断，实际 62.5 疲劳 → 10 分钟休息。
    result(
        &[
            afk(-80.0, -70.0),
            e(-70.0, -35.0),
            afk(-35.0, -30.0),
            e(-30.0, 0.0),
        ],
        "",
        65.0,
        10,
    );
}
#[test]
fn afk_recovery_accumulates() {
    // e(-180,-30) 跨系数边界：150 分钟工作 × (60×2.0 + 90×1.0) = 210 疲劳。
    // 10 分钟 AFK×0.5 = 5 疲劳恢复。e(-20,0) 20 分钟 × 1.0 = 20 疲劳。
    // 总疲劳 = 210 − 5 + 20 = 225 → 15 分钟休息。
    result(
        &[
            e(-180.0, -30.0),
            afk(-30.0, -25.0),
            afk(-25.0, -20.0),
            e(-20.0, 0.0),
        ],
        "",
        170.0,
        15,
    );
    // e(-180,-60) 120 分钟 × (60×2.0 + 60×1.0) = 180 疲劳。
    // 30 分钟 AFK×0.5 = 15 恢复。e(-30,0) 30 分钟 × 1.0 = 30。
    // 总疲劳 = 180 − 15 + 30 = 195 → 15 分钟休息。
    result(
        &[e(-180.0, -60.0), afk(-60.0, -30.0), e(-30.0, 0.0)],
        "",
        150.0,
        15,
    );
    // e(-180,-120) 60 分钟 × 2.0 = 120 疲劳。90 分钟 AFK×0.5 = 45 恢复。
    // e(-30,0) 30 分钟 × 1.0 = 30。总疲劳 = 120 − 45 + 30 = 105 → 15 分钟休息。
    result(
        &[e(-180.0, -120.0), afk(-120.0, -30.0), e(-30.0, 0.0)],
        "",
        90.0,
        15,
    );
}
#[test]
fn thresholds_with_known_baseline() {
    for (work, rest) in [(59.0, 0), (60.0, 10), (89.0, 10), (90.0, 15)] {
        result(&aw(now(), work), "", work, rest);
    }
}
#[test]
fn circadian_afternoon_dip() {
    let t = now() + Duration::hours(4);
    for (work, rest, reason) in [
        (40.0, 0, "below_fatigue_threshold"),
        (43.0, 10, "fatigue_threshold"),
        (65.0, 15, "fatigue_threshold"),
    ] {
        let d = result_at(t, &aw(t, work), reason, work, rest, json!([]));
        assert!(d.reason.contains("circadian=1.4"));
    }
}
#[test]
fn circadian_evening_secondary_rise() {
    let t = now() + Duration::hours(10);
    for (work, rest) in [(34.0, 10), (50.0, 15), (33.0, 0)] {
        result_at(
            t,
            &aw(t, work),
            if rest == 0 {
                "below_fatigue_threshold"
            } else {
                "fatigue_threshold"
            },
            work,
            rest,
            json!([]),
        );
    }
}
#[test]
fn circadian_night() {
    let t = now() + Duration::hours(14);
    for (work, rest) in [(30.0, 10), (45.0, 15)] {
        result_at(t, &aw(t, work), "fatigue_threshold", work, rest, json!([]));
    }
}
#[test]
fn circadian_boundary_split() {
    let t = now() + Duration::hours(3);
    let d = result_at(
        t,
        &[event_at(t, -120.0, 0.0, "not-afk")],
        "fatigue_threshold",
        120.0,
        15,
        json!([]),
    );
    assert!(d.reason.contains("fatigue=144.0"));
}
#[test]
fn predict_next_rest_assertions() {
    for (fatigue, until, advance, rest) in [
        (30, None, 30, 10),
        (60, None, 0, 10),
        (90, None, 0, 15),
        (30, Some(20), 30, 10),
        (40, Some(50), 50, 15),
    ] {
        assert_eq!(
            predict_next_rest(
                now(),
                fatigue as f64 * 60.0,
                until.map(|m| now() + Duration::minutes(m))
            ),
            (now() + Duration::minutes(advance), rest)
        );
    }
}
#[test]
fn crossing_across_boundary() {
    let t = now() + Duration::minutes(90);
    let want = t + Duration::minutes(30) + Duration::nanoseconds((30.0 / 1.4 * 60e9) as i64);
    assert!((crossing(t, 0.0, 3600.0) - want).abs() <= Duration::seconds(1));
}
#[test]
fn unknown_baseline_and_covered_lower_bound() {
    for (work, rest, reason) in [
        (59.0, 0, "unknown_baseline"),
        (60.0, 10, "lower_bound"),
        (89.0, 10, "lower_bound"),
        (90.0, 15, "lower_bound"),
    ] {
        result(&[e(-work, 0.0)], reason, work, rest);
    }
    result(
        &[afk(-180.0, -170.0), e(-170.0, 0.0)],
        "fatigue_threshold",
        170.0,
        15,
    );
    result(
        &[e(-100.0, -3.0), afk(-3.0, 0.0), e(0.0, 1.0)],
        "currently_afk",
        -1.0,
        0,
    );
}
#[test]
fn freshness_uses_end_not_start_and_never_extrapolates() {
    result(&[e(-180.0, 0.0)], "", 180.0, 15);
    result(&[e(-180.0, -5.0)], "", 175.0, 15);
    result(&[e(-180.0, -5.01)], "stale_latest_event_end", -1.0, 0);
    let (d, _) = evaluate_with_freshness(
        &[e(-180.0, -1.0)],
        None,
        &json!([]),
        now(),
        Duration::seconds(30),
    )
    .unwrap();
    assert!(!d.due);
    assert_eq!(d.rest_minutes, 0);
    assert_eq!(d.checked_at, iso(now()));
    assert_eq!(d.reason, "stale_latest_event_end");
}
#[test]
fn gaps_and_transition_tolerance() {
    result(
        &[e(-180.0, -100.0), e(-99.0, 0.0)],
        "fatigue_threshold",
        179.0,
        15,
    );
    for gap in [5.0, 5.01] {
        result(
            &[e(-100.0, -60.0), e(-60.0 + gap / 60.0, 0.0)],
            "",
            100.0 - gap / 60.0,
            15,
        );
    }
    result(
        &[
            e(-180.0, -20.0),
            afk(-20.0, -15.0),
            afk(-15.0 + 2.0 / 60.0, -10.0),
            e(-10.0, 0.0),
        ],
        "",
        170.0,
        15,
    );
}
#[test]
fn coverage_gap_counts_as_afk() {
    // e(-180,-100) 跨北京时间 08:00 系数边界：80 分钟工作 × (60×2.0 + 20×1.0) = 140 疲劳。
    // 70 分钟缺口按 AFK×0.5 恢复 35 分钟疲劳。
    // e(-30,0) 30 分钟工作 × 1.0 = 30 疲劳。
    // 总疲劳 = 140 + 30 − 35 = 135 → 15 分钟休息。
    let (d, f) = evaluate(&[e(-180.0, -100.0), e(-30.0, 0.0)], &json!([]), now()).unwrap();
    assert!(d.due);
    close(d.work_minutes, 110.0);
    close(f / 60.0, 135.0);
    assert_eq!(d.rest_minutes, 15);
}
#[test]
fn clip_window_and_future() {
    result(&[e(-240.0, 60.0)], "", 180.0, 15);
    result(&[e(1.0, 60.0)], "no_data", -1.0, 0);
    result(&[afk(-200.0, -175.0), e(-175.0, 0.0)], "", 175.0, 15);
    result(
        &[e(-180.0, -10.0), afk(-10.0, 100.0)],
        "currently_afk",
        -1.0,
        0,
    );
}
#[test]
fn sort_and_overlaps() {
    result(
        &[e(-90.0, 0.0), e(-180.0, -60.0), e(-90.0, 0.0)],
        "",
        180.0,
        15,
    );
    result(
        &[e(-100.0, 0.0), afk(-20.0, -10.0)],
        "conflicting_overlap",
        -1.0,
        0,
    );
}
#[test]
fn subsecond_jitter_overlap_tolerated() {
    let mut work = e(-100.0, 0.0);
    work.duration = 76.0 * 60.0 + 13.0;
    let mut rest = afk(-23.8, -20.0);
    rest.duration = 228.0;
    // 96.2 分钟工作 × 1.0 − 3.8 分钟 AFK×0.5 = 94.3 疲劳 → 15 分钟休息。
    result(&[work.clone(), rest.clone(), e(-20.0, 0.0)], "", 96.2, 15);
    rest.duration = 1800.0;
    result(&[work, rest, e(-20.0, 0.0)], "conflicting_overlap", -1.0, 0);
}
#[test]
fn unknown_and_malformed_data_fail_closed() {
    result(
        &[event_at(now(), -100.0, 0.0, "unknown")],
        "unknown_status",
        -1.0,
        0,
    );
    for v in [
        Value::Null,
        json!({}),
        json!([null]),
        json!([{"timestamp":"bad"}]),
    ] {
        assert!(
            check(&v, &Value::Null, now())
                .reason
                .contains("invalid_data")
        );
        assert!(!check(&v, &json!([]), now()).due);
    }
    // Typed entry preserves Go's NaN assertion even though JSON cannot encode it.
    for duration in [f64::NAN, f64::INFINITY, -1.0] {
        let mut ev = e(-100.0, 0.0);
        ev.duration = duration;
        assert!(evaluate(&[ev], &json!([]), now()).is_err());
    }
    let mut ev = e(-100.0, 0.0);
    ev.timestamp = "2026-09-06T10:00:00".into();
    result(&[ev], "invalid_data", -1.0, 0);
}
#[test]
fn planned_unlock_cooldown_dedup_not_actual_rest() {
    let events = [e(-180.0, 0.0)];
    for (h, reason, work, rest) in [
        (json!([planned(29, 15)]), "not_actual_rest", -1.0, 0),
        (json!([planned(30, 15)]), "", 180.0, 15),
        (
            json!([planned(29, 15), planned(5, 15)]),
            "planned_unlock_cooldown",
            -1.0,
            0,
        ),
        (
            json!([planned(-10, 15)]),
            "planned_unlock_cooldown",
            -1.0,
            0,
        ),
        (json!([{"ts":"bad","minutes":15}]), "invalid_data", -1.0, 0),
        (json!({}), "invalid_data", -1.0, 0),
    ] {
        result_at(now(), &events, reason, work, rest, h);
    }
    let mut p = planned(0, 15);
    p["success"] = json!(false);
    result_at(now(), &events, "", 180.0, 15, json!([p]));
    let mut p = planned(29, 15);
    p["unlockAt"] = json!(iso(now() - Duration::minutes(14)));
    p["status"] = json!("scheduled");
    result_at(
        now(),
        &events,
        "planned_unlock_cooldown",
        -1.0,
        0,
        json!([p]),
    );
    let mut p = planned(30, 15);
    p["unlockAt"] = json!(iso(now()));
    result_at(now(), &events, "invalid_data", -1.0, 0, json!([p]));
}
#[test]
fn lock_interval_recovery_six_x() {
    // 15 分钟 AFK 恰与 15 分钟计划锁屏完全重叠：锁屏区间按 6 倍恢复，
    // 90 分钟工作 − 90×6×(15/15) = 0 疲劳 → 不安排。
    result_at(
        now(),
        &[e(-120.0, -30.0), afk(-30.0, -15.0), e(-15.0, 0.0)],
        "",
        105.0,
        0,
        json!([planned(30, 15)]),
    );
    // 15 分钟 AFK（01:00-01:15）与计划锁屏（01:30-01:45）不重叠，
    // 按普通 AFK 恢复：105 分钟工作 − 15×0.5 = 97.5 疲劳 → 15 分钟休息。
    result_at(
        now(),
        &[e(-120.0, -60.0), afk(-60.0, -45.0), e(-45.0, 0.0)],
        "",
        105.0,
        15,
        json!([planned(30, 15)]),
    );
}

fn iv(start: i64, end: i64) -> Interval {
    Interval {
        start: now() + Duration::minutes(start),
        end: now() + Duration::minutes(end),
        status: "lock",
    }
}
#[test]
fn afk_plan_overlap_matrix() {
    // 恢复 = AFK 时长 × (0.5 + 5.5×锁定覆盖率)：10 分钟区间，
    // 全锁定 60 疲劳、部分覆盖按区间加权（如 (-25,-15) 跨 [-20,-10] 一半，
    // 32.5）、不锁定 5 疲劳。
    for (ranges, want) in [
        (vec![], 5.0),
        (vec![(-30, -20)], 5.0),
        (vec![(-25, -15)], 32.5),
        (vec![(-20, -10)], 60.0),
        (vec![(-18, -13)], 32.5),
        (vec![(-25, -5)], 60.0),
        (vec![(-15, -5)], 32.5),
        (vec![(-10, 0)], 5.0),
        (vec![(-20, -10), (-20, -10)], 60.0),
        (vec![(-20, -14), (-16, -10)], 60.0),
    ] {
        close(
            recovery_fatigue(
                iv(-20, -10),
                &ranges.iter().map(|&(s, e)| iv(s, e)).collect::<Vec<_>>(),
            ) / 60.0,
            want,
        );
    }
}
#[test]
fn afk_plan_overlap_chaos() {
    // Same 1000-case union oracle and bounds; deterministic std-only PRNG.
    let mut seed = 260906_u64;
    let mut rng = |n: i64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as i64
    };
    for _ in 0..1000 {
        let (a, b) = (rng(120), rng(120));
        let (s, e) = (a.min(b), a.max(b) + 1);
        let mut covered = vec![false; (e - s) as usize];
        let mut rests = Vec::new();
        for _ in 0..rng(12) {
            let (a, b) = (rng(140) - 10, rng(140) - 10);
            let (rs, re) = (a.min(b), a.max(b) + 1);
            rests.push(iv(rs, re));
            for m in rs.max(s)..re.min(e) {
                covered[(m - s) as usize] = true;
            }
        }
        // want = 时长×0.5 + 锁定覆盖分钟×5.5（全覆盖等价 6 倍），
        // 与实现同式：total×(0.5+5.5×ratio)，ratio = 覆盖分钟/总分钟。
        let dur = (e - s) as f64;
        let cv = covered.iter().filter(|v| **v).count() as f64;
        let want = dur * (0.5 + 5.5 * cv / dur);
        close(recovery_fatigue(iv(s, e), &rests) / 60.0, want);
    }
}
#[test]
fn rest_recovery_clears_fatigue() {
    // 计划锁屏完全覆盖 AFK：恢复 = 60×(0.5+5.5) = 90 疲劳/分钟×覆盖分钟，
    // 恰好等于工作分钟数 → 疲劳清零。
    for (events, h) in [
        (
            vec![e(-70.0, -10.0), afk(-10.0, 0.0)],
            json!([planned(10, 10)]),
        ),
        (
            vec![e(-105.0, -15.0), afk(-15.0, 0.0)],
            json!([planned(15, 15)]),
        ),
    ] {
        let (d, f) = evaluate(&events, &h, now()).unwrap();
        assert_eq!(d.reason, "currently_afk");
        assert_eq!(f, 0.0);
    }
    // 无锁屏时 AFK×0.5：90 分钟工作 − 30×0.5 = 75 疲劳。
    let (d, f) = evaluate(&[e(-90.0, -30.0), afk(-30.0, 0.0)], &json!([]), now()).unwrap();
    assert_eq!(d.reason, "currently_afk");
    close(f / 60.0, 45.0);
}
#[test]
fn current_afk_shows_gradual_recovery() {
    // 70 分钟工作 − 5 分钟当前 AFK×0.5 = 67.5 疲劳。
    let (d, f) = evaluate(&[e(-75.0, -5.0), afk(-5.0, 0.0)], &json!([]), now()).unwrap();
    assert_eq!(d.reason, "currently_afk");
    close(d.work_minutes, 70.0);
    close(f / 60.0, 67.5);
}
#[test]
fn rest_recovery_clears_fatigue_fix() {
    // 90 分钟工作 − 30 分钟当前 AFK×0.5 = 75 疲劳（currently_afk 提前返回）。
    let (d, f) = evaluate(&[e(-90.0, -30.0), afk(-30.0, 0.0)], &json!([]), now()).unwrap();
    assert_eq!(d.reason, "currently_afk");
    close(f / 60.0, 45.0);
}
#[test]
fn planned_rest_accounts_for_afk_watcher_delay() {
    // AFK 判定延迟修正（watcher 晚报 AFK）：锁屏起点前 not-afk 时段回溯给锁屏。
    // (-7,0): afk 延迟 3min < 5min 容忍，回溯到锁屏起点，60 工作 − 60 恢复 = 0。
    // (-4,0): afk 延迟 6min > 5min 容忍，不修正。66 工作 × 1.0 − 4×6×1.0 = 42。
    for (start, want) in [(-7.0, 0.0), (-4.0, 42.0)] {
        let (_, f) = evaluate(
            &[e(-70.0, start), afk(start, 0.0)],
            &json!([planned(10, 10)]),
            now(),
        )
        .unwrap();
        close(f / 60.0, want);
    }
}
#[test]
fn observed_lock_session_overrides_activity_watch() {
    // 锁定区间按 6 倍恢复：3 分钟锁屏从 70 分钟工作尾部扣除，
    // 67 分钟工作 − 18 分钟恢复 = 49 疲劳。
    for (minutes, want) in [(10, 0.0), (3, 49.0)] {
        // 10 分钟锁屏：70 分钟工作全被覆盖，70 − 60 = 10... 实际 overlay 后
        // 60 分钟工作 × 1.0 = 60 疲劳，10 分钟 AFK×6 = 60 恢复 → 0。
        // 3 分钟锁屏：67 分钟工作 × 1.0 = 67，3 分钟 AFK×6 = 18 → 49。
        let sessions = json!([{"id":"s1", "phase":"ended", "segments":[{
            "lockedAt": iso(now()-Duration::minutes(minutes)),
            "lockedThrough": iso(now()),
            "unlockedAt": iso(now()),
            "endReason": "scheduled"
        }]}]);
        let (d, f) =
            evaluate_observed(&[e(-70.0, 0.0)], Some(&sessions), &Value::Null, now()).unwrap();
        assert!(!d.due);
        close(f / 60.0, want);
    }
    let sessions = json!([{"id":"s2", "phase":"ended", "segments":[{
        "lockedAt": iso(now()-Duration::minutes(3)),
        "lockedThrough": iso(now()-Duration::minutes(1)),
        "unlockedAt": iso(now()-Duration::minutes(1)),
        "endReason": "password"
    }]}]);
    let (d, _) = evaluate_observed(&[e(-70.0, 0.0)], Some(&sessions), &Value::Null, now()).unwrap();
    assert!(d.reason.contains("rest_cooldown_until:"));
}
#[test]
fn completed_rest_boundary_and_return() {
    let (_, f) = evaluate(
        &[
            e(-70.0, -10.0),
            afk(-10.0, -1.0 / 60.0),
            e(-1.0 / 60.0, 0.0),
        ],
        &json!([planned(10, 10)]),
        now(),
    )
    .unwrap();
    close(f, 7.0);
    let (_, f) = evaluate(
        &[e(-71.0, -11.0), afk(-11.0, -1.0), e(-1.0, 0.0)],
        &json!([planned(11, 10)]),
        now(),
    )
    .unwrap();
    close(f / 60.0, 1.0);
}
#[test]
fn missing_and_corrupt_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.json");
    assert_eq!(read_history(&path).unwrap(), json!([]));
    std::fs::write(&path, "not json").unwrap();
    assert!(read_history(&path).is_err());
}
#[test]
fn observed_lock_overrides_afk_watcher_conflict() {
    // 系统锁定期间 aw-watcher-afk 仍上报贯穿锁定的 not-afk 长事件，与锁定
    // 后的 afk 事件重叠数十分钟。overlay 前移后锁定区间先被覆盖成 afk，
    // 不再误报 conflicting_overlap。
    let sessions = json!([{"id":"s1", "phase":"ended", "segments":[{
        "lockedAt": iso(now()-Duration::minutes(40)),
        "lockedThrough": iso(now()-Duration::minutes(30)),
        "unlockedAt": iso(now()-Duration::minutes(30)),
        "endReason": "scheduled"
    }]}]);
    let events = [e(-70.0, 0.0), afk(-40.0, -30.0)];
    let (d, f) = evaluate_observed(&events, Some(&sessions), &Value::Null, now()).unwrap();
    assert!(!d.reason.contains("conflicting_overlap"), "{d:?}");
    close(d.work_minutes, 60.0);
    // 30 分钟工作 − 10 分钟锁定 × 6（被 max(0) 截断）+ 30 分钟工作 = 30。
    close(f / 60.0, 30.0);
}
#[test]
fn conflicting_overlap_reports_real_fatigue() {
    // skip 路径不清零疲劳：残留工作段仍是真实疲劳，UI 不能显示"疲劳 0"。
    let (d, f) = evaluate(&[e(-100.0, 0.0), afk(-20.0, -10.0)], &json!([]), now()).unwrap();
    assert_eq!(d.reason, "conflicting_overlap");
    close(d.work_minutes, 100.0);
    close(f / 60.0, 100.0);
}
#[test]
fn long_event_starting_before_window_counts() {
    result(&[e(-360.0, 0.0)], "fatigue_threshold", 180.0, 15);
}
