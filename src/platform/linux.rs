use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;

use crate::keymap::RawKey;
use crate::platform::{BackendEvent, Grab};

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

fn open_and_grab_all() -> anyhow::Result<HashMap<PathBuf, evdev::Device>> {
    let mut grabbed: HashMap<PathBuf, evdev::Device> = HashMap::new();
    let devices: Vec<(PathBuf, evdev::Device)> = evdev::enumerate().collect();
    if devices.is_empty() {
        anyhow::bail!("no input devices found (need input group or root)");
    }
    for (path, mut dev) in devices {
        if is_ime_virtual(&dev) {
            continue;
        }
        if classify(&dev).is_none() {
            continue;
        }
        if let Err(e) = dev.grab() {
            // rollback
            for (_, mut d) in grabbed {
                let _ = d.ungrab();
            }
            anyhow::bail!("cannot grab {}: {} (check input group)", path.display(), e);
        }
        if let Err(e) = dev.set_nonblocking(true) {
            for (_, mut d) in grabbed {
                let _ = d.ungrab();
            }
            anyhow::bail!("cannot set nonblocking on {}: {}", path.display(), e);
        }
        grabbed.insert(path, dev);
    }
    if grabbed.is_empty() {
        anyhow::bail!("no grab-able keyboard/pointer devices");
    }
    Ok(grabbed)
}

fn raw_from_evdev(code: u16) -> Option<RawKey> {
    use crate::keymap::evdev;
    let raw = evdev::to_raw(code);
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
        if let Err(e) = dev.grab() {
            errors.push(format!("cannot grab new device {}: {}", path.display(), e));
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
                            {
                                let code = key_code.0;
                                if let Some(raw) = raw_from_evdev(code) {
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
