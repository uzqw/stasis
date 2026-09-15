//! Assertion ports from aide/rest-break/main_test.go plus the three deferred
//! check_test.go HTTP tests. Mock AW/MCP are real loopback TcpListener servers;
//! env vars are process-global, so this file runs single-threaded.
use chrono::{Duration, SecondsFormat, TimeZone, Utc};
use rest_break::run::*;
use rest_break::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn now_fixed() -> Time {
    // 与 check_test.go 的 NOW 一致（2026-09-06T10:20:00+08:00 = UTC 02:20）。
    Utc.with_ymd_and_hms(2026, 9, 6, 2, 20, 0).unwrap()
}

// ---- 最小 mock HTTP 服务器 ----

struct Server {
    url: String,
    _join: std::thread::JoinHandle<()>,
}

/// handler: (path, query, body) -> (status, body)
fn serve(handler: impl Fn(&str, &str, &[u8]) -> (u16, String) + Send + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let join = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.is_empty() {
                return;
            }
            let target = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let (path, query) = match target.split_once('?') {
                Some((p, q)) => (p.to_string(), q.to_string()),
                None => (target, String::new()),
            };
            let mut content_len = 0usize;
            loop {
                line.clear();
                if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                    break;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_len = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; content_len];
            reader.read_exact(&mut body).unwrap_or_default();
            let (status, resp) = handler(&path, &query, &body);
            let reason = if status == 200 { "OK" } else { "Error" };
            let out = format!(
                "HTTP/1.1 {status} {reason}\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{resp}",
                resp.len()
            );
            let _ = stream.write_all(out.as_bytes());
        }
    });
    Server { url, _join: join }
}

/// mock AW：/buckets/ 返回 afk bucket，/events 返回给定事件。
fn aw_server(events: Value) -> Server {
    serve(move |path, _query, _body| {
        if path.ends_with("/buckets/") {
            (200, r#"["aw-watcher-afk_TEST"]"#.into())
        } else if path.contains("/events") {
            (200, events.to_string())
        } else {
            (404, String::new())
        }
    })
}

/// 180 分钟连续 not-afk → due:true, rest=15。
fn due_events() -> Value {
    let now = Utc::now();
    json!([{
        "timestamp": (now - Duration::minutes(180)).to_rfc3339_opts(SecondsFormat::Secs, true),
        "duration": 180 * 60,
        "data": {"status": "not-afk"},
    }])
}

/// mock aide MCP：get_locker_plan 返回给定计划，set_locker_plan 记入 written。
fn mock_mcp(initial_plan: Value, written: &'static AtomicUsize) -> Server {
    serve(move |path, _query, body| {
        let _ = path;
        let req: Value = serde_json::from_slice(body).unwrap_or(json!({}));
        let name = req
            .get("params")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let text = match name {
            "get_locker_plan" => initial_plan.to_string(),
            "set_locker_plan" => {
                written.fetch_add(1, Ordering::SeqCst);
                req["params"]["arguments"]["tasks"].to_string()
            }
            "get_rest_sessions" => "[]".into(),
            "request_rest" => {
                r#"{"sessionId":"test-session","eventId":"test-event","phase":"waiting"}"#.into()
            }
            _ => return (400, "unknown tool".into()),
        };
        (
            200,
            json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {"content": [{"type": "text", "text": text}]},
            })
            .to_string(),
        )
    })
}

static NO_WRITES: AtomicUsize = AtomicUsize::new(0);
static WRITES: AtomicUsize = AtomicUsize::new(0);

// 进程全局 env 覆盖：AW_BASE_URL 只读；mcp url 经全局槽位（fn 指针无法捕获）。
static MCP_URL: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
fn mcp_url_slot() -> Result<(String, Vec<(String, String)>), String> {
    MCP_URL
        .lock()
        .unwrap()
        .clone()
        .map(|u| (u, vec![]))
        .ok_or("no mock mcp url".into())
}

fn dir_env(dir: &std::path::Path) -> Env {
    Env::for_test(dir.to_path_buf(), http_get_real, mcp_url_slot)
}

fn http_get_real(url: &str) -> Result<Vec<u8>, String> {
    ureq::get(url)
        .call()
        .and_then(|mut r| r.body_mut().read_to_vec())
        .map_err(|e| e.to_string())
}

fn run_capture(env: &Env, args: &[&str]) -> (i32, String, String) {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run_with(env, &args, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn setup(
    events: Value,
    plan: Value,
    writes: &'static AtomicUsize,
) -> (Server, Server, Env, PathBuf) {
    let aw = aw_server(events);
    let mcp = mock_mcp(plan, writes);
    unsafe { std::env::set_var("AW_BASE_URL", &aw.url) };
    *MCP_URL.lock().unwrap() = Some(mcp.url.clone());
    let dir = tempfile::tempdir().unwrap().keep();
    (aw, mcp, dir_env(&dir), dir)
}

// ---- main_test.go 移植 ----

#[test]
fn not_due_exits_without_executing() {
    let (_aw, _mcp, env, _dir) = setup(json!([]), json!([]), &NO_WRITES);
    let (code, out, _) = run_capture(&env, &[]);
    assert_eq!(code, 0);
    let d: Decision = serde_json::from_str(&out).unwrap();
    assert!(!d.due);
}

#[test]
fn check_never_executes_even_when_due() {
    let (_aw, _mcp, env, dir) = setup(due_events(), json!([]), &NO_WRITES);
    let (code, out, _) = run_capture(&env, &["--check"]);
    assert_eq!(code, 0);
    let d: Decision = serde_json::from_str(&out).unwrap();
    assert!(d.due && d.rest_minutes == 15, "{d:?}");
    // --check 不写 pending、不写 history。
    assert!(!dir.join(PENDING_FILE).exists());
    assert!(!dir.join(HISTORY_FILE).exists());
}

#[test]
fn uncertain_execution_suppresses_due_decision() {
    let (_aw, _mcp, env, dir) = setup(due_events(), json!([]), &NO_WRITES);
    let p = Pending {
        session_id: String::new(),
        started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        expires_at: (Utc::now() + Duration::minutes(1)).to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    std::fs::write(dir.join(PENDING_FILE), serde_json::to_vec(&p).unwrap()).unwrap();
    let (code, out, _) = run_capture(&env, &[]);
    assert_eq!(code, 0);
    let d: Decision = serde_json::from_str(&out).unwrap();
    assert!(!d.due, "pending 防重应抑制 due: {d:?}");
}

#[test]
fn expired_protection_no_longer_suppresses_checks() {
    let (_aw, _mcp, env, dir) = setup(due_events(), json!([]), &NO_WRITES);
    let p = Pending {
        session_id: String::new(),
        started_at: "2000-01-01T00:00:00Z".into(),
        expires_at: "2000-01-01T00:00:00Z".into(),
    };
    std::fs::write(dir.join(PENDING_FILE), serde_json::to_vec(&p).unwrap()).unwrap();
    let (code, out, _) = run_capture(&env, &["--check"]);
    assert_eq!(code, 0);
    let d: Decision = serde_json::from_str(&out).unwrap();
    assert!(d.due, "过期 pending 不再抑制: {d:?}");
}

#[test]
fn corrupt_pending_fails_closed() {
    let (_aw, _mcp, env, dir) = setup(due_events(), json!([]), &NO_WRITES);
    std::fs::write(dir.join(PENDING_FILE), r#"{"expiresAt":"bad date"}"#).unwrap();
    let (code, _out, err) = run_capture(&env, &[]);
    assert_ne!(code, 0);
    assert!(err.contains("pending.json 无效"), "stderr = {err}");
}

#[test]
fn prune_expired_drops_past_once_keeps_future_and_recurring() {
    let now = Utc::now();
    let past = (now - Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let future = (now + Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true);
    let plan = json!([
        {"mode": "once", "when": {"at": past}, "unlock": {"at": past}},
        {"mode": "once", "when": {"at": future}, "unlock": {"at": future}},
        {"mode": "recurring", "when": {"days": ["mon"], "time": "09:00"},
         "unlock": {"days": ["mon"], "time": "09:10"}},
    ]);
    let kept = prune_expired(plan.as_array().unwrap().clone(), now);
    assert_eq!(
        kept.len(),
        2,
        "drop expired once, keep future once + recurring"
    );
    assert_eq!(kept[0]["mode"], "once");
    assert_eq!(kept[1]["mode"], "recurring");
}

#[test]
fn execute_plan_publishes_immutable_rest_request() {
    let (_aw, _mcp, env, dir) = setup(due_events(), json!([]), &NO_WRITES);
    let (code, _out, err) = run_capture(&env, &[]);
    assert_eq!(code, 0, "stderr = {err}");
    assert!(!dir.join(HISTORY_FILE).exists(), "事件源请求不写 history");
    assert!(
        !dir.join(PENDING_FILE).exists(),
        "请求被接受后应删除 pending"
    );
}

#[test]
fn run_does_not_rewrite_legacy_plan() {
    let (_aw, _mcp, env, _dir) = setup(due_events(), json!([]), &WRITES);
    let (code, _out, err) = run_capture(&env, &[]);
    assert_eq!(code, 0, "stderr = {err}");
    assert_eq!(
        WRITES.load(Ordering::SeqCst),
        0,
        "事件源 run 不得重写 legacy 计划"
    );
}

#[test]
fn cleanup_no_expired_no_write_back() {
    let writes: &'static AtomicUsize = Box::leak(Box::new(AtomicUsize::new(0)));
    let future = Utc::now() + Duration::hours(2);
    let initial = json!([{
        "mode": "once",
        "when": {"at": future.to_rfc3339_opts(SecondsFormat::Secs, true)},
        "unlock": {
            "at": (future + Duration::minutes(10)).to_rfc3339_opts(SecondsFormat::Secs, true),
        },
        "confirm": "未来",
    }]);
    let (_aw, mcp, env, _dir) = setup(due_events(), initial, writes);
    let _ = mcp;
    let mut err = Vec::new();
    cleanup_expired_plans(&env, &mut err);
    assert_eq!(writes.load(Ordering::SeqCst), 0, "无过期任务不应写回");
}

#[test]
fn write_json_atomic_crash_before_rename_keeps_old_file() {
    let dir = tempfile::tempdir().unwrap().keep();
    let path = dir.join("history.json");
    write_json_atomic(&path, &json!([{"v": "old"}])).unwrap();
    // 模拟崩溃：tmp 半截，rename 未执行
    std::fs::write(dir.join("history.json.tmp"), r#"[{"v": "n"#).unwrap();
    let got = read_history(&path).unwrap();
    assert_eq!(got, json!([{"v": "old"}]), "崩溃后目标文件被污染");
    write_json_atomic(&path, &json!([{"v": "new"}])).unwrap();
    let got = read_history(&path).unwrap();
    assert_eq!(got, json!([{"v": "new"}]), "恢复后写入失败");
}

// ---- check_test.go 尾部三个 HTTP/CLI 测试 ----

#[test]
fn fetch_raw_three_hours_unlimited() {
    // 记录请求参数：bucket 发现 + events 拉取两次调用。
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let now = now_fixed();
    let want_start = iso(now - Duration::hours(24));
    let want_end = iso(now);
    let server = serve(move |path, query, _body| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        if path.ends_with("/buckets/") {
            return (200, r#"["aw-watcher-afk_TEST"]"#.into());
        }
        assert!(
            path.contains("/buckets/aw-watcher-afk_TEST/events"),
            "{path}"
        );
        let q: std::collections::HashMap<_, _> = query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), v.replace("%2B", "+")))
            .collect();
        assert_eq!(q["start"], want_start);
        assert_eq!(q["end"], want_end);
        assert_eq!(q["limit"], "-1");
        let events = json!([{
            "timestamp": "2026-09-06T10:20:00+00:00",
            "duration": 6000,
            "data": {"status": "not-afk"},
        }]);
        (200, events.to_string())
    });
    unsafe { std::env::set_var("AW_BASE_URL", &server.url) };
    let env = dir_env(&tempfile::tempdir().unwrap().keep());
    let events = fetch_events(&env, now).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].duration, 6000.0);
    assert!(CALLS.load(Ordering::SeqCst) >= 2);
}

#[test]
fn find_afk_bucket_prefixed() {
    let server = serve(|path, _query, _body| {
        assert_eq!(path, "/api/0/buckets/");
        (200, r#"["aw-stopwatch","aw-watcher-afk_TEST"]"#.into())
    });
    unsafe { std::env::set_var("AW_BASE_URL", format!("{}/api/0", server.url)) };
    let env = dir_env(&tempfile::tempdir().unwrap().keep());
    assert_eq!(find_afk_bucket(&env).unwrap(), "aw-watcher-afk_TEST");
    drop(server);

    // 真实 AW 返回 map（bucket_id → 元数据）。
    let server = serve(|_path, _query, _body| {
        let buckets = json!({
            "aw-watcher-afk_DESKTOP-4J3EMM7": {"id": "aw-watcher-afk_DESKTOP-4J3EMM7"},
            "aw-watcher-window_DESKTOP-4J3EMM7": {"id": "aw-watcher-window_DESKTOP-4J3EMM7"},
        });
        (200, buckets.to_string())
    });
    unsafe { std::env::set_var("AW_BASE_URL", format!("{}/api/0", server.url)) };
    assert_eq!(
        find_afk_bucket(&env).unwrap(),
        "aw-watcher-afk_DESKTOP-4J3EMM7"
    );
    drop(server);

    let server = serve(|_p, _q, _b| (200, r#"["aw-watcher-window_TEST"]"#.into()));
    unsafe { std::env::set_var("AW_BASE_URL", format!("{}/api/0", server.url)) };
    assert!(find_afk_bucket(&env).is_err(), "no AFK bucket should error");
}

#[test]
fn cli_errors_emit_skip_json_and_exit_zero() {
    // AW 不可达：http_get 注入失败；MCP 可用（sessions=[]）。
    let mcp = mock_mcp(json!([]), &NO_WRITES);
    *MCP_URL.lock().unwrap() = Some(mcp.url.clone());
    fn fail_get(_: &str) -> Result<Vec<u8>, String> {
        Err("connection refused".into())
    }
    let dir = tempfile::tempdir().unwrap().keep();
    let env = Env::for_test(dir, fail_get, mcp_url_slot);
    let (code, out, _err) = run_capture(&env, &[]);
    assert_eq!(code, 0);
    let d: Decision = serde_json::from_str(&out).unwrap();
    assert!(
        !d.due && d.rest_minutes == 0 && d.reason.contains("error:"),
        "{d:?}"
    );
}
