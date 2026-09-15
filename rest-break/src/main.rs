//! rest-break：休息提醒编排（Rust 重写，行为规格为 aide/rest-break 的 Go 实现）。
//! Pure core lives in the library; scheduling is not connected yet.

fn main() {
    // 编排尚未接入：fail-closed，任何调用都输出"不安排"的判定并以 0 退出
    // （与 Go 版"出错也输出判定、exit 0"的 stdout 契约一致）。
    let decision = rest_break::decision(
        chrono::Utc::now(),
        "error:orchestration not implemented yet",
        0.0,
        0,
    );
    println!(
        "{}",
        serde_json::to_string(&decision).expect("decision serializes")
    );
}
