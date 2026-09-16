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
}
