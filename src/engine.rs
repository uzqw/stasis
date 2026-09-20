use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::config::Config;
use crate::keypad::{Action, Keypad};
use crate::platform::{BackendEvent, Grab};
use crate::protocol::EventStore;
use crate::session::{Controller, LockOps};

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub locked: bool,
    pub unlock_mode: bool,
    pub password_len: usize,
    pub message: String,
    pub ok: bool,
    pub events_dir: PathBuf,
}

impl Snapshot {
    fn new(events_dir: PathBuf) -> Self {
        Self {
            locked: false,
            unlock_mode: false,
            password_len: 0,
            message: String::new(),
            ok: true,
            events_dir,
        }
    }
}

#[derive(Debug, Clone)]
pub enum UiCmd {
    Lock,
    Unlock,
    /// Input reached the app window while locked: capture is no longer
    /// reliable.  Re-arm the grab instead of silently leaking keystrokes.
    Rehook,
    ChangePassword {
        old: String,
        new: String,
    },
    Exit,
}

enum BackendState {
    Idle,
    Locked {
        _grab: Grab,
        rx: Receiver<BackendEvent>,
    },
}

struct BackendHandle {
    state: BackendState,
    _keepawake: Option<keepawake::KeepAwake>,
}

impl BackendHandle {
    fn new() -> Self {
        Self {
            state: BackendState::Idle,
            _keepawake: None,
        }
    }

    fn grab_alive(&self) -> bool {
        match &self.state {
            BackendState::Locked { _grab, .. } => _grab.is_alive(),
            _ => false,
        }
    }
}

impl LockOps for BackendHandle {
    fn is_locked(&self) -> bool {
        matches!(self.state, BackendState::Locked { .. })
    }

    fn start_lock(&mut self) -> anyhow::Result<()> {
        let (tx, rx) = unbounded();
        let grab = crate::platform::grab(tx)?;
        let ka = keepawake::Builder::default()
            .display(true)
            .idle(true)
            .app_name("stasis")
            .reason("locked")
            .create()
            .ok();
        self.state = BackendState::Locked { _grab: grab, rx };
        self._keepawake = ka;
        Ok(())
    }

    fn stop_lock(&mut self) -> anyhow::Result<()> {
        self.state = BackendState::Idle;
        self._keepawake = None;
        Ok(())
    }
}

impl BackendHandle {
    /// Replace the running grab with a fresh one, keeping the lock.  A fresh
    /// hook is the only remedy available for a backend that stopped seeing
    /// input, and the new grab also re-runs the platform's foreground handoff.
    fn restart_grab(&mut self) -> anyhow::Result<()> {
        self.stop_lock()?;
        self.start_lock()
    }
}

/// Engine owns all business state in a single background thread.
pub struct Engine {
    snapshot: Arc<Mutex<Snapshot>>,
    cmd_tx: Sender<UiCmd>,
}

impl Engine {
    pub fn new(config: Config) -> Self {
        let snapshot = Arc::new(Mutex::new(Snapshot::new(config.effective_events_dir())));
        let (cmd_tx, cmd_rx) = unbounded();
        let snap = snapshot.clone();
        std::thread::spawn(move || run(config, snap, cmd_rx));
        Self { snapshot, cmd_tx }
    }

    pub fn snapshot(&self) -> Arc<Mutex<Snapshot>> {
        self.snapshot.clone()
    }

    pub fn send(&self, cmd: UiCmd) {
        let _ = self.cmd_tx.send(cmd);
    }
}

fn run(mut config: Config, snapshot: Arc<Mutex<Snapshot>>, cmd_rx: Receiver<UiCmd>) {
    let events_dir = config.effective_events_dir();
    let store = EventStore::new(&events_dir);
    let _ = store.ensure_dirs();
    let mut controller = Controller::new(store);
    let mut backend = BackendHandle::new();
    let mut keypad = Keypad::new();
    let tick_rx = crossbeam_channel::tick(Duration::from_secs(1));
    let mut last_poll = Instant::now();
    const POLL_INTERVAL: Duration = Duration::from_secs(5);

    loop {
        // Determine which channels to select on this iteration.
        let backend_rx: Option<Receiver<BackendEvent>> = match &backend.state {
            BackendState::Locked { rx, .. } => Some(rx.clone()),
            _ => None,
        };

        let mut sel = crossbeam_channel::Select::new();
        let ui_idx = sel.recv(&cmd_rx);
        let tick_idx = sel.recv(&tick_rx);
        let backend_idx = backend_rx.as_ref().map(|rx| sel.recv(rx));

        let oper = sel.select();
        match oper.index() {
            i if i == ui_idx => match oper.recv(&cmd_rx) {
                Ok(UiCmd::Lock) => {
                    if !backend.is_locked() {
                        if config.password.is_empty() {
                            tracing::warn!("lock refused: no password configured");
                            update(&snapshot, |s| {
                                s.message = "请先设置密码".into();
                                s.ok = false;
                            });
                            continue;
                        }
                        match backend.start_lock() {
                            Ok(()) => {
                                tracing::info!("locked by ui");
                                keypad.reset();
                                update(&snapshot, |s| {
                                    s.locked = true;
                                    s.message = "已锁定".into();
                                    s.ok = true;
                                });
                            }
                            Err(e) => {
                                tracing::warn!("lock failed: {e}");
                                update(&snapshot, |s| {
                                    s.message = format!("锁定失败: {}", e);
                                    s.ok = false;
                                });
                            }
                        }
                    }
                }
                Ok(UiCmd::Unlock) => {
                    if backend.is_locked() {
                        perform_unlock(
                            "manual",
                            &mut keypad,
                            &mut backend,
                            &mut controller,
                            &snapshot,
                        );
                    }
                }
                Ok(UiCmd::Rehook) => {
                    if backend.is_locked() {
                        // Keystrokes that went to the window were never
                        // captured, so the buffer is missing characters: say so
                        // instead of comparing a silently wrong password.
                        tracing::warn!(
                            "input reached the window while locked; re-arming input capture"
                        );
                        keypad.clear_password();
                        match backend.restart_grab() {
                            Ok(()) => {
                                tracing::info!("input capture re-armed");
                                update(&snapshot, |s| {
                                    s.password_len = 0;
                                    s.message = "输入已重新捕获 — 请重新输入密码".into();
                                    s.ok = true;
                                });
                            }
                            Err(e) => {
                                // Without capture there is no lock to claim.
                                tracing::warn!("re-arming input capture failed: {e}");
                                let _ = backend.stop_lock();
                                keypad.device_lost();
                                update(&snapshot, |s| {
                                    s.locked = false;
                                    s.unlock_mode = false;
                                    s.password_len = 0;
                                    s.message = format!("重新捕获输入失败，已解除锁定: {e}");
                                    s.ok = false;
                                });
                            }
                        }
                    }
                }
                Ok(UiCmd::ChangePassword { old, new }) => {
                    if old != config.password {
                        update(&snapshot, |s| {
                            s.message = "当前密码错误".into();
                            s.ok = false;
                        });
                    } else {
                        let mut candidate = config.clone();
                        candidate.password = new;
                        match candidate.validate().and_then(|_| candidate.save()) {
                            Ok(()) => {
                                config.password = candidate.password;
                                tracing::info!("password changed");
                                update(&snapshot, |s| {
                                    s.message = "密码已修改".into();
                                    s.ok = true;
                                });
                            }
                            Err(e) => {
                                tracing::warn!("password change failed: {e}");
                                update(&snapshot, |s| {
                                    s.message = format!("保存失败: {}", e);
                                    s.ok = false;
                                });
                            }
                        }
                    }
                }
                Ok(UiCmd::Exit) | Err(_) => break,
            },
            i if i == tick_idx => {
                // Complete the selected operation before handling it;
                // dropping `oper` without `recv` panics in crossbeam.
                let _ = oper.recv(&tick_rx);
                let now = Utc::now();

                // Backend health check: if grab thread died without sending Released,
                // transition out of locked state so the UI is not stuck.
                if backend.is_locked() && !backend.grab_alive() {
                    tracing::warn!("grab thread died without reporting a reason; releasing lock");
                    let _ = backend.stop_lock();
                    keypad.device_lost();
                    update(&snapshot, |s| {
                        s.locked = false;
                        s.unlock_mode = false;
                        s.password_len = 0;
                        s.message = "输入后端异常退出".into();
                        s.ok = false;
                    });
                }

                let msgs = controller.tick(&mut backend, now);
                for (msg, ok) in msgs {
                    update(&snapshot, |s| {
                        s.locked = backend.is_locked();
                        s.message = msg;
                        s.ok = ok;
                    });
                }

                // ponytail: debug command file for headless verification;
                // ~/.stasis-cmd containing "lock"/"unlock" is consumed and
                // deleted once per second. Harmless when absent.
                if let Some(home) = std::env::var_os("HOME") {
                    let cmd_path = std::path::Path::new(&home).join(".stasis-cmd");
                    if let Ok(cmd) = std::fs::read_to_string(&cmd_path) {
                        let _ = std::fs::remove_file(&cmd_path);
                        match cmd.trim() {
                            "lock" if !backend.is_locked() => {
                                if let Err(e) = backend.start_lock() {
                                    update(&snapshot, |s| {
                                        s.message = format!("锁定失败: {}", e);
                                        s.ok = false;
                                    });
                                } else {
                                    keypad.reset();
                                    update(&snapshot, |s| {
                                        s.locked = true;
                                        s.message = "已锁定".into();
                                        s.ok = true;
                                    });
                                }
                            }
                            "unlock" if backend.is_locked() => {
                                perform_unlock(
                                    "manual",
                                    &mut keypad,
                                    &mut backend,
                                    &mut controller,
                                    &snapshot,
                                );
                            }
                            _ => {}
                        }
                    }
                }
                // Poll command file (throttled to avoid blocking the engine loop)
                if Instant::now().duration_since(last_poll) >= POLL_INTERVAL {
                    last_poll = Instant::now();
                    let dir = events_dir.parent().unwrap_or(&events_dir);
                    let _ = crate::protocol::poll_command(dir, |cmd| match cmd.cmd.as_str() {
                        "lock" => {
                            if !backend.is_locked() {
                                backend.start_lock()?;
                                tracing::info!("locked by command file");
                                keypad.reset();
                                update(&snapshot, |s| {
                                    s.locked = true;
                                    s.message = "命令触发锁定".into();
                                    s.ok = true;
                                });
                            }
                            Ok("ok".into())
                        }
                        "unlock" => {
                            if backend.is_locked() {
                                perform_unlock(
                                    "command",
                                    &mut keypad,
                                    &mut backend,
                                    &mut controller,
                                    &snapshot,
                                );
                            }
                            Ok("ok".into())
                        }
                        _ => Err(anyhow::anyhow!("unknown cmd")),
                    });
                }
            }
            i if backend_idx.map(|idx| idx == i).unwrap_or(false) => {
                let rx = backend_rx.as_ref().unwrap();
                match oper.recv(rx) {
                    Ok(BackendEvent::Key(key)) => {
                        let action = keypad.feed(Instant::now(), key);
                        handle_keypad_action(
                            action,
                            &mut keypad,
                            &mut backend,
                            &mut controller,
                            &snapshot,
                            &config,
                        );
                    }
                    Ok(BackendEvent::ShiftRelease) => {
                        keypad.shift_release();
                    }
                    Ok(BackendEvent::Health(msg)) => {
                        tracing::warn!("backend health: {}", msg);
                        update(&snapshot, |s| {
                            s.message = msg;
                            s.ok = false;
                        });
                    }
                    Ok(BackendEvent::Released) | Err(_) => {
                        // Backend died or was stopped
                        tracing::warn!("backend exited; releasing lock if held");
                        if backend.is_locked() {
                            let _ = backend.stop_lock();
                            keypad.device_lost();
                            update(&snapshot, |s| {
                                s.locked = false;
                                s.unlock_mode = false;
                                s.password_len = 0;
                                s.message = "输入后端已退出".into();
                                s.ok = false;
                            });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Cleanup on exit
    let _ = backend.stop_lock();
    update(&snapshot, |s| {
        s.locked = false;
    });
}

fn handle_keypad_action(
    action: Action,
    keypad: &mut Keypad,
    backend: &mut BackendHandle,
    controller: &mut Controller,
    snapshot: &Arc<Mutex<Snapshot>>,
    config: &Config,
) {
    match action {
        Action::UnlockMode => {
            tracing::info!("unlock mode armed by gesture");
            update(snapshot, |s| {
                s.unlock_mode = true;
                s.password_len = 0;
                s.message = "解锁模式已开启 — 请输入密码".into();
                s.ok = true;
            });
        }
        Action::Password { len } => {
            update(snapshot, |s| {
                s.password_len = len;
            });
        }
        Action::Submit => {
            let pwd = keypad.password().to_string();
            if pwd == config.password {
                perform_unlock("password", keypad, backend, controller, snapshot);
            } else {
                tracing::warn!("unlock rejected: wrong password");
                keypad.cancel_unlock_mode();
                update(snapshot, |s| {
                    s.unlock_mode = false;
                    s.password_len = 0;
                    s.message = "密码错误 — 连按3次 j 重新解锁".into();
                    s.ok = false;
                });
            }
        }
        Action::None => {}
    }
}

fn update<F>(snapshot: &Arc<Mutex<Snapshot>>, f: F)
where
    F: FnOnce(&mut Snapshot),
{
    if let Ok(mut s) = snapshot.lock() {
        f(&mut s);
        // ponytail: debug state dump for SSH verification; ~1 write per state
        // change, harmless on all platforms
        if let Some(home) = std::env::var_os("HOME") {
            let p = std::path::Path::new(&home).join(".stasis-state");
            let _ = std::fs::write(
                p,
                format!("locked={} msg={} ok={}", s.locked, s.message, s.ok),
            );
        }
    }
}

/// Manual unlock shows a clear UI message; rest-session facts live in events.
fn unlock_display(ok: bool, msg: String) -> String {
    if ok { "已成功解锁".into() } else { msg }
}

/// Unlock via the controller and mirror the result into the snapshot.
fn perform_unlock(
    reason: &str,
    keypad: &mut Keypad,
    backend: &mut BackendHandle,
    controller: &mut Controller,
    snapshot: &Arc<Mutex<Snapshot>>,
) {
    let now = Utc::now();
    let (ok, msg) = controller.unlock(backend, reason, now);
    // A lock transition only ever changed the UI snapshot, which no rig can
    // read — so every transition leaves a line here, with its reason.
    if ok {
        tracing::info!("unlocked ({reason})");
    } else {
        tracing::warn!("unlock failed ({reason}): {msg}");
    }
    keypad.cancel_unlock_mode();
    let msg = unlock_display(ok, msg);
    update(snapshot, |s| {
        s.locked = backend.is_locked();
        s.unlock_mode = false;
        s.password_len = 0;
        s.message = msg;
        s.ok = ok;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::Grab;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn backend_handle_grab_alive_true_while_running() {
        let (_tx, rx) = unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = thread::spawn(move || {
            while !stop2.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(10));
            }
        });
        let grab = Grab::new_mock(stop, Some(handle));
        let mut backend = BackendHandle::new();
        backend.state = BackendState::Locked { _grab: grab, rx };
        assert!(backend.grab_alive());
        backend.stop_lock().ok();
    }

    #[test]
    fn backend_handle_grab_alive_false_after_finish() {
        let (_tx, rx) = unbounded();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = thread::spawn(|| {});
        // Wait for thread to finish
        thread::sleep(Duration::from_millis(50));
        let grab = Grab::new_mock(stop, Some(handle));
        let mut backend = BackendHandle::new();
        backend.state = BackendState::Locked { _grab: grab, rx };
        assert!(!backend.grab_alive());
        backend.stop_lock().ok();
    }

    #[test]
    fn unlock_display_ok() {
        assert_eq!(unlock_display(true, "ignored".into()), "已成功解锁");
        assert_eq!(unlock_display(false, "error".into()), "error");
    }
}
