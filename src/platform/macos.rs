//! macOS input backend: a `kCGHIDEventTap` `CGEventTap` on a dedicated
//! thread's `CFRunLoop`.
//!
//! Contract mirrors [`linux`](super::linux), which the engine consumes:
//!
//! - presses are de-duplicated and auto-repeat is dropped (field 8
//!   `kCGKeyboardEventAutorepeat`, plus a held-key table as belt),
//! - a shift release arrives as a separate [`BackendEvent::ShiftRelease`]
//!   (reconstructed from `FlagsChanged` + NX device shift bits),
//! - dropping [`Grab`] sets the stop flag; a repeating `CFRunLoopTimer`
//!   notices and stops the run loop, with the shared join timeout as a
//!   safety valve.
//!
//! An event tap is not a security boundary.  ⌘⌥Esc (Force Quit),
//! Ctrl+⌘+Q / Ctrl+⇧+⌘+Q (lock screen), Touch ID and the loginwindow
//! secure attention path are handled below the HID tap and always stay
//! available.  While another process holds Secure Event Input
//! (`EnableSecureEventInput`, e.g. a password field), the tap goes deaf
//! until it is disabled; the engine keeps no illusions about that window.
//!
//! Creating a tap requires the Accessibility permission ("post events"
//! access).  Without it `CGEventTapCreate` returns NULL; we preflight
//! with `CGPreflightPostEventAccess` so the failure message can name the
//! exact System Settings pane.  The system may also disable an active tap
//! that stalls (`TapDisabledByTimeout`) or on user request
//! (`TapDisabledByUserInput`); both arrive out of band and the tap is
//! re-enabled, reported as [`BackendEvent::Health`].
//!
//! A run loop that stops servicing sources is the observable signal of a
//! dead tap, so the same timer that polls the stop flag answers watchdog
//! probes; a silent thread is reported as [`BackendEvent::Released`].
//!
//! All `unsafe` Core Graphics / Core Foundation calls are confined to
//! this file.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use core_foundation::base::TCFType;
use core_foundation::date::CFDate;
use core_foundation::mach_port::CFMachPortRef;
use core_foundation::runloop::{
    CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext, CFRunLoopTimerRef, kCFRunLoopCommonModes,
};
use core_graphics::event::{
    CGEvent, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};
use crossbeam_channel::{Sender, bounded};

use crate::keymap::{RawKey, mac as keymap};
use crate::platform::{BackendEvent, Grab, Liveness};

/// Stop-flag poll cadence; also the watchdog probe answer.
const TICK_INTERVAL: Duration = Duration::from_millis(250);
/// A tap thread that has not fired its timer within this window has
/// stopped servicing its run loop.
const PROBE_DEADLINE: Duration = Duration::from_millis(1500);

// NX device-dependent modifier bits carried in `CGEventFlags` for
// `FlagsChanged` events (IOLLEvent.h).  Each bit reports whether that
// *physical* shift key is currently down: a `FlagsChanged` for keycode
// 56 with the LSHIFT bit set means the left shift was just pressed.
// (Cross-checked against `modifierFlags == 131330` == shift|LSHIFT
// meaning "left shift is down".)
const NX_DEVICELSHIFTKEYMASK: u64 = 0x0000_0002;
const NX_DEVICERSHIFTKEYMASK: u64 = 0x0000_0004;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// True when the process may post (and therefore tap) events.
    fn CGPreflightPostEventAccess() -> bool;
    /// Ask the system for the access; triggers the permission prompt.
    /// The grant lands in System Settings only after user approval, so a
    /// `false` result here is expected on first run.
    fn CGRequestPostEventAccess() -> bool;
    /// Re-enable a tap.  `CGEventTap` keeps the port private, so the
    /// callback reaches it through this FFI instead.
    fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    /// Connect or disconnect the mouse from the on-screen cursor.
    /// Dropping `mouseMoved` events only hides them from apps; the
    /// WindowServer still moves the cursor from the same HID reports.
    /// Disconnecting is what actually freezes the pointer.
    fn CGAssociateMouseAndMouseCursorPosition(connected: bool) -> i32;
}

thread_local! {
    /// Raw mach port of the installed tap, so the `TapDisabledBy*` path
    /// in the callback can re-enable it.  Written once on the tap thread
    /// before the run loop starts.
    static TAP_PORT: RefCell<CFMachPortRef> = const { RefCell::new(std::ptr::null_mut()) };
}

/// Re-arm a tap the system disabled, from inside the tap callback.
fn reenable_tap(tx: &Sender<BackendEvent>) {
    TAP_PORT.with(|port| {
        let port = *port.borrow();
        if !port.is_null() {
            // SAFETY: `port` is the live tap's mach port, stored right
            // after creation on this same thread.
            unsafe { CGEventTapEnable(port, true) };
            let _ = tx.send(BackendEvent::Health(
                "macOS event tap was disabled by the system; re-enabled".into(),
            ));
        }
    });
}

/// Timer context: shares ownership of the stop flag and probe counter
/// with `grab`/`watchdog`.
struct TickContext {
    stop: Arc<AtomicBool>,
    probes: Arc<AtomicU64>,
}

/// `extern "C"` timer callout: answers one watchdog probe per tick and
/// stops the run loop once the grab is dropped.
extern "C" fn tick(_timer: CFRunLoopTimerRef, info: *mut c_void) {
    // SAFETY: `info` points at the `TickContext` owned by `run_tap`'s
    // stack frame; the timer stops firing when the run loop exits, which
    // is before that frame returns.
    let ctx = unsafe { &*(info as *const TickContext) };
    ctx.probes.fetch_add(1, Ordering::Relaxed);
    if ctx.stop.load(Ordering::Relaxed) {
        CFRunLoop::get_current().stop();
    }
}

pub fn grab(tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    // Preflight only triggers the system prompt on first run; it is NOT
    // the gate.  On macOS 26 the preflight result is cached per process
    // and does not go live-update after the user grants access, so a
    // stale `false` here must not stop us — `CGEventTapCreate` returning
    // NULL is the real verdict (Quinn, Apple DTS).
    // SAFETY: no pointers involved.
    if !unsafe { CGPreflightPostEventAccess() } {
        // SAFETY: no pointers involved.
        let _ = unsafe { CGRequestPostEventAccess() };
    }

    let stop = Arc::new(AtomicBool::new(false));
    let probes = Arc::new(AtomicU64::new(0));
    let (ready_tx, ready_rx) = bounded::<Result<(), String>>(1);

    let handle = thread::spawn({
        let stop = stop.clone();
        let probes = probes.clone();
        let tx = tx.clone();
        move || run_tap(stop, probes, ready_tx, tx)
    });
    // The watchdog starts only after the tap thread reports ready, so a
    // tap-creation failure does not trigger a false "wedged run loop".
    thread::spawn({
        let stop = stop.clone();
        let ready = ready_rx.clone();
        move || {
            if matches!(ready.recv(), Ok(Ok(()))) {
                watchdog(stop, probes, tx)
            }
        }
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
            stop.store(true, Ordering::Relaxed);
            let _ = handle.join();
            anyhow::bail!("macOS event tap thread did not become ready in 5s")
        }
    }
}

/// Backend thread body: install the tap on this thread's run loop, run
/// until stopped, then release.
fn run_tap(
    stop: Arc<AtomicBool>,
    probes: Arc<AtomicU64>,
    ready_tx: Sender<Result<(), String>>,
    tx: Sender<BackendEvent>,
) {
    let held = RefCell::new([false; 256]);
    let cb_tx = tx.clone();
    let held_ref = &held;
    let callback = move |_proxy, etype: CGEventType, event: &CGEvent| -> CallbackResult {
        match etype {
            // Out-of-band: the system disabled the tap; re-arm it.
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                reenable_tap(&cb_tx);
                CallbackResult::Keep
            }
            CGEventType::KeyDown => {
                if event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0 {
                    return CallbackResult::Drop;
                }
                let keycode =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                let mut held = held_ref.borrow_mut();
                let Some(slot) = held.get_mut(keycode as usize) else {
                    return CallbackResult::Drop;
                };
                if *slot {
                    return CallbackResult::Drop; // repeat without the flag
                }
                *slot = true;
                let raw = keymap::to_raw(keycode);
                if raw != RawKey::Other {
                    // Only the normalized physical key leaves this
                    // callback; keystrokes are never logged.
                    let _ = cb_tx.send(BackendEvent::Key(raw));
                }
                CallbackResult::Drop
            }
            CGEventType::KeyUp => {
                let keycode =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                if let Some(slot) = held_ref.borrow_mut().get_mut(keycode as usize) {
                    *slot = false;
                }
                CallbackResult::Drop
            }
            CGEventType::FlagsChanged => {
                let keycode =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
                let flags = event.get_flags().bits();
                let down = match keycode {
                    keymap::VK_SHIFT => flags & NX_DEVICELSHIFTKEYMASK != 0,
                    keymap::VK_RIGHT_SHIFT => flags & NX_DEVICERSHIFTKEYMASK != 0,
                    _ => return CallbackResult::Drop, // caps lock, cmd, …
                };
                let mut held = held_ref.borrow_mut();
                let Some(slot) = held.get_mut(keycode as usize) else {
                    return CallbackResult::Drop;
                };
                if down {
                    if !*slot {
                        *slot = true;
                        let _ = cb_tx.send(BackendEvent::Key(RawKey::ShiftLeft));
                    }
                } else {
                    *slot = false;
                    // Reported even when the press predates the grab, so
                    // the shared Keypad never keeps a phantom shift.
                    let _ = cb_tx.send(BackendEvent::ShiftRelease);
                }
                CallbackResult::Drop
            }
            _ => CallbackResult::Drop, // pointer, scroll, everything else
        }
    };

    // SAFETY: the callback only touches `held` and `cb_tx`, both owned by
    // this thread's frame and outliving the tap (the tap is dropped
    // first, below).  The tap only ever fires on this thread's run loop.
    let tap = match unsafe {
        CGEventTap::new_unchecked(
            CGEventTapLocation::HID,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::Default,
            vec![
                CGEventType::KeyDown,
                CGEventType::KeyUp,
                CGEventType::FlagsChanged,
                CGEventType::LeftMouseDown,
                CGEventType::LeftMouseUp,
                CGEventType::RightMouseDown,
                CGEventType::RightMouseUp,
                CGEventType::MouseMoved,
                CGEventType::LeftMouseDragged,
                CGEventType::RightMouseDragged,
                CGEventType::ScrollWheel,
                CGEventType::OtherMouseDown,
                CGEventType::OtherMouseUp,
                CGEventType::OtherMouseDragged,
            ],
            callback,
        )
    } {
        Ok(tap) => tap,
        Err(()) => {
            let _ = ready_tx.send(Err(
                "CGEventTapCreate failed; check System Settings → Privacy & \
                 Security → Accessibility"
                    .into(),
            ));
            return;
        }
    };

    let run_loop = CFRunLoop::get_current();
    let source = match tap.mach_port().create_runloop_source(0) {
        Ok(source) => source,
        Err(()) => {
            let _ = ready_tx.send(Err("CFMachPortCreateRunLoopSource failed".into()));
            return;
        }
    };
    // SAFETY: `kCFRunLoopCommonModes` is the standard "all modes" constant.
    unsafe { run_loop.add_source(&source, kCFRunLoopCommonModes) };

    // The callback needs the port to re-enable a disabled tap.
    TAP_PORT.with(|p| *p.borrow_mut() = tap.mach_port().as_concrete_TypeRef());

    // Repeating timer: polls the stop flag and answers watchdog probes.
    let mut tick_ctx = Box::new(TickContext { stop, probes });
    let mut timer_ctx = CFRunLoopTimerContext {
        version: 0,
        info: &mut *tick_ctx as *mut TickContext as *mut c_void,
        retain: None,
        release: None,
        copyDescription: None,
    };
    let now = CFDate::now().abs_time();
    let interval = TICK_INTERVAL.as_secs_f64();
    let timer = CFRunLoopTimer::new(now + interval, interval, 0, 0, tick, &mut timer_ctx);
    // SAFETY: same common modes as the tap source.
    unsafe { run_loop.add_timer(&timer, kCFRunLoopCommonModes) };

    tap.enable();

    // Dropping mouseMoved hides it from apps but the cursor still tracks
    // the HID reports; decouple it so the lock owns the pointer too.
    // SAFETY: plain FFI, no pointers.
    unsafe { CGAssociateMouseAndMouseCursorPosition(false) };

    let _ = ready_tx.send(Ok(()));

    CFRunLoop::run_current();

    // The run loop has exited, so neither callout can fire again; the
    // context and the tap may now be dropped.  `tap`'s Drop invalidates
    // the mach port.
    // Reconnect the cursor before releasing the tap so the pointer is
    // never left frozen if we exit unexpectedly.
    // SAFETY: plain FFI, no pointers.
    unsafe { CGAssociateMouseAndMouseCursorPosition(true) };

    TAP_PORT.with(|p| *p.borrow_mut() = std::ptr::null_mut());
    drop(timer);
    drop(tick_ctx);
    drop(tap);
    let _ = tx.send(BackendEvent::Released);
}

/// Watchdog: the tap thread answers a probe every timer tick; silence
/// means the run loop is wedged and capture is no longer reliable.
fn watchdog(stop: Arc<AtomicBool>, probes: Arc<AtomicU64>, tx: Sender<BackendEvent>) {
    let mut liveness = Liveness::new(PROBE_DEADLINE, Instant::now());
    let mut answered = probes.load(Ordering::Relaxed);
    loop {
        thread::sleep(TICK_INTERVAL);
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        let current = probes.load(Ordering::Relaxed);
        if current != answered {
            answered = current;
            liveness.seen(now);
        } else if liveness.stalled(now) {
            let _ = tx.send(BackendEvent::Health(
                "macOS event tap run loop stopped answering probes; \
                 input capture is no longer reliable"
                    .into(),
            ));
            let _ = tx.send(BackendEvent::Released);
            return;
        }
    }
}
