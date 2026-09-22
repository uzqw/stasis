//! Pure rest-break decision core. No network or locking side effects.
//! `run` 模块承载编排副作用（HTTP/MCP/文件），经 Env 注入端点便于测试。
pub mod run;
pub mod ui;

use chrono::{DateTime, Duration, SecondsFormat, Timelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Time = DateTime<Utc>;
pub fn parse_time(s: &str) -> Result<Time, String> {
    DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|_| "timestamp must include a timezone".into())
}
pub fn iso(t: Time) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}
fn seconds(d: Duration) -> f64 {
    d.as_seconds_f64()
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    pub due: bool,
    pub work_minutes: f64,
    pub rest_minutes: i32,
    pub reason: String,
    pub checked_at: String,
}
pub fn decision(now: Time, reason: impl Into<String>, work: f64, rest: i32) -> Decision {
    Decision {
        due: rest > 0,
        work_minutes: (work / 60.0 * 1000.0).round() / 1000.0,
        rest_minutes: rest,
        reason: reason.into(),
        checked_at: iso(now),
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub timestamp: String,
    pub duration: f64,
    pub data: EventData,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventData {
    pub status: String,
}
#[derive(Debug, Clone, Copy)]
pub struct Interval {
    pub start: Time,
    pub end: Time,
    pub status: &'static str,
}
pub fn beijing_hour(t: Time) -> u32 {
    (t.hour() + 8) % 24
}
pub fn circadian_factor(hour: u32) -> f64 {
    match hour {
        8..=11 => 1.0,
        12..=17 => 1.4,
        18..=21 => 1.8,
        _ => 2.0,
    }
}
fn next_boundary(t: Time) -> Time {
    let mut next = t
        .with_minute(0)
        .unwrap()
        .with_second(0)
        .unwrap()
        .with_nanosecond(0)
        .unwrap()
        + Duration::hours(1);
    while !matches!(next.hour(), 0 | 4 | 10 | 14) {
        next += Duration::hours(1);
    }
    next
}
pub fn fatigue_seconds(start: Time, end: Time) -> f64 {
    let (mut cur, mut total) = (start, 0.0);
    while cur < end {
        let next = next_boundary(cur).min(end);
        total += seconds(next - cur) * circadian_factor(beijing_hour(cur));
        cur = next;
    }
    total
}
pub fn planned_rests(history: &Value) -> Result<Vec<Interval>, String> {
    let list = history.as_array().ok_or("history must be an array")?;
    let mut rests = Vec::new();
    for entry in list {
        let entry = entry.as_object().ok_or("invalid history entry")?;
        if let Some(flag) = entry.get("success")
            && !flag.as_bool().ok_or("invalid history success flag")?
        {
            continue;
        }
        let minutes = entry
            .get("minutes")
            .and_then(Value::as_f64)
            .filter(|m| *m == 10.0 || *m == 15.0)
            .ok_or("invalid planned rest duration")?;
        let start = parse_time(
            entry
                .get("ts")
                .and_then(Value::as_str)
                .ok_or("invalid history ts")?,
        )?;
        let end = match entry.get("unlockAt").and_then(Value::as_str) {
            Some(s) => parse_time(s)?,
            None => start + Duration::minutes(minutes as i64),
        };
        if (seconds(end - start) - minutes * 60.0).abs() >= 1.0 {
            return Err("planned unlock does not match rest duration".into());
        }
        rests.push(Interval {
            start,
            end,
            status: "lock",
        });
    }
    Ok(rests)
}
pub fn cooldown_until(history: &Value) -> Result<Option<Time>, String> {
    Ok(planned_rests(history)?
        .iter()
        .map(|r| r.end + Duration::minutes(15))
        .max())
}
pub fn observed_rest_intervals(
    sessions: &Value,
    now: Time,
) -> Result<(Vec<Interval>, Option<Time>), String> {
    let mut rests = Vec::new();
    let mut until = None;
    for session in sessions
        .as_array()
        .ok_or("rest sessions must be an array")?
    {
        let session = session.as_object().ok_or("invalid rest session")?;
        let phase = session.get("phase").and_then(Value::as_str).unwrap_or("");
        let segments = match session.get("segments") {
            None | Some(Value::Null) => &[][..],
            Some(Value::Array(a)) => a.as_slice(),
            _ => return Err("invalid rest session segments".into()),
        };
        for segment in segments {
            let segment = segment.as_object().ok_or("invalid rest segment")?;
            let start = parse_time(
                segment
                    .get("lockedAt")
                    .and_then(Value::as_str)
                    .ok_or("invalid rest lockedAt")?,
            )?;
            let unlocked = segment
                .get("unlockedAt")
                .and_then(Value::as_str)
                .map(parse_time)
                .transpose()?;
            let end = if let Some(end) = unlocked {
                end
            } else if let Some(s) = segment.get("lockedThrough").and_then(Value::as_str) {
                let end = parse_time(s)?;
                if phase == "active" { end.max(now) } else { end }
            } else {
                now
            };
            if end > start {
                rests.push(Interval {
                    start,
                    end,
                    status: "lock",
                });
            }
            if let Some(end) = unlocked {
                until = until.max(Some(end + Duration::minutes(15)));
            }
        }
    }
    Ok((rests, until))
}
pub fn recovery_fatigue(iv: Interval, rests: &[Interval]) -> f64 {
    let mut overlaps: Vec<_> = rests
        .iter()
        .filter_map(|r| {
            let (start, end) = (iv.start.max(r.start), iv.end.min(r.end));
            (start < end).then_some((start, end))
        })
        .collect();
    overlaps.sort();
    let (mut i, mut locked) = (0, 0.0);
    while i < overlaps.len() {
        let (start, mut end) = overlaps[i];
        i += 1;
        while i < overlaps.len() && overlaps[i].0 <= end {
            end = end.max(overlaps[i].1);
            i += 1;
        }
        locked += fatigue_seconds(start, end);
    }
    let locked_ratio = locked / fatigue_seconds(iv.start, iv.end);
    // 普通 AFK 恢复 0.5 倍（2026-09-21 用户判定 2 倍与事实不符：AFK 不能证明
    // 在休息）；锁定区间 6 倍 → 系数 = 0.5 + 5.5×锁定覆盖率。
    fatigue_seconds(iv.start, iv.end) * (0.5 + 5.5 * locked_ratio)
}
fn overlay(intervals: &[Interval], rests: &[Interval]) -> Vec<Interval> {
    if rests.is_empty() {
        return intervals.to_vec();
    }
    let mut boundaries: Vec<_> = intervals
        .iter()
        .chain(rests)
        .flat_map(|i| [i.start, i.end])
        .collect();
    boundaries.sort();
    boundaries.dedup();
    let mut out: Vec<Interval> = Vec::new();
    for w in boundaries.windows(2) {
        let (start, end) = (w[0], w[1]);
        let mid = start + (end - start) / 2;
        let status = if rests.iter().any(|r| mid >= r.start && mid < r.end) {
            Some("afk")
        } else {
            intervals
                .iter()
                .find(|i| mid >= i.start && mid < i.end)
                .map(|i| i.status)
        };
        if let Some(status) = status {
            if let Some(last) = out.last_mut()
                && last.status == status
                && last.end >= start
            {
                last.end = end;
                continue;
            }
            out.push(Interval { start, end, status });
        }
    }
    out
}
fn account_delay(intervals: &mut [Interval], rests: &[Interval]) {
    for i in 1..intervals.len() {
        if intervals[i - 1].status != "not-afk" || intervals[i].status != "afk" {
            continue;
        }
        let mut boundary = intervals[i].start;
        for r in rests {
            let delay = intervals[i].start - r.start;
            if delay < Duration::zero()
                || delay > Duration::minutes(5)
                || intervals[i].start >= r.end
            {
                continue;
            }
            boundary = boundary.min(r.start.max(intervals[i - 1].start));
        }
        intervals[i - 1].end = boundary;
        intervals[i].start = boundary;
    }
}
pub fn evaluate(events: &[Event], history: &Value, now: Time) -> Result<(Decision, f64), String> {
    evaluate_observed(events, None, history, now)
}
pub fn evaluate_observed(
    events: &[Event],
    sessions: Option<&Value>,
    history: &Value,
    now: Time,
) -> Result<(Decision, f64), String> {
    evaluate_with_freshness(events, sessions, history, now, Duration::minutes(5))
}

pub fn evaluate_with_freshness(
    events: &[Event],
    sessions: Option<&Value>,
    history: &Value,
    now: Time,
    freshness: Duration,
) -> Result<(Decision, f64), String> {
    let (rests, until) = match sessions {
        Some(s) => observed_rest_intervals(s, now)?,
        None => {
            let r = planned_rests(history)?;
            let u = r.iter().map(|r| r.end + Duration::minutes(15)).max();
            (r, u)
        }
    };
    // 跳过路径也如实计算疲劳：锁定期间 aw-watcher-afk 残留的 not-afk 工作段
    // 仍是真实疲劳，清零会让 UI 显示"疲劳 0"误导用户。
    let skip = |intervals: &[Interval], rests: &[Interval], reason: &str| {
        let (mut work, mut fatigue) = (0.0, 0.0);
        for iv in intervals {
            if iv.status == "afk" {
                fatigue = (fatigue - recovery_fatigue(*iv, rests)).max(0.0);
            } else {
                work += seconds(iv.end - iv.start);
                fatigue += fatigue_seconds(iv.start, iv.end);
            }
        }
        Ok((decision(now, reason, work, 0), fatigue))
    };
    let window_start = now - Duration::hours(3);
    let mut parsed = Vec::new();
    let (mut last_start, mut last_zero) = (Time::MIN_UTC, false);
    for event in events {
        let start = parse_time(&event.timestamp)?;
        if !event.duration.is_finite() || event.duration < 0.0 {
            return Err("invalid event duration".into());
        }
        let nanos = event.duration * 1e9;
        if nanos > i64::MAX as f64 {
            return Err("invalid event duration".into());
        }
        let end = start
            .checked_add_signed(Duration::nanoseconds(nanos as i64))
            .ok_or("invalid event duration")?;
        let zero = event.duration == 0.0;
        if start > last_start {
            last_start = start;
            last_zero = zero;
        }
        parsed.push((start, end, event.data.status.as_str(), zero));
    }
    if last_zero
        && last_start >= window_start
        && last_start <= now
        && !parsed
            .iter()
            .any(|p| !p.3 && seconds(p.0 - last_start).abs() < 1.0)
    {
        return skip(&[], &rests, "zero_duration_event_no_coverage");
    }
    let mut intervals = Vec::new();
    for (start, end, status, zero) in parsed {
        if zero {
            continue;
        }
        let (start, end) = (start.max(window_start), end.min(now));
        if start >= end {
            continue;
        }
        let status = match status {
            "afk" => "afk",
            "not-afk" => "not-afk",
            _ => return skip(&intervals, &rests, "unknown_status"),
        };
        intervals.push(Interval { start, end, status });
    }
    if intervals.is_empty() {
        return skip(&intervals, &rests, "no_data");
    }
    intervals.sort_by_key(|i| (i.start, i.end, i.status));
    if now - intervals.iter().map(|i| i.end).max().unwrap() > freshness {
        return skip(&intervals, &rests, "stale_latest_event_end");
    }
    // 观测到的锁定会话优先于 AW 状态：aw-watcher-afk 在系统锁定期间仍会
    // 上报贯穿锁定的 not-afk 长事件，与锁定后的 afk 事件重叠数十分钟。
    // 先把锁定区间覆盖成 afk 再判冲突，否则恒定误报 conflicting_overlap。
    if sessions.is_some() {
        intervals = overlay(&intervals, &rests);
    }
    let mut merged: Vec<Interval> = Vec::new();
    for iv in intervals {
        if let Some(prev) = merged.last_mut()
            && iv.start < prev.end
        {
            if iv.status != prev.status {
                if prev.end - iv.start > Duration::seconds(5) {
                    return skip(&merged, &rests, "conflicting_overlap");
                }
                prev.end = iv.start;
            } else {
                prev.end = prev.end.max(iv.end);
                continue;
            }
        }
        merged.push(iv);
    }
    let mut filled = Vec::new();
    for (i, iv) in merged.iter().enumerate() {
        if i > 0 && iv.start - merged[i - 1].end > Duration::seconds(5) {
            filled.push(Interval {
                start: merged[i - 1].end,
                end: iv.start,
                status: "afk",
            });
        }
        filled.push(*iv);
    }
    if sessions.is_none() {
        account_delay(&mut filled, &rests);
    }
    let (mut work, mut fatigue, mut covered, mut baseline) = (0.0, 0.0, 0.0, false);
    for iv in &filled {
        covered += seconds(iv.end - iv.start);
        if iv.status == "afk" {
            baseline = true;
            fatigue = (fatigue - recovery_fatigue(*iv, &rests)).max(0.0);
        } else {
            work += seconds(iv.end - iv.start);
            fatigue += fatigue_seconds(iv.start, iv.end);
        }
    }
    if filled.last().is_some_and(|i| i.status == "afk") {
        return Ok((decision(now, "currently_afk", work, 0), fatigue));
    }
    if let Some(until) = until.filter(|u| now < *u) {
        let reason = if sessions.is_some() {
            format!("rest_cooldown_until:{}", iso(until))
        } else {
            format!(
                "planned_unlock_cooldown_until:{};not_actual_rest",
                iso(until)
            )
        };
        return Ok((decision(now, reason, work, 0), fatigue));
    }
    if !baseline && covered < 3600.0 {
        return Ok((decision(now, "unknown_baseline", work, 0), fatigue));
    }
    let rest = if fatigue >= 5400.0 {
        15
    } else if fatigue >= 3600.0 {
        10
    } else {
        0
    };
    let reason = format!(
        "{}fatigue_threshold:fatigue={:.1},circadian={:.1}{}",
        if rest == 0 { "below_" } else { "" },
        fatigue / 60.0,
        circadian_factor(beijing_hour(now)),
        if baseline { "" } else { "_lower_bound" }
    );
    Ok((decision(now, reason, work, rest), fatigue))
}
pub fn check(events: &Value, history: &Value, now: Time) -> Decision {
    let result = serde_json::from_value::<Vec<Event>>(events.clone())
        .map_err(|_| "events must be an array".into())
        .and_then(|events| evaluate(&events, history, now));
    match result {
        Ok((d, _)) => d,
        Err(e) => decision(now, format!("invalid_data:{e}"), 0.0, 0),
    }
}
pub fn rest_for(fatigue: f64) -> i32 {
    if fatigue >= 5400.0 { 15 } else { 10 }
}
pub fn crossing(now: Time, fatigue: f64, target: f64) -> Time {
    let (mut t, mut remaining) = (now, target - fatigue);
    while remaining > 0.0 {
        let next = next_boundary(t);
        let rate = circadian_factor(beijing_hour(t));
        let available = seconds(next - t) * rate;
        if remaining <= available {
            return t + Duration::nanoseconds((remaining / rate * 1e9) as i64);
        }
        remaining -= available;
        t = next;
    }
    t
}
pub fn predict_next_rest(now: Time, fatigue: f64, until: Option<Time>) -> (Time, i32) {
    if let Some(until) = until.filter(|u| now < *u) {
        let f = fatigue + fatigue_seconds(now, until);
        return if f >= 3600.0 {
            (until, rest_for(f))
        } else {
            (crossing(until, f, 3600.0), 10)
        };
    }
    if fatigue >= 3600.0 {
        (now, rest_for(fatigue))
    } else {
        (crossing(now, fatigue, 3600.0), 10)
    }
}
pub fn read_history(path: &std::path::Path) -> Result<Value, String> {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!([])),
        Err(e) => Err(e.to_string()),
        Ok(bytes) => {
            let entries: Vec<serde_json::Map<String, Value>> =
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            Ok(serde_json::to_value(entries).unwrap())
        }
    }
}

// ---- 编排支撑：pending 防重、原子写、状态文件、计划清理 ----

/// pending.json：一次已提交但结果未确认的休息请求（不确定执行保护）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session_id: String,
    pub started_at: String,
    pub expires_at: String,
}

/// 读取 pending.json 的三种状态：不存在、存在（可能损坏）、读文件 IO 错。
pub enum PendingFile {
    Absent,
    Present(Pending),
    IoError(std::io::Error),
}

pub fn read_pending(path: &std::path::Path) -> PendingFile {
    match std::fs::read(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => PendingFile::Absent,
        Err(e) => PendingFile::IoError(e),
        Ok(bytes) => PendingFile::Present(serde_json::from_slice(&bytes).unwrap_or(Pending {
            session_id: String::new(),
            started_at: String::new(),
            expires_at: "invalid".into(),
        })),
    }
}

/// 原子写 JSON：tmp + rename，崩溃前 rename 则目标文件保持旧内容。
pub fn write_json_atomic(path: &std::path::Path, v: &impl Serialize) -> Result<(), String> {
    let mut data = serde_json::to_vec_pretty(v).map_err(|e| e.to_string())?;
    data.push(b'\n');
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::Path::new(&tmp);
    std::fs::write(tmp, data).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, path).map_err(|e| e.to_string())
}

/// 新会话 ID：16 字节随机 hex，前缀 rb-（与 Go 版格式一致）。
pub fn new_session_id() -> Result<String, String> {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b))
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "rb-{}",
        b.iter().map(|x| format!("{x:02x}")).collect::<String>()
    ))
}

/// 状态 JSON（UI 轮询，cron 每分钟原子写一次）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String, // ok | cooldown | error
    pub fatigue_minutes: f64,
    pub circadian: f64,
    pub work_minutes: f64,
    pub due: bool,
    pub rest_minutes: i32,
    pub next_rest_at: String,
    pub next_rest_minutes: i32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cooldown_until: String,
    pub reason: String,
    pub checked_at: String,
}

fn beijing(t: Time) -> String {
    iso(t + Duration::hours(8)).replace("+00:00", "+08:00")
}

/// 组装状态对象；写文件失败只告警由调用方处理。
pub fn build_status(now: Time, result: &Decision, fatigue: f64, until: Option<Time>) -> Status {
    let state = if result.reason.starts_with("error:") || result.reason.starts_with("invalid_data:")
    {
        "error"
    } else if until.is_some_and(|u| now < u) {
        "cooldown"
    } else {
        "ok"
    };
    let (next_at, next_min) = predict_next_rest(now, fatigue, until);
    Status {
        state: state.into(),
        fatigue_minutes: (fatigue / 60.0 * 10.0).round() / 10.0,
        circadian: circadian_factor(beijing_hour(now)),
        work_minutes: result.work_minutes,
        due: result.due,
        rest_minutes: result.rest_minutes,
        next_rest_at: beijing(next_at),
        next_rest_minutes: next_min,
        cooldown_until: until.map(beijing).unwrap_or_default(),
        reason: result.reason.clone(),
        checked_at: iso(now),
    }
}

/// 计划里存在未结束的 once 锁定或任何 recurring → 冲突。
pub fn find_conflict(plan: &[Value], now: Time) -> bool {
    plan.iter().any(|task| {
        let mode = task.get("mode").and_then(Value::as_str).unwrap_or("");
        if mode == "recurring" {
            return true;
        }
        if mode != "once" {
            return false;
        }
        ["when", "unlock"].iter().any(|key| {
            task.get(*key)
                .and_then(|w| w.get("at"))
                .and_then(Value::as_str)
                .and_then(|s| parse_time(s).ok())
                .is_some_and(|t| t > now)
        })
    })
}

/// 丢弃 unlock 已过的 once 任务；recurring 保留。
pub fn prune_expired(plan: Vec<Value>, now: Time) -> Vec<Value> {
    plan.into_iter()
        .filter(|task| {
            if task.get("mode").and_then(Value::as_str) != Some("once") {
                return true;
            }
            !task
                .get("unlock")
                .and_then(|u| u.get("at"))
                .and_then(Value::as_str)
                .and_then(|s| parse_time(s).ok())
                .is_some_and(|t| t <= now)
        })
        .collect()
}
