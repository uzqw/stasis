//! 常驻状态 UI 的纯逻辑部分：读 status.json、格式化一行摘要与详情。
//! 本模块不依赖 GUI（不引用 eframe），随 lib 默认编译，逻辑测试不引入 GUI 依赖；
//! GUI 壳在 `src/bin/rest-break-ui.rs`（`ui` feature 下的独立 bin）。

use chrono::{DateTime, Local};
use serde::Deserialize;
use std::path::Path;

/// status.json 的只读视图（UI 消费侧字段，与 cron 写入的 Status 对应）。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiStatus {
    pub state: String,
    #[serde(default)]
    pub fatigue_minutes: f64,
    #[serde(default)]
    pub circadian: f64,
    #[serde(default)]
    pub work_minutes: f64,
    #[serde(default)]
    pub next_rest_at: String,
    #[serde(default)]
    pub next_rest_minutes: i32,
    #[serde(default)]
    pub cooldown_until: Option<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub checked_at: String,
}

/// 读取并解析 status.json；文件缺失/损坏/IO 错误都返回 Err（UI 如实显示不可用）。
pub fn load(path: &Path) -> Result<UiStatus, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("无法读取状态文件 {}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("状态文件解析失败: {e}"))
}

/// All UI timestamps use the same system-local timezone, including the summary.
fn hhmm(iso: &str) -> String {
    DateTime::parse_from_rfc3339(iso)
        .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
        .unwrap_or_else(|_| {
            if iso.is_empty() {
                "-".into()
            } else {
                iso.into()
            }
        })
}

fn minutes(v: f64) -> String {
    if v == v.trunc() {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

fn pending_info(reason: &str) -> Option<(&str, &str)> {
    let value = reason
        .strip_prefix("request_unconfirmed_until:")
        .or_else(|| reason.strip_prefix("上次执行结果待确认，防重至 "))?;
    Some(value.split_once(";last_error:").unwrap_or((value, "")))
}

/// Distinguish forecasts, accepted requests and failures; a forecast is not a schedule.
pub fn summary_line(s: &UiStatus) -> String {
    let fatigue = format!("疲劳 {} 分钟", minutes(s.fatigue_minutes));
    if let Some((until, _)) = pending_info(&s.reason) {
        return format!("{fatigue} · 请求未确认 · {} 后重试", hhmm(until));
    }
    if s.state == "error" {
        return format!("{fatigue} · 安排/检查失败（展开查看）");
    }
    if s.state == "active" {
        return format!("{fatigue} · 正在休息");
    }
    format!(
        "{fatigue} · {} {} 休息 · {} 分钟",
        if s.state == "scheduled" {
            "已安排"
        } else {
            "预计"
        },
        hhmm(&s.next_rest_at),
        s.next_rest_minutes
    )
}

fn reason_text(reason: &str) -> String {
    if let Some((until, error)) = pending_info(reason) {
        let mut text = format!(
            "休息请求尚未确认，暂不重复提交；{} 后重试",
            local_time(until)
        );
        if !error.is_empty() {
            text.push_str(&format!("；上次错误：{error}"));
        }
        return text;
    }
    if let Some(error) = reason.strip_prefix("error:") {
        return error.into();
    }
    if reason.starts_with("rest_requested:") {
        return "休息请求已接受，等待 Stasis 锁定".into();
    }
    if reason == "rest_active" {
        return "Stasis 已确认实际锁定".into();
    }
    if reason == "currently_afk" {
        return "正在离开电脑，不安排新的休息".into();
    }
    if reason.starts_with("below_fatigue_threshold:") {
        return "疲劳尚未达到 60 分钟门槛".into();
    }
    if reason.starts_with("fatigue_threshold:") {
        return "疲劳已达到休息门槛".into();
    }
    if reason.starts_with("rest_cooldown_until:") {
        return "实际休息结束后，暂缓安排下一次休息".into();
    }
    if reason == "unknown_baseline" {
        return "活动记录还不足以可靠判断疲劳".into();
    }
    reason.into()
}

/// ISO 时间转系统本地时区显示（"MM-dd HH:mm"），与状态文件写入时区无关。
fn local_time(iso_str: &str) -> String {
    DateTime::parse_from_rfc3339(iso_str)
        .map(|t| t.with_timezone(&Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| {
            if iso_str.is_empty() {
                "-".into()
            } else {
                iso_str.into()
            }
        })
}

/// 详情字段（label, value）：完整展示 status.json 的九个字段，时间列统一本地时区。
pub fn detail_lines(s: &UiStatus) -> Vec<(String, String)> {
    vec![
        (
            "状态".into(),
            match s.state.as_str() {
                "ok" => "监测中",
                "scheduled" => "已安排，等待锁定",
                "active" => "正在休息",
                "cooldown" => "休息后冷却",
                "blocked" => "请求待确认",
                "error" => "安排/检查失败",
                other => other,
            }
            .into(),
        ),
        ("疲劳（分钟）".into(), minutes(s.fatigue_minutes)),
        ("下次休息".into(), local_time(&s.next_rest_at)),
        (
            "休息时长（分钟）".into(),
            if s.next_rest_at.is_empty() {
                "-".into()
            } else {
                s.next_rest_minutes.to_string()
            },
        ),
        ("工作（分钟）".into(), minutes(s.work_minutes)),
        ("昼夜节律系数".into(), s.circadian.to_string()),
        ("原因".into(), reason_text(&s.reason)),
        ("检查时间".into(), local_time(&s.checked_at)),
        (
            "冷却截止".into(),
            s.cooldown_until
                .as_deref()
                .filter(|v| !v.is_empty())
                .map(local_time)
                .unwrap_or("-".to_string()),
        ),
    ]
}

/// 指针移出后保持展开的宽限时间：既方便从摘要移到详情，也避免指针在窗口
/// 边缘抖动时反复开合。
pub const COLLAPSE_DELAY_MS: u64 = 600;
/// 钉住后的收起宽限：足够长让「短暂划出又回来」不打断阅读，
/// 又足够短不会让用户觉得窗口卡死。
pub const PINNED_COLLAPSE_DELAY_MS: u64 = 4_000;

/// 展开交互状态机（纯逻辑，时间由调用方以单调毫秒传入，便于离线测试）：
///
/// - 指针进入窗口即展开；
/// - 指针移出后延迟 [`COLLAPSE_DELAY_MS`] 收起；
/// - 点击切换「钉住」，钉住只把收起宽限放宽到 [`PINNED_COLLAPSE_DELAY_MS`]
///   ——鼠标走了最终还是会收，钉住只防「短暂划出又回来」的误收。
///   永久钉住会让用户以为窗口卡死（没有视觉反馈提示要再点一下）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HoverState {
    expanded: bool,
    pinned: bool,
    hover_out_ms: Option<u64>,
}

impl HoverState {
    pub fn expanded(self) -> bool {
        self.expanded
    }

    pub fn pinned(self) -> bool {
        self.pinned
    }

    /// 每帧调用一次：`hovered` = 指针在窗口内，`clicked` = 本帧窗内按下并松开，
    /// `now_ms` = 单调毫秒。返回更新后的展开态。
    pub fn update(&mut self, hovered: bool, clicked: bool, now_ms: u64) -> bool {
        if clicked {
            self.pinned = !self.pinned;
        }
        if hovered {
            self.expanded = true;
            self.hover_out_ms = None;
        } else if self.expanded {
            let delay = if self.pinned {
                PINNED_COLLAPSE_DELAY_MS
            } else {
                COLLAPSE_DELAY_MS
            };
            match self.hover_out_ms {
                None => self.hover_out_ms = Some(now_ms),
                Some(at) if now_ms.saturating_sub(at) >= delay => {
                    self.expanded = false;
                    self.pinned = false;
                    self.hover_out_ms = None;
                }
                Some(_) => {}
            }
        }
        self.expanded
    }

    /// 收起倒计时剩余毫秒；`Some` 表示需要按该延迟安排一次重绘。
    pub fn collapse_in_ms(self, now_ms: u64) -> Option<u64> {
        let at = self.hover_out_ms?;
        let delay = if self.pinned {
            PINNED_COLLAPSE_DELAY_MS
        } else {
            COLLAPSE_DELAY_MS
        };
        Some(delay.saturating_sub(now_ms.saturating_sub(at)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{
        "state": "ok",
        "fatigueMinutes": 55.5,
        "circadian": 1.4,
        "workMinutes": 39.8,
        "due": false,
        "restMinutes": 0,
        "nextRestAt": "2026-09-17T23:42:00+08:00",
        "nextRestMinutes": 10,
        "cooldownUntil": "",
        "reason": "below_fatigue_threshold:fatigue=55.5,circadian=1.4",
        "checkedAt": "2026-09-17T23:27:00+00:00"
    }"#;

    fn tempdir() -> PathBuf {
        tempfile::tempdir().unwrap().keep()
    }

    use std::path::PathBuf;

    #[test]
    fn summary_labels_forecasts_in_local_time() {
        let s: UiStatus = serde_json::from_str(OK).unwrap();
        assert_eq!(
            summary_line(&s),
            format!(
                "疲劳 55.5 分钟 · 预计 {} 休息 · 10 分钟",
                hhmm(&s.next_rest_at)
            )
        );
    }

    #[test]
    fn summary_and_details_use_local_time() {
        let s: UiStatus = serde_json::from_str(OK).unwrap();
        let expected = DateTime::parse_from_rfc3339(&s.next_rest_at)
            .unwrap()
            .with_timezone(&Local)
            .format("%H:%M")
            .to_string();
        assert!(summary_line(&s).contains(&expected));
        assert!(detail_lines(&s)[2].1.ends_with(&expected));
    }

    #[test]
    fn error_state_reports_unavailable() {
        let mut s: UiStatus = serde_json::from_str(OK).unwrap();
        s.state = "error".into();
        assert_eq!(
            summary_line(&s),
            "疲劳 55.5 分钟 · 安排/检查失败（展开查看）"
        );
    }

    #[test]
    fn detail_covers_all_nine_fields() {
        let s: UiStatus = serde_json::from_str(OK).unwrap();
        let lines = detail_lines(&s);
        let labels: Vec<&str> = lines.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(
            labels,
            [
                "状态",
                "疲劳（分钟）",
                "下次休息",
                "休息时长（分钟）",
                "工作（分钟）",
                "昼夜节律系数",
                "原因",
                "检查时间",
                "冷却截止"
            ]
        );
        assert_eq!(lines[1].1, "55.5");
        assert_eq!(lines[8].1, "-"); // 空 cooldownUntil 如实显示 -
    }

    #[test]
    fn unconfirmed_request_is_not_presented_as_a_schedule() {
        let mut s: UiStatus = serde_json::from_str(OK).unwrap();
        s.state = "blocked".into();
        s.next_rest_at.clear();
        s.reason = "request_unconfirmed_until:2026-09-30T16:13:04Z;last_error:timeout".into();
        assert!(summary_line(&s).contains("请求未确认"));
        assert!(summary_line(&s).contains(&hhmm("2026-09-30T16:13:04Z")));
        let lines = detail_lines(&s);
        assert_eq!(lines[2].1, "-");
        assert_eq!(lines[3].1, "-");
        assert!(lines[6].1.contains("暂不重复提交"));
        assert!(lines[6].1.contains("timeout"));
        assert!(!lines[6].1.contains("防重"));
        s.reason = "上次执行结果待确认，防重至 2026-09-30T16:13:04Z".into();
        assert!(summary_line(&s).contains("请求未确认"));
    }

    #[test]
    fn accepted_request_and_active_lock_have_distinct_summaries() {
        let mut s: UiStatus = serde_json::from_str(OK).unwrap();
        s.state = "scheduled".into();
        assert!(summary_line(&s).contains("已安排"));
        assert!(!summary_line(&s).contains("预计"));
        s.state = "active".into();
        assert!(summary_line(&s).contains("正在休息"));
    }

    #[test]
    fn load_missing_file_is_error_not_panic() {
        let dir = tempdir();
        let err = load(&dir.join("status.json")).unwrap_err();
        assert!(err.contains("无法读取状态文件"), "{err}");
    }

    #[test]
    fn load_corrupt_json_is_error() {
        let dir = tempdir();
        let path = dir.join("status.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = load(&path).unwrap_err();
        assert!(err.contains("解析失败"), "{err}");
    }

    #[test]
    fn load_refreshes_when_file_rewritten() {
        let dir = tempdir();
        let path = dir.join("status.json");
        std::fs::write(&path, OK).unwrap();
        assert_eq!(load(&path).unwrap().fatigue_minutes, 55.5);
        let updated = OK.replace("23:42", "00:15").replace("55.5", "61.0");
        std::fs::write(&path, updated).unwrap();
        let s = load(&path).unwrap();
        assert_eq!(
            summary_line(&s),
            format!(
                "疲劳 61 分钟 · 预计 {} 休息 · 10 分钟",
                hhmm(&s.next_rest_at)
            )
        );
        assert!(s.next_rest_at.contains("00:15"));
    }

    #[test]
    fn hover_expands_and_collapses_after_delay() {
        let mut h = HoverState::default();
        assert!(!h.expanded());
        assert!(h.update(true, false, 0));
        // 移出后延迟内保持展开
        assert!(h.update(false, false, 100));
        assert_eq!(h.collapse_in_ms(100), Some(COLLAPSE_DELAY_MS));
        assert!(h.update(false, false, 100 + COLLAPSE_DELAY_MS - 1));
        assert!(h.expanded());
        // 超过延迟收起，倒计时结束
        assert!(!h.update(false, false, 100 + COLLAPSE_DELAY_MS));
        assert_eq!(h.collapse_in_ms(100 + COLLAPSE_DELAY_MS), None);
        // 重新进入立即展开
        assert!(h.update(true, false, 5000));
        assert_eq!(h.collapse_in_ms(5000), None);
    }

    #[test]
    fn click_pins_expanded_with_longer_grace() {
        let mut h = HoverState::default();
        assert!(h.update(true, true, 0));
        assert!(h.pinned());
        // 钉住只延长宽限，不永久展开：移出后 PINNED 延迟内保持
        assert!(h.update(false, false, 10_000));
        assert!(h.expanded());
        assert_eq!(h.collapse_in_ms(10_000), Some(PINNED_COLLAPSE_DELAY_MS));
        assert!(h.update(false, false, 10_000 + PINNED_COLLAPSE_DELAY_MS - 1));
        assert!(h.expanded());
        // 超过 PINNED 延迟仍收起，钉住一并解除
        assert!(!h.update(false, false, 10_000 + PINNED_COLLAPSE_DELAY_MS));
        assert!(!h.pinned());
        // 再点一次取消钉住，移出后按普通延迟收起
        assert!(h.update(true, true, 20_000));
        assert!(h.update(true, true, 20_100));
        assert!(!h.pinned());
        assert!(h.update(false, false, 20_200));
        assert!(!h.update(false, false, 20_200 + COLLAPSE_DELAY_MS));
    }

    #[test]
    fn move_out_without_expand_keeps_timer_idle() {
        let mut h = HoverState::default();
        assert!(!h.update(false, false, 0));
        assert_eq!(h.collapse_in_ms(0), None);
    }
}
