//! 编排：AW 拉取 + aide MCP 直连 + pending 防重 + status.json。
//! 行为规格为 aide/rest-break/main.go 与 check.go 的 HTTP 尾部；所有副作用
//! fail-closed：判定失败/超时宁可不锁，不确定执行结果用 pending.json 抑制重试。
use crate::*;
use chrono::{Duration, SecondsFormat, Utc};
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration as StdDuration;

pub const HISTORY_FILE: &str = "history.json";
pub const PENDING_FILE: &str = "pending.json";

type HttpGet = fn(&str) -> Result<Vec<u8>, String>;
type McpUrl = fn() -> Result<(String, Vec<(String, String)>), String>;

/// 进程级环境（skill 目录、HTTP 端点）。测试经 Env::for_test 注入 mock。
pub struct Env {
    pub skill_dir: PathBuf,
    pub http_get: HttpGet,
    pub mcp_url: McpUrl,
}

impl Env {
    /// 生产环境：skill 目录 = 二进制所在目录（history/pending 与二进制同目录）。
    pub fn production() -> Self {
        let exe = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from("."));
        Env {
            skill_dir: exe,
            http_get,
            mcp_url: mcp_config,
        }
    }
    /// 测试环境：临时目录 + 可替换端点（mock HTTP 的真实 URL 或注入函数）。
    pub fn for_test(skill_dir: PathBuf, http_get: HttpGet, mcp_url: McpUrl) -> Self {
        Env {
            skill_dir,
            http_get,
            mcp_url,
        }
    }
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    ureq::get(url)
        .config()
        .timeout_global(Some(StdDuration::from_secs(5)))
        .build()
        .call()
        .and_then(|mut r| r.body_mut().read_to_vec())
        .map_err(|e| e.to_string())
}

fn aw_base() -> String {
    std::env::var("AW_BASE_URL")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.trim_end_matches('/').to_string())
        .unwrap_or_else(|| "http://localhost:5600/api/0".into())
}

/// 自动发现 aw-watcher-afk_ 前缀的 bucket；兼容真实 AW 的 map 与旧版/测试的数组。
pub fn find_afk_bucket(env: &Env) -> Result<String, String> {
    let data = (env.http_get)(&format!("{}/buckets/", aw_base()))?;
    let mut matches: Vec<String> =
        if let Ok(map) = serde_json::from_slice::<serde_json::Map<String, Value>>(&data) {
            map.keys()
                .filter(|b| b.starts_with("aw-watcher-afk_"))
                .cloned()
                .collect()
        } else {
            serde_json::from_slice::<Vec<String>>(&data)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter(|b| b.starts_with("aw-watcher-afk_"))
                .collect()
        };
    if matches.is_empty() {
        return Err("no AFK bucket found".into());
    }
    matches.sort();
    Ok(matches.remove(0))
}

/// 拉取最近 24h 的原始事件（limit=-1 不截断历史）。
pub fn fetch_events(env: &Env, now: Time) -> Result<Vec<Event>, String> {
    let bucket = find_afk_bucket(env)?;
    let start = iso(now - Duration::hours(24));
    let end = iso(now);
    let url = format!(
        "{}/buckets/{}/events?start={}&end={}&limit=-1",
        aw_base(),
        bucket,
        url_plus(&start),
        url_plus(&end)
    );
    let data = (env.http_get)(&url)?;
    serde_json::from_slice(&data).map_err(|e| e.to_string())
}

/// RFC3339 时间放进 query：`+` 必须转义（Go url.Values.Encode 同样处理）。
fn url_plus(s: &str) -> String {
    s.replace('+', "%2B")
}

// ---- aide MCP 直连 ----

/// 读 ~/.config/mcp/mcp.json 找 aide 服务器（URL + headers）。
fn mcp_config() -> Result<(String, Vec<(String, String)>), String> {
    let home = std::env::var("HOME").map_err(|e| e.to_string())?;
    let data = std::fs::read(format!("{home}/.config/mcp/mcp.json")).map_err(|e| e.to_string())?;
    let cfg: Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
    let server = cfg
        .get("mcpServers")
        .and_then(|m| m.get("aide"))
        .ok_or("mcp.json 中没有 aide 服务器")?;
    let url = server
        .get("url")
        .and_then(Value::as_str)
        .ok_or("aide 服务器缺少 url")?
        .to_string();
    let headers = server
        .get("headers")
        .and_then(Value::as_object)
        .map(|h| {
            h.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();
    Ok((url, headers))
}

/// MCP 响应：先试纯 JSON，再试 SSE（data: {...} 行）。
fn parse_mcp_result(data: &[u8]) -> Result<String, String> {
    fn text(rpc: &Value) -> Result<String, String> {
        if let Some(err) = rpc.get("error").filter(|e| !e.is_null()) {
            return Err(format!(
                "MCP error {}: {}",
                err.get("code").and_then(Value::as_i64).unwrap_or(0),
                err.get("message").and_then(Value::as_str).unwrap_or("")
            ));
        }
        let mut out = String::new();
        if let Some(content) = rpc
            .get("result")
            .and_then(|r| r.get("content"))
            .and_then(Value::as_array)
        {
            for c in content {
                if c.get("type").and_then(Value::as_str) == Some("text") {
                    out.push_str(c.get("text").and_then(Value::as_str).unwrap_or(""));
                }
            }
            return Ok(out);
        }
        Err("无法解析 MCP 响应".into())
    }
    if let Ok(rpc) = serde_json::from_slice::<Value>(data)
        && (rpc.get("result").is_some() || rpc.get("error").is_some())
    {
        return text(&rpc);
    }
    for line in String::from_utf8_lossy(data).lines() {
        if let Some(payload) = line.trim().strip_prefix("data: ")
            && let Ok(rpc) = serde_json::from_str::<Value>(payload)
        {
            return text(&rpc);
        }
    }
    Err("无法解析 MCP 响应".into())
}

pub fn mcp_call(env: &Env, tool: &str, args: Value) -> Result<String, String> {
    let (url, headers) = (env.mcp_url)()?;
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": args},
    });
    let mut req = ureq::post(&url)
        .config()
        .timeout_global(Some(StdDuration::from_secs(10)))
        .build()
        // aide MCP 严格要求 Content-Type: application/json（缺省 415）；
        // ureq send_json 只在 body 自带 content-type 时设置，这里显式钉死。
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for (k, v) in headers {
        req = req.header(k, v);
    }
    let mut resp = req.send_json(&payload).map_err(|e| e.to_string())?;
    let data = resp.body_mut().read_to_vec().map_err(|e| e.to_string())?;
    parse_mcp_result(&data)
}

pub fn mcp_get_locker_plan(env: &Env) -> Result<Vec<Value>, String> {
    let text = mcp_call(env, "get_locker_plan", json!({}))?;
    serde_json::from_str(&text).map_err(|e| format!("计划结构不明: {e}"))
}

pub fn mcp_set_locker_plan(env: &Env, plan: &[Value]) -> Result<(), String> {
    mcp_call(env, "set_locker_plan", json!({"tasks": plan})).map(|_| ())
}

pub fn fetch_rest_sessions(env: &Env) -> Result<Vec<Value>, String> {
    let text = mcp_call(env, "get_rest_sessions", json!({}))?;
    serde_json::from_str(&text).map_err(|e| format!("rest sessions structure unclear: {e}"))
}

/// 清理 aide 计划文件里 unlock 已过的 once 任务（recurring 保留）。
/// 失败只告警，不影响判定主流程。注意：事件源运行链路不调用它（Go 版 run 同样不调），
/// 仅保留给兼容调用方；写计划前的过期清理契约由本函数承担。
pub fn cleanup_expired_plans(env: &Env, stderr: &mut dyn Write) {
    let plan = match mcp_get_locker_plan(env) {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(stderr, "清理过期计划失败（读计划）: {e}");
            return;
        }
    };
    let kept = prune_expired(plan.clone(), Utc::now());
    if kept.len() == plan.len() {
        return;
    }
    if let Err(e) = mcp_set_locker_plan(env, &kept) {
        let _ = writeln!(stderr, "清理过期计划失败（写回）: {e}");
    }
}

/// 提交不可变 rest.requested 事件。请求不是 history：只有 InputLocker 的
/// 结果事件能产生恢复额度。
pub fn execute_rest_request(env: &Env, result: &Decision) -> Result<(), String> {
    let started = Utc::now();
    let session_id = new_session_id()?;
    let request_event_id = format!("{session_id}-requested");
    let pending_path = env.skill_dir.join(PENDING_FILE);
    // 1. 一个 in-flight 请求在 35 分钟内抑制重复提交。
    write_json_atomic(
        &pending_path,
        &Pending {
            session_id: session_id.clone(),
            started_at: started.to_rfc3339_opts(SecondsFormat::Secs, true),
            expires_at: (started + Duration::minutes(35))
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        },
    )?;
    let sessions = fetch_rest_sessions(env).map_err(|e| format!("读取休息会话失败: {e}"))?;
    for session in &sessions {
        let phase = session.get("phase").and_then(Value::as_str).unwrap_or("");
        if phase == "waiting" || phase == "active" {
            // 重复提交已知无副作用，不应让不确定执行保护抑制 35 分钟检查。
            let _ = std::fs::remove_file(&pending_path);
            return Err("已有未结束休息会话".into());
        }
    }
    let lock_at = Utc::now() + Duration::seconds(30);
    let unlock_at = lock_at + Duration::minutes(result.rest_minutes as i64);
    let text = mcp_call(
        env,
        "request_rest",
        json!({
            "sessionId": session_id,
            "eventId": request_event_id,
            "lockAt": lock_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
            "unlockAt": unlock_at.to_rfc3339_opts(SecondsFormat::Nanos, true),
            "source": "rest-break",
            "reason": result.reason,
        }),
    )
    .map_err(|e| format!("提交休息请求失败: {e}"))?;
    let accepted: Value = serde_json::from_str(&text).map_err(|_| "休息请求响应无 sessionId")?;
    let accepted_id = accepted
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("休息请求响应无 sessionId")?;
    write_json_atomic(
        &pending_path,
        &Pending {
            session_id: accepted_id.to_string(),
            started_at: started.to_rfc3339_opts(SecondsFormat::Secs, true),
            expires_at: (started + Duration::minutes(35))
                .to_rfc3339_opts(SecondsFormat::Secs, true),
        },
    )?;
    let _ = std::fs::remove_file(&pending_path);
    Ok(())
}

fn status_path(env: &Env) -> PathBuf {
    std::env::var("STATUS_FILE")
        .ok()
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| env.skill_dir.join("status.json"))
}

/// 原子写状态文件供 UI 轮询；失败只告警，不影响判定。
fn write_status(
    env: &Env,
    now: Time,
    result: &Decision,
    fatigue: f64,
    until: Option<Time>,
    stderr: &mut dyn Write,
) {
    let status = build_status(now, result, fatigue, until);
    if let Err(e) = write_json_atomic(&status_path(env), &status) {
        let _ = writeln!(stderr, "写状态文件失败: {e}");
    }
}

/// 主流程；返回进程退出码。`--check` 只判定不安排。
pub fn run_with(env: &Env, args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    let check_only = args.iter().any(|a| a == "--check");
    let now = Utc::now();
    // 事件源会话是休息事实的唯一活来源。--check 在 aide 端点未部署时仍可
    // 只看 AW；调度路径 fail-closed 而不退回计划 history。
    let sessions_result = fetch_rest_sessions(env);
    let (mut result, mut fatigue, mut until) = (decision(now, "", 0.0, 0), 0.0, None);
    if let Err(e) = &sessions_result
        && !check_only
    {
        result = decision(now, format!("error:{e}"), 0.0, 0);
    }
    if sessions_result.is_ok() || check_only {
        let sessions = sessions_result.ok().map(|s| json!(s));
        match fetch_events(env, now) {
            Err(e) => result = decision(now, format!("error:{e}"), 0.0, 0),
            Ok(events) => {
                match evaluate_observed(&events, sessions.as_ref(), &json!([]), now) {
                    Ok((d, f)) => {
                        result = d;
                        fatigue = f;
                    }
                    Err(e) => result = decision(now, format!("invalid_data:{e}"), 0.0, 0),
                }
                if let Some(s) = &sessions
                    && let Ok((_, u)) = observed_rest_intervals(s, now)
                {
                    until = u;
                }
            }
        }
    }

    // pending 防重：存在且未过期时不安排。
    let pending_path = env.skill_dir.join(PENDING_FILE);
    match read_pending(&pending_path) {
        PendingFile::IoError(e) => {
            let _ = writeln!(stderr, "读 pending.json 失败: {e}");
            return 1;
        }
        PendingFile::Present(p) => match parse_time(&p.expires_at) {
            Err(_) => {
                let _ = writeln!(stderr, "pending.json 无效，停止安排");
                return 1;
            }
            Ok(expires_at) if Utc::now() < expires_at => {
                result.due = false;
                result.reason = format!("上次执行结果待确认，防重至 {}", p.expires_at);
            }
            _ => {}
        },
        PendingFile::Absent => {}
    }

    // 事件日志不可变；终态会话保持可重放，没有能在锁定期间抹掉执行事实的清理。

    write_status(env, now, &result, fatigue, until, stderr);

    let _ = writeln!(
        stdout,
        "{}",
        serde_json::to_string(&result).expect("decision serializes")
    );
    if check_only || !result.due {
        return 0;
    }
    if let Err(e) = execute_rest_request(env, &result) {
        let _ = writeln!(stderr, "执行失败，保留 pending.json 防重: {e}");
        return 1;
    }
    0
}
