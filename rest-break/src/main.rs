//! rest-break：休息提醒编排（Rust 重写，行为规格为 aide/rest-break 的 Go 实现）。
//! cron 每分钟调用：判定 due 后经 aide MCP 提交 rest.requested 事件；
//! 出错也输出不安排的判定 JSON 并以 0 退出（fail-closed）。

fn main() {
    let env = rest_break::run::Env::production();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdout = std::io::stdout().lock();
    let mut stderr = std::io::stderr().lock();
    std::process::exit(rest_break::run::run_with(
        &env,
        &args,
        &mut stdout,
        &mut stderr,
    ));
}
