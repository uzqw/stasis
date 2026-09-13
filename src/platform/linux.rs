use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

fn classify(dev: &evdev::Device) -> Option<DeviceKind> {
    let keys = dev.supported_keys()?;
    let rel = dev.supported_relative_axes();
    let abs = dev.supported_absolute_axes();
    if keys.contains(evdev::KeyCode::KEY_A) && keys.contains(evdev::KeyCode::KEY_Z) {
        return Some(DeviceKind::Keyboard);
    }
    if keys.contains(evdev::KeyCode::BTN_LEFT)
        || keys.contains(evdev::KeyCode::BTN_TOUCH)
        || rel.map_or(false, |r| r.contains(evdev::RelativeAxisCode::REL_X))
        || abs.map_or(false, |a| a.contains(evdev::AbsoluteAxisCode::ABS_X))
    {
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

fn scan_new(grabbed: &mut HashMap<PathBuf, evdev::Device>) {
    for (path, mut dev) in evdev::enumerate() {
        if grabbed.contains_key(&path) {
            continue;
        }
        if classify(&dev).is_none() {
            continue;
        }
        if dev.grab().is_err() {
            continue;
        }
        let _ = dev.set_nonblocking(true);
        grabbed.insert(path, dev);
    }
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
                                        && (raw == RawKey::ShiftLeft
                                            || raw == RawKey::ShiftRight)
                                    {
                                        let _ = tx.send(BackendEvent::ShiftRelease);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => {
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
                scan_new(&mut devices);
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
