//! 常驻状态 UI 的纯逻辑部分：读 status.json、格式化一行摘要与详情。
//! 本模块不依赖 GUI（不引用 eframe），随 lib 默认编译，逻辑测试不引入 GUI 依赖；
//! GUI 壳在 `src/bin/rest-break-ui.rs`（`ui` feature 下的独立 bin）。

use chrono::DateTime;
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

/// ISO 时间字符串取 HH:MM（按字符串自身时区取墙钟时分，status.json 写的是北京时间）。
fn hhmm(iso: &str) -> String {
    DateTime::parse_from_rfc3339(iso)
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_else(|_| iso.to_string())
}

fn minutes(v: f64) -> String {
    if v == v.trunc() {
        format!("{}", v as i64)
    } else {
        format!("{v:.1}")
    }
}

/// 一行摘要：与现役 overlay 文本同构——疲劳分钟 · 下次休息时间 · 时长。
pub fn summary_line(s: &UiStatus) -> String {
    if s.state == "error" {
        return "休息状态数据不可用".to_string();
    }
    format!(
        "疲劳 {} 分钟 · 下次 {} 开始 · {} 分钟",
        minutes(s.fatigue_minutes),
        hhmm(&s.next_rest_at),
        s.next_rest_minutes
    )
}

/// 详情字段（label, value）：完整展示 status.json 的九个字段。
pub fn detail_lines(s: &UiStatus) -> Vec<(String, String)> {
    vec![
        ("状态".into(), s.state.clone()),
        ("疲劳（分钟）".into(), minutes(s.fatigue_minutes)),
        ("下次休息".into(), s.next_rest_at.clone()),
        ("休息时长（分钟）".into(), s.next_rest_minutes.to_string()),
        ("工作（分钟）".into(), minutes(s.work_minutes)),
        ("昼夜节律系数".into(), s.circadian.to_string()),
        ("原因".into(), s.reason.clone()),
        ("检查时间".into(), s.checked_at.clone()),
        (
            "冷却截止".into(),
            s.cooldown_until
                .as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or("-")
                .to_string(),
        ),
    ]
}

/// 指针移出后保持展开的宽限时间：既方便从摘要移到详情，也避免指针在窗口
/// 边缘抖动时反复开合。
pub const COLLAPSE_DELAY_MS: u64 = 600;

/// 展开交互状态机（纯逻辑，时间由调用方以单调毫秒传入，便于离线测试）：
///
/// - 指针进入窗口即展开；
/// - 指针移出后延迟 [`COLLAPSE_DELAY_MS`] 收起；
/// - 点击切换「钉住」，钉住期间指针移出也不收起（点击时指针必然在窗内）。
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
        if hovered || self.pinned {
            self.expanded = true;
            self.hover_out_ms = None;
        } else if self.expanded {
            match self.hover_out_ms {
                None => self.hover_out_ms = Some(now_ms),
                Some(at) if now_ms.saturating_sub(at) >= COLLAPSE_DELAY_MS => {
                    self.expanded = false;
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
        Some(COLLAPSE_DELAY_MS.saturating_sub(now_ms.saturating_sub(at)))
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
    fn summary_formats_like_existing_overlay_text() {
        let s: UiStatus = serde_json::from_str(OK).unwrap();
        assert_eq!(
            summary_line(&s),
            "疲劳 55.5 分钟 · 下次 23:42 开始 · 10 分钟"
        );
    }

    #[test]
    fn summary_uses_beijing_time_component() {
        let s: UiStatus = serde_json::from_str(OK).unwrap();
        assert!(summary_line(&s).contains("23:42"));
    }

    #[test]
    fn error_state_reports_unavailable() {
        let mut s: UiStatus = serde_json::from_str(OK).unwrap();
        s.state = "error".into();
        assert_eq!(summary_line(&s), "休息状态数据不可用");
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
        assert!(summary_line(&load(&path).unwrap()).contains("23:42"));
        let updated = OK.replace("23:42", "00:15").replace("55.5", "61.0");
        std::fs::write(&path, updated).unwrap();
        assert_eq!(
            summary_line(&load(&path).unwrap()),
            "疲劳 61 分钟 · 下次 00:15 开始 · 10 分钟"
        );
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
    fn click_pins_expanded_until_next_click() {
        let mut h = HoverState::default();
        assert!(h.update(true, true, 0));
        assert!(h.pinned());
        // 钉住期间移出再久也不收起
        assert!(h.update(false, false, 10_000));
        assert!(h.expanded());
        assert_eq!(h.collapse_in_ms(10_000), None);
        // 再点一次取消钉住，移出后仍按延迟收起
        assert!(h.update(true, true, 10_100));
        assert!(!h.pinned());
        assert!(h.update(false, false, 10_200));
        assert!(!h.update(false, false, 10_200 + COLLAPSE_DELAY_MS));
    }

    #[test]
    fn move_out_without_expand_keeps_timer_idle() {
        let mut h = HoverState::default();
        assert!(!h.update(false, false, 0));
        assert_eq!(h.collapse_in_ms(0), None);
    }
}
