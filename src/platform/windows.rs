//! Windows input backend: `WH_KEYBOARD_LL` + `WH_MOUSE_LL` low-level hooks.
//!
//! Contract mirrors [`linux`](super::linux), which the engine consumes:
//!
//! - presses are de-duplicated and auto-repeat is dropped
//!   ([`crate::keymap::win::HeldKeys`]),
//! - a shift release arrives as a separate [`BackendEvent::ShiftRelease`],
//! - dropping [`Grab`] unhooks and ends the message loop, with the shared
//!   join timeout as a safety valve.
//!
//! Low-level hooks are not a security boundary.  The secure attention
//! sequence (Ctrl+Alt+Del) is handled by winlogon and never reaches a hook,
//! so the system exit stays available by construction; this backend does not
//! try to intercept it.
//!
//! Windows silently removes a hook whose callback exceeds
//! `LowLevelHooksTimeout` and gives the process no notification (MSDN,
//! `LowLevelKeyboardProc`).  What is observable is the precondition: a
//! callback that runs too long, and a hook thread that has stopped answering
//! probes.  Both are reported as [`BackendEvent::Health`], and a silent thread
//! additionally as [`BackendEvent::Released`], so the engine never keeps
//! claiming a lock the backend no longer holds.
//!
//! All `unsafe` Win32 calls are confined to this file.

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, bounded};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, MSG, PM_REMOVE,
    PeekMessageW, PostThreadMessageW, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_QUIT,
};

use crate::keymap::win::{HeldKeys, Transition, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP};
use crate::platform::{BackendEvent, Grab, Liveness};

/// Thread message used to probe the hook thread; must be >= `WM_APP`.
const WM_APP_PROBE: u32 = WM_APP + 0x51;
/// Probe cadence.
const PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// A hook thread that has not answered a probe within this window has stopped
/// servicing its message loop.  Kept above the maximum `LowLevelHooksTimeout`
/// (1000 ms on Windows 10 1709+) so a slow callback is not a false positive.
const PROBE_DEADLINE: Duration = Duration::from_millis(1500);
/// A callback above this latency is reported once.  Windows already starts
/// treating a low-level hook as timed out well below it (the default
/// `LowLevelHooksTimeout` is 300 ms).
const CALLBACK_WARN: Duration = Duration::from_millis(100);

thread_local! {
    /// Per-hook-thread callback state.  A hook procedure is a bare `fn`, so
    /// thread locals are the only channel in.  Everything here is O(1): no
    /// disk I/O, no logging, no blocking waits.
    static HOOK: RefCell<HookState> = RefCell::new(HookState::default());
}

#[derive(Default)]
struct HookState {
    tx: Option<Sender<BackendEvent>>,
    held: HeldKeys,
    probes: Option<Arc<AtomicU64>>,
    slow_warned: bool,
}

impl HookState {
    fn on_key(&mut self, msg: u32, vk: u32) {
        let event = match self.held.record(msg, vk) {
            Transition::Press(key) => BackendEvent::Key(key),
            Transition::ShiftRelease => BackendEvent::ShiftRelease,
            Transition::Ignore => return,
        };
        if let Some(tx) = &self.tx {
            // Only the normalized physical key leaves this function, and only
            // to the engine.  Passwords and raw keystrokes are never logged.
            let _ = tx.send(event);
        }
    }

    fn warn_slow(&mut self, elapsed: Duration) {
        if self.slow_warned || elapsed < CALLBACK_WARN {
            return;
        }
        self.slow_warned = true;
        if let Some(tx) = &self.tx {
            let _ = tx.send(BackendEvent::Health(format!(
                "Windows hook callback took {} ms; Windows may silently remove \
                 a low-level hook once its callback exceeds LowLevelHooksTimeout",
                elapsed.as_millis()
            )));
        }
    }
}

/// `WH_KEYBOARD_LL` procedure.  Swallows every key; only normalized presses
/// and shift releases reach the engine.
unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        // Windows requires unprocessed notifications to be passed along.
        return unsafe { CallNextHookEx(None, code, wparam, lparam) };
    }
    let started = Instant::now();
    let msg = wparam.0 as u32;
    if matches!(msg, WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP) {
        // SAFETY: for WH_KEYBOARD_LL, lparam points at a KBDLLHOOKSTRUCT.
        let vk = unsafe { (*(lparam.0 as *const KBDLLHOOKSTRUCT)).vkCode };
        let _ = HOOK.try_with(|state| {
            if let Ok(mut state) = state.try_borrow_mut() {
                state.on_key(msg, vk);
                state.warn_slow(started.elapsed());
            }
        });
    }
    LRESULT(1) // swallow: the lock owns the keyboard
}

/// `WH_MOUSE_LL` procedure.  Swallows movement, buttons and wheel.
unsafe extern "system" fn mouse_proc(code: i32, _wparam: WPARAM, _lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(None, code, _wparam, _lparam) };
    }
    LRESULT(1) // swallow: the lock owns the pointer
}

struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
}

impl Hooks {
    /// Install both hooks.  Must run on the thread that will pump messages.
    unsafe fn install() -> Result<Self, String> {
        // SAFETY: both procedures match HOOKPROC; `hMod` is NULL for
        // in-process low-level hooks.
        let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) }
            .map_err(|e| format!("SetWindowsHookExW(WH_KEYBOARD_LL) failed: {e}"))?;
        let mouse = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) } {
            Ok(hook) => hook,
            Err(e) => {
                // Partial install must roll back, not leave the pointer hooked.
                let _ = unsafe { UnhookWindowsHookEx(keyboard) };
                return Err(format!("SetWindowsHookExW(WH_MOUSE_LL) failed: {e}"));
            }
        };
        Ok(Self { keyboard, mouse })
    }

    /// Remove both hooks.  Must run on the installing thread.
    unsafe fn remove(&self) {
        // SAFETY: both handles came from `install` above.
        let _ = unsafe { UnhookWindowsHookEx(self.keyboard) };
        let _ = unsafe { UnhookWindowsHookEx(self.mouse) };
    }
}

pub fn grab(tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    let stop = Arc::new(AtomicBool::new(false));
    let hook_stop = stop.clone();
    let probe_stop = stop.clone();
    let thread_id = Arc::new(AtomicU32::new(0));
    let hook_thread_id = thread_id.clone();
    let probe_thread_id = thread_id.clone();
    let probes = Arc::new(AtomicU64::new(0));
    let hook_probes = probes.clone();

    let (ready_tx, ready_rx) = bounded::<Result<(), String>>(1);
    let hook_tx = tx.clone();
    let probe_tx = tx.clone();

    let handle = thread::spawn(move || {
        // SAFETY: no pointers involved.
        let tid = unsafe { GetCurrentThreadId() };
        hook_thread_id.store(tid, Ordering::Relaxed);
        // Force the message queue into existence before the watchdog can post
        // a probe to this thread.
        unsafe {
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE);
        }
        HOOK.with(|state| {
            let mut state = state.borrow_mut();
            state.tx = Some(hook_tx);
            state.probes = Some(hook_probes);
        });

        // SAFETY: installs on the thread that owns the message loop below.
        let hooks = match unsafe { Hooks::install() } {
            Ok(hooks) => hooks,
            Err(err) => {
                let _ = ready_tx.send(Err(err));
                return;
            }
        };
        let _ = ready_tx.send(Ok(()));

        thread::spawn(move || watchdog(probe_stop, probe_thread_id, probes, probe_tx));

        let mut msg = MSG::default();
        loop {
            if hook_stop.load(Ordering::Relaxed) {
                break;
            }
            // SAFETY: `msg` is a valid MSG for the duration of the call.
            let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if result.0 <= 0 {
                break; // 0 == WM_QUIT, -1 == error
            }
            if msg.message == WM_APP_PROBE {
                HOOK.with(|state| {
                    if let Ok(state) = state.try_borrow()
                        && let Some(probes) = &state.probes
                    {
                        probes.fetch_add(1, Ordering::Relaxed);
                    }
                });
                continue;
            }
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // SAFETY: same thread that installed the hooks.
        unsafe { hooks.remove() };
        let _ = tx.send(BackendEvent::Released);
    });

    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Grab {
            stop,
            handle: Some(handle),
        }),
        Ok(Err(err)) => {
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
            anyhow::bail!(err)
        }
        Err(_) => {
            // Timed out before ready: cancel whatever installs late, then
            // wake the message loop so it cannot keep swallowing input.
            stop.store(true, Ordering::Relaxed);
            post(thread_id.load(Ordering::Relaxed), WM_QUIT);
            anyhow::bail!("Windows hook thread did not become ready in 5s")
        }
    }
}

/// Watchdog: probes the hook thread and reports when it stops answering.
fn watchdog(
    stop: Arc<AtomicBool>,
    thread_id: Arc<AtomicU32>,
    probes: Arc<AtomicU64>,
    tx: Sender<BackendEvent>,
) {
    let mut liveness = Liveness::new(PROBE_DEADLINE, Instant::now());
    let mut answered = probes.load(Ordering::Relaxed);
    loop {
        if stop.load(Ordering::Relaxed) {
            // Wake the message loop so `Grab::drop` can join it promptly.
            post(thread_id.load(Ordering::Relaxed), WM_QUIT);
            return;
        }
        post(thread_id.load(Ordering::Relaxed), WM_APP_PROBE);
        thread::sleep(PROBE_INTERVAL);
        let now = Instant::now();
        let current = probes.load(Ordering::Relaxed);
        if current != answered {
            answered = current;
            liveness.seen(now);
        } else if liveness.stalled(now) {
            // A silently removed hook has no notification; the observable
            // signal is that the installing thread no longer answers.
            let _ = tx.send(BackendEvent::Health(
                "Windows hook thread stopped answering probes; \
                 input capture is no longer reliable"
                    .into(),
            ));
            let _ = tx.send(BackendEvent::Released);
            post(thread_id.load(Ordering::Relaxed), WM_QUIT);
            return;
        }
    }
}

/// Post a thread message, ignoring a failed post (the queue may be gone).
fn post(thread_id: u32, message: u32) {
    if thread_id == 0 {
        return;
    }
    // SAFETY: no pointers are involved.
    let _ = unsafe { PostThreadMessageW(thread_id, message, WPARAM(0), LPARAM(0)) };
}
