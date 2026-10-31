#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Stasis entry point.
//!
//! `stasis` (no arguments) keeps the desktop UI: tray icon by default, the
//! old always-open window with `--windowed`.  The Omarchy port uses three
//! additional entry points so a Quickshell bar widget can drive the engine
//! without a GUI process:
//!
//! - `stasis daemon`   — engine only, no window and no tray icon.
//! - `stasis rest [N]` — ask the engine for an N-minute rest (writes the
//!   same `rest.requested` event aide writes).
//! - `stasis status`   — print the runtime snapshot the bar widget reads.
//! - `stasis afk`      — publish AFK heartbeats to ActivityWatch so the bar
//!   can show how long the current work stretch is (Linux/evdev).
//!
//! `stasis lock` / `stasis unlock` use the existing command-file protocol.

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Duration as ChronoDuration, Utc};
use tracing::{error, info};
use uuid::Uuid;

use stasis::config::Config;
use stasis::protocol::{self, EventStore, REQUESTED};
use stasis::status::Status;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("daemon") | Some("--daemon") => {
            let _log_guard = init_logging();
            run_daemon();
        }
        Some("status") => std::process::exit(cli_status(args.iter().any(|a| a == "--json"))),
        Some("rest") => std::process::exit(cli_rest(&args[1..])),
        Some("afk") => std::process::exit(cli_afk(&args[1..])),
        Some("lock") => std::process::exit(cli_command("lock")),
        Some("unlock") => std::process::exit(cli_command("unlock")),
        _ => run_gui(&args),
    }
}

fn init_logging() -> tracing_appender::non_blocking::WorkerGuard {
    let data_dir = match Config::config_path() {
        Ok(p) => p.parent().unwrap_or(&p).to_path_buf(),
        Err(_) => std::env::temp_dir().join("stasis"),
    };
    let _ = std::fs::create_dir_all(&data_dir);
    let file_appender = tracing_appender::rolling::daily(&data_dir, "stasis.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    // Held by the caller for the process lifetime so buffered lines flush on
    // a normal exit.
    _guard
}

/// Engine-only mode for the Omarchy shell.  The bar widget is the whole UI;
/// this process owns the lock state, the evdev grab, and the status file.
fn run_daemon() {
    info!("Stasis daemon starting");
    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {}", e);
            Config::default()
        }
    };
    if cfg.password.is_empty() {
        info!("No password set; locking is disabled until one is configured");
    }
    let _engine = stasis::engine::Engine::new(cfg);
    // No signal handler: `systemctl --user stop` terminates the process and
    // the kernel closes the evdev fds, which releases the grab.  That is the
    // safe direction (unlock on exit), so blocking forever is enough.
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn run_gui(args: &[String]) {
    let _log_guard = init_logging();

    let cfg = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {}", e);
            Config::default()
        }
    };
    if cfg.password.is_empty() {
        info!("No password set; locking is disabled until one is configured");
    }

    let engine = Arc::new(stasis::engine::Engine::new(cfg));

    // Default: tray-icon mode (menu bar icon + native dropdown). Pass
    // --windowed to keep the original always-open 400x620 window.
    let windowed = args.iter().any(|a| a == "--windowed");
    if !windowed {
        stasis::ui_tray::run(engine);
        return;
    }

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([400.0, 620.0])
        .with_resizable(false)
        .with_title("Stasis");
    // Avoid decoding/retaining eframe's generic egui Dock icon; use the OS icon.
    #[cfg(target_os = "macos")]
    let viewport = viewport.with_icon(egui::IconData::default());
    let native_options = eframe::NativeOptions {
        viewport,
        #[cfg(target_os = "macos")]
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Stasis",
        native_options,
        Box::new(|cc| {
            stasis::ui::install_style(&cc.egui_ctx);
            Ok(Box::new(stasis::ui::App::new(engine)))
        }),
    );
}

/// Print the runtime snapshot.  Exit code 0 only when the daemon is alive.
fn cli_status(json: bool) -> i32 {
    let path = Config::status_path();
    let fresh = Status::read(&path).filter(|s| s.is_fresh(ChronoDuration::seconds(5)));
    match fresh {
        Some(status) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&status).unwrap_or_default()
                );
            } else {
                let session = status
                    .session
                    .as_ref()
                    .map(|s| format!(" {} until {}", s.phase, s.unlock_at))
                    .unwrap_or_default();
                println!(
                    "running pid={} locked={} ok={}{} message={}",
                    status.pid, status.locked, status.ok, session, status.message
                );
            }
            0
        }
        None => {
            if json {
                println!(r#"{{"schema":"stasis.status/v1","running":false}}"#);
            } else {
                println!("stasis: not running");
            }
            1
        }
    }
}

/// Ask the engine for a rest of `minutes` starting now.
///
/// This writes the same `rest.requested` event aide writes; the engine
/// projects it and performs the lock, so there is no second control path.
fn cli_rest(args: &[String]) -> i32 {
    let force = args.iter().any(|a| a == "--force");
    let minutes = args
        .iter()
        .find_map(|a| a.parse::<u32>().ok())
        .unwrap_or(20)
        .clamp(1, 240);

    if !force {
        let alive = Status::read(&Config::status_path())
            .is_some_and(|s| s.is_fresh(ChronoDuration::seconds(5)));
        if !alive {
            eprintln!(
                "stasis: daemon is not running; start it first \
                 (systemctl --user start stasis) or pass --force"
            );
            return 1;
        }
    }

    let cfg = Config::load().unwrap_or_default();
    let store = EventStore::new(cfg.effective_events_dir());
    if let Err(e) = store.ensure_dirs() {
        eprintln!("stasis: cannot create event directory: {e}");
        return 1;
    }

    let now = Utc::now();
    let session_id = Uuid::new_v4().to_string().replace('-', "");
    let data = serde_json::json!({
        "lockAt": stasis::status::iso(now),
        "unlockAt": stasis::status::iso(now + ChronoDuration::minutes(minutes as i64)),
        "source": "omarchy-bar",
        "reason": "manual",
    });
    match store.emit(
        &store.requests_dir(),
        session_id.as_str(),
        REQUESTED,
        data,
        now,
        None,
    ) {
        Ok(event) => {
            println!("{}", event.session_id);
            0
        }
        Err(e) => {
            eprintln!("stasis: cannot write rest request: {e}");
            1
        }
    }
}

/// Publish AFK status to ActivityWatch (see `stasis::afk`).
///
/// Runs forever; the user unit restarts it.  Flags are read positionally so
/// the command stays dependency-free: `--timeout SECS`, `--poll SECS`,
/// `--aw URL`.
fn cli_afk(args: &[String]) -> i32 {
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let timeout = value("--timeout")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(stasis::afk::DEFAULT_TIMEOUT_SECS)
        .max(10);
    let poll = value("--poll")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(stasis::afk::DEFAULT_POLL_SECS)
        .max(1);
    let base = value("--aw")
        .or_else(|| std::env::var("AW_BASE_URL").ok())
        .unwrap_or_else(|| stasis::afk::DEFAULT_AW_BASE.to_string());
    let settings = stasis::afk::Settings::new(
        base,
        stasis::afk::hostname(),
        Duration::from_secs(timeout),
        Duration::from_secs(poll),
    );
    match stasis::afk::run(settings) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("stasis afk: {e}");
            1
        }
    }
}

/// Send `lock` / `unlock` through the command-file protocol and wait for the
/// engine's ack.  A missing ack means the daemon is not running.
fn cli_command(cmd: &str) -> i32 {
    let cfg = Config::load().unwrap_or_default();
    let events_dir = cfg.effective_events_dir();
    let dir = events_dir.parent().unwrap_or(&events_dir).to_path_buf();
    let id = Uuid::new_v4().to_string().replace('-', "");

    if let Err(e) = protocol::write_command(&dir, cmd, &id) {
        eprintln!("stasis: cannot write command file: {e}");
        return 1;
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(ack) = protocol::read_ack(&dir)
            && ack.id == id
        {
            if ack.result == "ok" {
                return 0;
            }
            eprintln!("stasis: {cmd} failed: {}", ack.error.unwrap_or(ack.result));
            return 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    eprintln!("stasis: no ack from the engine; is `stasis daemon` running?");
    1
}
