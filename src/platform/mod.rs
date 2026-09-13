use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, Sender};

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
#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "macos")]
pub mod macos;

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
