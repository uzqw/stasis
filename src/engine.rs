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
    pub focus_request: u64,
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
            focus_request: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub enum UiCmd {
    Lock,
    Unlock,
    ChangePassword { old: String, new: String },
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
                            update(&snapshot, |s| {
                                s.message = "请先设置密码".into();
                                s.ok = false;
                            });
                            continue;
                        }
                        match backend.start_lock() {
                            Ok(()) => {
                                keypad.reset();
                                update(&snapshot, |s| {
                                    s.locked = true;
                                    s.message = "已锁定".into();
                                    s.ok = true;
                                });
                            }
                            Err(e) => {
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
                        let now = Utc::now();
                        let (ok, msg) = controller.unlock(&mut backend, "manual", now);
                        keypad.cancel_unlock_mode();
                        let msg = unlock_display(ok, msg);
                        update(&snapshot, |s| {
                            s.locked = backend.is_locked();
                            s.unlock_mode = false;
                            s.password_len = 0;
                            s.message = msg;
                            s.ok = ok;
                        });
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
                                update(&snapshot, |s| {
                                    s.message = "密码已修改".into();
                                    s.ok = true;
                                });
                            }
                            Err(e) => {
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
                // Poll command file (throttled to avoid blocking the engine loop)
                if Instant::now().duration_since(last_poll) >= POLL_INTERVAL {
                    last_poll = Instant::now();
                    let dir = events_dir.parent().unwrap_or(&events_dir);
                    let _ = crate::protocol::poll_command(dir, |cmd| match cmd.cmd.as_str() {
                        "lock" => {
                            if !backend.is_locked() {
                                backend.start_lock()?;
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
                                let now = Utc::now();
                                let (ok, msg) = controller.unlock(&mut backend, "command", now);
                                keypad.cancel_unlock_mode();
                                let msg = unlock_display(ok, msg);
                                update(&snapshot, |s| {
                                    s.locked = backend.is_locked();
                                    s.unlock_mode = false;
                                    s.password_len = 0;
                                    s.message = msg;
                                    s.ok = ok;
                                });
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
                        update(&snapshot, |s| {
                            s.message = msg;
                            s.ok = false;
                        });
                    }
                    Ok(BackendEvent::Released) | Err(_) => {
                        // Backend died or was stopped
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
            update(snapshot, |s| {
                s.unlock_mode = true;
                s.password_len = 0;
                s.focus_request = s.focus_request.wrapping_add(1);
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
                let now = Utc::now();
                let (ok, msg) = controller.unlock(backend, "password", now);
                keypad.cancel_unlock_mode();
                let msg = unlock_display(ok, msg);
                update(snapshot, |s| {
                    s.locked = backend.is_locked();
                    s.unlock_mode = false;
                    s.password_len = 0;
                    s.message = msg;
                    s.ok = ok;
                });
            } else {
                keypad.cancel_unlock_mode();
                update(snapshot, |s| {
                    s.unlock_mode = false;
                    s.password_len = 0;
                    s.message = "密码错误 — 连按3次 CapsLock 重新解锁".into();
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
    }
}

/// Manual unlock shows a clear UI message; rest-session facts live in events.
fn unlock_display(ok: bool, msg: String) -> String {
    if ok { "已成功解锁".into() } else { msg }
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
