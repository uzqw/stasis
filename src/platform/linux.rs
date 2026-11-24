use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use crate::keymap::RawKey;
use crate::platform::{BackendEvent, Grab, GrabBusy};

/// Maximum wait inside a single `grab()` call for physically held keys/buttons
/// to be released before deferring to the next engine tick.
const QUIET_GRAB_TIMEOUT: Duration = Duration::from_millis(500);
/// Poll cadence while waiting for held keys/buttons to release.
const QUIET_POLL_INTERVAL: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq)]
enum DeviceKind {
    Keyboard,
    Pointer,
}

/// Heuristic: skip known virtual/input-method devices to avoid breaking
/// fcitx/ibus on Wayland. These create uinput virtual keyboards that
/// must remain ungrabbed for composed input to work.
fn is_ime_virtual(dev: &evdev::Device) -> bool {
    let Some(name) = dev.name() else { return false };
    let lower = name.to_lowercase();
    lower.contains("fcitx")
        || lower.contains("ibus")
        || lower.contains("input method")
        || lower.contains("uinput")
}

fn classify(dev: &evdev::Device) -> Option<DeviceKind> {
    let keys = dev.supported_keys()?;
    // Broader keyboard detection: look for common typing keys.
    let has_typing_keys = keys.contains(evdev::KeyCode::KEY_A)
        && keys.contains(evdev::KeyCode::KEY_Z)
        && keys.contains(evdev::KeyCode::KEY_SPACE)
        && keys.contains(evdev::KeyCode::KEY_ENTER);
    if has_typing_keys {
        return Some(DeviceKind::Keyboard);
    }
    // Pointer detection: buttons or axes.
    let has_pointer = keys.contains(evdev::KeyCode::BTN_LEFT)
        || keys.contains(evdev::KeyCode::BTN_TOUCH)
        || dev
            .supported_relative_axes()
            .is_some_and(|r| r.contains(evdev::RelativeAxisCode::REL_X))
        || dev
            .supported_absolute_axes()
            .is_some_and(|a| a.contains(evdev::AbsoluteAxisCode::ABS_X));
    if has_pointer {
        return Some(DeviceKind::Pointer);
    }
    None
}

/// Passive stylus/trackpad tool-presence indicators (`BTN_TOOL_*`) can remain
/// asserted while a pen merely rests near a tablet without any button or touch
/// contact.  All other `EV_KEY` codes represent active keys or buttons.
fn is_tool_proximity(key: evdev::KeyCode) -> bool {
    matches!(
        key.code(),
        0x140..=0x148 | 0x14d..=0x14f
    )
}

fn has_active_keys_in<I: IntoIterator<Item = evdev::KeyCode>>(keys: I) -> bool {
    keys.into_iter().any(|k| !is_tool_proximity(k))
}

/// Query kernel key/button state via `EVIOCGKEY`.
///
/// `EVIOCGRAB` does not synthesize key-up events to other `evdev` clients
/// (`libinput` / the Wayland compositor).  Grabbing while a key is pressed
/// swallows the subsequent physical release, leaving Wayland clients'
/// userspace key-repeat timers firing forever.
fn has_pressed_keys(dev: &evdev::Device) -> io::Result<bool> {
    dev.get_key_state()
        .map(|state| has_active_keys_in(state.iter()))
}

trait GrabTarget {
    fn has_pressed_keys(&self) -> io::Result<bool>;
    fn grab(&mut self) -> io::Result<()>;
    fn ungrab(&mut self) -> io::Result<()>;
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()>;
}

impl GrabTarget for evdev::Device {
    fn has_pressed_keys(&self) -> io::Result<bool> {
        has_pressed_keys(self)
    }

    fn grab(&mut self) -> io::Result<()> {
        evdev::Device::grab(self)
    }

    fn ungrab(&mut self) -> io::Result<()> {
        evdev::Device::ungrab(self)
    }

    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        evdev::Device::set_nonblocking(self, nonblocking)
    }
}

fn rollback_slice<D: GrabTarget>(devices: &mut [(PathBuf, D)], count: usize) {
    for (_, dev) in &mut devices[..count] {
        let _ = dev.ungrab();
    }
}

/// Grab every candidate device only once all pressed keys/buttons are released,
/// and re-verify `EVIOCGKEY` right after each `EVIOCGRAB` to close the microsecond
/// race between checking key state and grabbing.
fn grab_all_when_quiet<D: GrabTarget>(
    candidates: &mut [(PathBuf, D)],
    timeout: Duration,
    poll: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut any_pressed = false;
        for (path, dev) in candidates.iter() {
            match dev.has_pressed_keys() {
                Ok(true) => {
                    any_pressed = true;
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    anyhow::bail!("cannot query key state on {}: {}", path.display(), e);
                }
            }
        }

        if !any_pressed {
            let mut raced = false;
            for i in 0..candidates.len() {
                if let Err(e) = candidates[i].1.grab() {
                    let path = candidates[i].0.display().to_string();
                    rollback_slice(candidates, i);
                    anyhow::bail!("cannot grab {}: {} (check input group)", path, e);
                }
                // Re-check while grabbed: if a key went down in the tiny window
                // before EVIOCGRAB, ungrab immediately so the physical key-up
                // still reaches libinput/compositor.
                match candidates[i].1.has_pressed_keys() {
                    Ok(true) => {
                        rollback_slice(candidates, i + 1);
                        raced = true;
                        break;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        let path = candidates[i].0.display().to_string();
                        rollback_slice(candidates, i + 1);
                        anyhow::bail!("cannot verify key state on {}: {}", path, e);
                    }
                }
                if let Err(e) = candidates[i].1.set_nonblocking(true) {
                    let path = candidates[i].0.display().to_string();
                    rollback_slice(candidates, i + 1);
                    anyhow::bail!("cannot set nonblocking on {}: {}", path, e);
                }
            }
            if !raced {
                return Ok(());
            }
        }

        if Instant::now() >= deadline {
            return Err(anyhow::Error::new(GrabBusy(
                "input keys or buttons are still pressed; deferring grab".into(),
            )));
        }
        thread::sleep(poll);
    }
}

fn open_and_grab_all() -> anyhow::Result<HashMap<PathBuf, evdev::Device>> {
    let devices: Vec<(PathBuf, evdev::Device)> = evdev::enumerate().collect();
    if devices.is_empty() {
        anyhow::bail!("no input devices found (need input group or root)");
    }
    let mut candidates: Vec<(PathBuf, evdev::Device)> = devices
        .into_iter()
        .filter(|(_, dev)| !is_ime_virtual(dev) && classify(dev).is_some())
        .collect();
    if candidates.is_empty() {
        anyhow::bail!("no grab-able keyboard/pointer devices");
    }
    grab_all_when_quiet(&mut candidates, QUIET_GRAB_TIMEOUT, QUIET_POLL_INTERVAL)?;
    Ok(candidates.into_iter().collect())
}

fn raw_from_evdev(key: evdev::KeyCode) -> Option<RawKey> {
    let raw = crate::keymap::evdev::to_raw(key);
    if raw == RawKey::Other {
        None
    } else {
        Some(raw)
    }
}

/// Scan for newly plugged devices. Returns newly-grabbed paths and errors.
fn scan_new(grabbed: &mut HashMap<PathBuf, evdev::Device>) -> (Vec<PathBuf>, Vec<String>) {
    let mut added = Vec::new();
    let mut errors = Vec::new();
    for (path, mut dev) in evdev::enumerate() {
        if grabbed.contains_key(&path) {
            continue;
        }
        if is_ime_virtual(&dev) {
            continue;
        }
        if classify(&dev).is_none() {
            continue;
        }
        // Defer grabbing a hot-plugged device while a key/button is held down;
        // the next scan will pick it up cleanly once released.
        if matches!(has_pressed_keys(&dev), Ok(true)) {
            continue;
        }
        if let Err(e) = dev.grab() {
            errors.push(format!("cannot grab new device {}: {}", path.display(), e));
            continue;
        }
        if matches!(has_pressed_keys(&dev), Ok(true)) {
            let _ = dev.ungrab();
            continue;
        }
        if let Err(e) = dev.set_nonblocking(true) {
            errors.push(format!(
                "cannot set nonblocking on new device {}: {}",
                path.display(),
                e
            ));
            let _ = dev.ungrab();
            continue;
        }
        added.push(path.clone());
        grabbed.insert(path, dev);
    }
    (added, errors)
}

pub fn grab(tx: Sender<BackendEvent>) -> anyhow::Result<Grab> {
    let mut devices = open_and_grab_all()?;
    let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stop2 = stop.clone();

    let handle = thread::spawn(move || {
        let mut last_scan = Instant::now();
        ready_tx.send(()).ok();

        loop {
            if stop2.load(Ordering::Relaxed) {
                break;
            }

            // Poll all open devices
            let mut to_remove = Vec::new();
            for (path, dev) in devices.iter_mut() {
                match dev.fetch_events() {
                    Ok(iter) => {
                        for ev in iter {
                            if let evdev::EventSummary::Key(_key_ev, key_code, value) =
                                ev.destructure()
                                && let Some(raw) = raw_from_evdev(key_code)
                            {
                                if value == 1 {
                                    let _ = tx.send(BackendEvent::Key(raw));
                                } else if value == 0
                                    && (raw == RawKey::ShiftLeft || raw == RawKey::ShiftRight)
                                {
                                    let _ = tx.send(BackendEvent::ShiftRelease);
                                }
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        let _ = tx.send(BackendEvent::Health(format!(
                            "device {} error: {}",
                            path.display(),
                            e
                        )));
                        to_remove.push(path.clone());
                    }
                }
            }
            for p in to_remove {
                devices.remove(&p);
                if devices.is_empty() {
                    let _ = tx.send(BackendEvent::Released);
                    return;
                }
            }

            if last_scan.elapsed() >= Duration::from_secs(2) {
                last_scan = Instant::now();
                let (_added, errors) = scan_new(&mut devices);
                for err in errors {
                    let _ = tx.send(BackendEvent::Health(err));
                }
            }

            thread::sleep(Duration::from_millis(50));
        }

        // Cleanup: ungrab before exit so inputs are not left blocked.
        for (_, mut dev) in devices {
            let _ = dev.ungrab();
        }
        let _ = tx.send(BackendEvent::Released);
    });

    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| anyhow::anyhow!("evdev grab thread did not become ready in 5s"))?;

    Ok(Grab {
        stop,
        handle: Some(handle),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::is_grab_busy;
    use std::cell::Cell;

    struct MockDevice {
        /// Sequence of `has_pressed_keys` answers; once exhausted, `steady` is returned.
        pressed_seq: RefCellVec<bool>,
        steady: bool,
        fail_grab: bool,
        grabbed: bool,
        grabs: Cell<usize>,
        ungrabs: Cell<usize>,
    }

    struct RefCellVec<T>(std::cell::RefCell<std::collections::VecDeque<T>>);

    impl<T> RefCellVec<T> {
        fn new(items: impl IntoIterator<Item = T>) -> Self {
            Self(std::cell::RefCell::new(items.into_iter().collect()))
        }

        fn pop(&self) -> Option<T> {
            self.0.borrow_mut().pop_front()
        }
    }

    impl MockDevice {
        fn new(seq: impl IntoIterator<Item = bool>, steady: bool) -> Self {
            Self {
                pressed_seq: RefCellVec::new(seq),
                steady,
                fail_grab: false,
                grabbed: false,
                grabs: Cell::new(0),
                ungrabs: Cell::new(0),
            }
        }
    }

    impl GrabTarget for MockDevice {
        fn has_pressed_keys(&self) -> io::Result<bool> {
            Ok(self.pressed_seq.pop().unwrap_or(self.steady))
        }

        fn grab(&mut self) -> io::Result<()> {
            if self.fail_grab {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
            }
            self.grabbed = true;
            self.grabs.set(self.grabs.get() + 1);
            Ok(())
        }

        fn ungrab(&mut self) -> io::Result<()> {
            self.grabbed = false;
            self.ungrabs.set(self.ungrabs.get() + 1);
            Ok(())
        }

        fn set_nonblocking(&self, _nonblocking: bool) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn active_key_filter_catches_keys_and_buttons_but_ignores_tool_presence() {
        use evdev::KeyCode as K;
        assert!(!has_active_keys_in([]));
        assert!(has_active_keys_in([K::KEY_SPACE]));
        assert!(has_active_keys_in([K::KEY_BACKSPACE]));
        assert!(has_active_keys_in([K::KEY_LEFTSHIFT]));
        assert!(has_active_keys_in([K::BTN_LEFT]));
        assert!(has_active_keys_in([K::BTN_TOUCH]));
        assert!(has_active_keys_in([K::BTN_STYLUS]));
        // Passive proximity flags must not block locking when a pen rests nearby.
        assert!(!has_active_keys_in([
            K::BTN_TOOL_PEN,
            K::BTN_TOOL_FINGER,
            K::BTN_TOOL_DOUBLETAP,
        ]));
        assert!(has_active_keys_in([K::BTN_TOOL_PEN, K::KEY_SPACE]));
    }

    #[test]
    fn waits_for_held_space_to_release_before_grabbing() {
        // Space is held across 3 pre-checks, then released cleanly.
        let mut candidates = vec![(
            PathBuf::from("/dev/input/event4"),
            MockDevice::new([true, true, true, false, false], false),
        )];
        grab_all_when_quiet(
            &mut candidates,
            Duration::from_millis(200),
            Duration::from_millis(1),
        )
        .expect("should grab once key releases");
        assert!(candidates[0].1.grabbed);
        assert_eq!(
            candidates[0].1.grabs.get(),
            1,
            "must not call EVIOCGRAB while key is still down"
        );
        assert_eq!(candidates[0].1.ungrabs.get(), 0);
    }

    #[test]
    fn ungrabs_and_retries_when_key_press_races_with_grab() {
        // Pre-check sees false, then key goes down right before grab() so
        // post-check sees true; must ungrab immediately and re-grab after release.
        let mut candidates = vec![
            (
                PathBuf::from("/dev/input/event3"),
                MockDevice::new([], false),
            ),
            (
                PathBuf::from("/dev/input/event4"),
                // Attempt 1: pre-check=false, post-check=true (raced!)
                // Attempt 2: pre-check=true (still held)
                // Attempt 3: pre-check=false, post-check=false (clean!)
                MockDevice::new([false, true, true, false, false], false),
            ),
        ];
        grab_all_when_quiet(
            &mut candidates,
            Duration::from_millis(200),
            Duration::from_millis(1),
        )
        .expect("should recover from TOCTOU race and grab cleanly");
        assert!(candidates[0].1.grabbed);
        assert!(candidates[1].1.grabbed);
        assert_eq!(candidates[1].1.ungrabs.get(), 1);
        assert_eq!(candidates[0].1.ungrabs.get(), 1, "first device rolled back");
    }

    #[test]
    fn held_key_past_timeout_returns_grab_busy_without_leaving_grabs() {
        let mut candidates = vec![(
            PathBuf::from("/dev/input/event4"),
            MockDevice::new([], true),
        )];
        let err = grab_all_when_quiet(
            &mut candidates,
            Duration::from_millis(15),
            Duration::from_millis(2),
        )
        .expect_err("continuously held key must not be force-grabbed");
        assert!(is_grab_busy(&err));
        assert!(!candidates[0].1.grabbed);
        assert_eq!(candidates[0].1.grabs.get(), 0);
    }

    #[test]
    fn hard_grab_error_rolls_back_immediately() {
        let mut bad = MockDevice::new([], false);
        bad.fail_grab = true;
        let mut candidates = vec![
            (
                PathBuf::from("/dev/input/event3"),
                MockDevice::new([], false),
            ),
            (PathBuf::from("/dev/input/event4"), bad),
        ];
        let err = grab_all_when_quiet(
            &mut candidates,
            Duration::from_secs(5),
            Duration::from_millis(50),
        )
        .expect_err("permission failure must fail immediately");
        assert!(!is_grab_busy(&err));
        assert!(!candidates[0].1.grabbed);
        assert_eq!(candidates[0].1.ungrabs.get(), 1);
    }
}
