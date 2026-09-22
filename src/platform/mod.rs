use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{Sender, bounded};

use crate::keymap::RawKey;

/// Event from a platform backend.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// A key was pressed (de-duplicated, no auto-repeat).
    Key(RawKey),
    /// Left or right shift was released.
    ShiftRelease,
    /// The grab has ended (device lost, error, or stopped).
    Released,
    /// Non-fatal health report from the backend (e.g. hot-plug failure).
    Health(String),
}

/// Handle to an active grab.  Dropping it releases input.
pub struct Grab {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Grab {
    pub fn is_alive(&self) -> bool {
        self.handle
            .as_ref()
            .map(|h| !h.is_finished())
            .unwrap_or(false)
    }

    #[cfg(test)]
    pub(crate) fn new_mock(stop: Arc<AtomicBool>, handle: Option<JoinHandle<()>>) -> Self {
        Self { stop, handle }
    }
}

impl Drop for Grab {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join_timeout(Duration::from_secs(3));
        }
    }
}

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

/// Platform-specific grab constructor.
#[cfg(target_os = "linux")]
pub fn grab(tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    linux::grab(tx)
}

#[cfg(target_os = "windows")]
pub fn grab(tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    windows::grab(tx)
}

#[cfg(target_os = "macos")]
pub fn grab(tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    macos::grab(tx)
}

/// Hand the foreground to another window so the platform backend keeps
/// receiving keys.  Windows stops delivering input to a low-level hook while
/// the hooking process owns the foreground; other platforms have no such
/// rule and this is a no-op.
#[cfg(target_os = "windows")]
pub fn release_foreground() {
    unsafe { windows::release_foreground() }
}

#[cfg(not(target_os = "windows"))]
pub fn release_foreground() {}

/// Liveness watchdog for backends that own a native message loop.
///
/// Windows removes a low-level hook whose callback exceeds
/// `LowLevelHooksTimeout` and never notifies the process; a wedged macOS
/// run loop goes just as silent, so the only observable signal is that
/// the backend thread stops answering probes.  A backend feeds probe
/// responses in with [`Liveness::seen`] and treats [`Liveness::stalled`]
/// as "capture is no longer reliable".
#[cfg(any(target_os = "windows", target_os = "macos", test))]
#[derive(Debug)]
pub(crate) struct Liveness {
    deadline: Duration,
    last_seen: std::time::Instant,
}

#[cfg(any(target_os = "windows", target_os = "macos", test))]
impl Liveness {
    /// Arm at `now`; a fresh watchdog is stalled once `deadline` passes with
    /// no probe response, so a thread that never answers is still detected.
    pub(crate) fn new(deadline: Duration, now: std::time::Instant) -> Self {
        Self {
            deadline,
            last_seen: now,
        }
    }

    /// Record a probe response.
    pub(crate) fn seen(&mut self, now: std::time::Instant) {
        self.last_seen = now;
    }

    /// True once no probe has been answered within `deadline`.
    pub(crate) fn stalled(&self, now: std::time::Instant) -> bool {
        now.saturating_duration_since(self.last_seen) > self.deadline
    }
}

// Helper: thread join with timeout using crossbeam.
trait JoinTimeout {
    fn join_timeout(self, timeout: Duration) -> thread::Result<()>;
}

impl JoinTimeout for JoinHandle<()> {
    fn join_timeout(self, timeout: Duration) -> thread::Result<()> {
        let (tx, rx) = bounded(1);
        thread::spawn(move || {
            let _ = tx.send(self.join());
        });
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(_) => {
                // Leak the thread; OS will clean up on process exit.
                // This is a safety valve, not normal flow.
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Liveness;
    use std::time::{Duration, Instant};

    #[test]
    fn liveness_flags_a_silent_thread() {
        let start = Instant::now();
        let mut liveness = Liveness::new(Duration::from_secs(1), start);
        assert!(!liveness.stalled(start + Duration::from_millis(999)));
        assert!(liveness.stalled(start + Duration::from_millis(1001)));

        // Any probe response re-arms the deadline.
        let pong = start + Duration::from_secs(5);
        liveness.seen(pong);
        assert!(!liveness.stalled(pong + Duration::from_millis(999)));
        assert!(liveness.stalled(pong + Duration::from_millis(1001)));
    }
}
