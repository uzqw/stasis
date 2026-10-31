//! AFK publisher for ActivityWatch (Linux/evdev).
//!
//! Omarchy runs Hyprland, which has no global input API.  The upstream
//! `aw-watcher-afk` reads X11 through pynput, so under Wayland it sees no
//! input and reports the user as permanently active.  This module reads the
//! same evdev devices the locker reads (read-only, never grabbing them) and
//! posts `afkstatus` heartbeats to `aw-server` / `aw-server-rust`, which is
//! what `rest-break` consumes to compute work time.
//!
//! This is a display-side helper: Stasis locking does not depend on it.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::json;

/// Default ActivityWatch API root (same port for aw-server and aw-server-rust).
pub const DEFAULT_AW_BASE: &str = "http://localhost:5600/api/0";
/// Seconds without input before the user counts as AFK (upstream default).
pub const DEFAULT_TIMEOUT_SECS: u64 = 180;
/// How often input devices are polled.
pub const DEFAULT_POLL_SECS: u64 = 5;

#[derive(Debug, Clone)]
pub struct Settings {
    pub base_url: String,
    pub hostname: String,
    pub timeout: Duration,
    pub poll: Duration,
}

impl Settings {
    pub fn new(
        base_url: impl Into<String>,
        hostname: impl Into<String>,
        timeout: Duration,
        poll: Duration,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            hostname: hostname.into(),
            timeout,
            poll,
        }
    }

    /// Bucket name ActivityWatch clients expect: `<client>_<hostname>`.
    pub fn bucket(&self) -> String {
        format!("aw-watcher-afk_{}", self.hostname)
    }

    /// How long a heartbeat may overlap the previous event and still merge.
    /// Must cover the timeout, otherwise the AFK event starts a new bucket
    /// event every poll instead of extending one.
    pub fn pulsetime_secs(&self) -> u64 {
        self.timeout.as_secs() + self.poll.as_secs()
    }

    pub fn bucket_body(&self) -> serde_json::Value {
        json!({
            "client": "aw-watcher-afk",
            "hostname": self.hostname,
            "type": "afkstatus",
        })
    }
}

/// AFK once the last input is at least `timeout` old.
pub fn is_afk(idle: Duration, timeout: Duration) -> bool {
    idle >= timeout
}

/// One heartbeat event.  `start` is the beginning of the current AFK or
/// not-AFK stretch; ActivityWatch merges overlapping heartbeats into it.
pub fn heartbeat_event(afk: bool, start: DateTime<Utc>, now: DateTime<Utc>) -> serde_json::Value {
    let duration = (now - start).num_milliseconds().max(0) as f64 / 1000.0;
    json!({
        "timestamp": start.to_rfc3339_opts(SecondsFormat::Millis, true),
        "duration": duration,
        "data": {"status": if afk { "afk" } else { "not-afk" }},
    })
}

pub fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(target_os = "linux")]
fn post_json(url: &str, body: &serde_json::Value) -> Result<u16, String> {
    let request = ureq::post(url)
        .config()
        .timeout_global(Some(Duration::from_secs(3)))
        .build()
        .header("Content-Type", "application/json");
    match request.send_json(body) {
        Ok(response) => Ok(response.status().as_u16()),
        // ureq treats non-2xx as an error; 409 is "bucket already exists",
        // which is the normal case after the first run.
        Err(ureq::Error::StatusCode(code)) => Ok(code),
        Err(e) => Err(e.to_string()),
    }
}

/// Create the AFK bucket.  Existing buckets answer 409 (aw-server) or 304
/// (aw-server-rust, "not modified"), both of which mean "ready".
#[cfg(target_os = "linux")]
pub fn ensure_bucket(settings: &Settings) -> Result<(), String> {
    let url = format!("{}/buckets/{}", settings.base_url, settings.bucket());
    match post_json(&url, &settings.bucket_body())? {
        200..=299 | 304 | 409 => Ok(()),
        code => Err(format!("bucket creation returned HTTP {code}")),
    }
}

#[cfg(target_os = "linux")]
pub fn post_heartbeat(settings: &Settings, event: &serde_json::Value) -> Result<(), String> {
    let url = format!(
        "{}/buckets/{}/heartbeat?pulsetime={}",
        settings.base_url,
        settings.bucket(),
        settings.pulsetime_secs()
    );
    match post_json(&url, event)? {
        200..=299 => Ok(()),
        code => Err(format!("heartbeat returned HTTP {code}")),
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Devices that can produce human input.  Pure EV_ABS devices are skipped:
    /// accelerometers and ambient-light sensors emit absolute axes without any
    /// user involvement and would keep the session "active" forever.
    fn is_activity_device(dev: &evdev::Device) -> bool {
        let Some(keys) = dev.supported_keys() else {
            return false;
        };
        let typing = keys.contains(evdev::KeyCode::KEY_A)
            && keys.contains(evdev::KeyCode::KEY_Z)
            && keys.contains(evdev::KeyCode::KEY_SPACE)
            && keys.contains(evdev::KeyCode::KEY_ENTER);
        if typing {
            return true;
        }
        let buttons = keys.contains(evdev::KeyCode::BTN_LEFT)
            || keys.contains(evdev::KeyCode::BTN_TOUCH)
            || keys.contains(evdev::KeyCode::BTN_SOUTH);
        let relative = dev
            .supported_relative_axes()
            .is_some_and(|r| r.contains(evdev::RelativeAxisCode::REL_X));
        buttons || relative
    }

    pub struct InputMonitor {
        devices: HashMap<PathBuf, evdev::Device>,
        scans: u32,
    }

    impl InputMonitor {
        pub fn open() -> anyhow::Result<Self> {
            let mut monitor = Self {
                devices: HashMap::new(),
                scans: 0,
            };
            monitor.scan();
            if monitor.devices.is_empty() {
                anyhow::bail!(
                    "no readable input devices (need input group or root); \
                     AFK detection would report nothing"
                );
            }
            Ok(monitor)
        }

        fn scan(&mut self) {
            for (path, dev) in evdev::enumerate() {
                if self.devices.contains_key(&path) || !is_activity_device(&dev) {
                    continue;
                }
                // `enumerate()` opens blocking fds; without this the first
                // idle device parks the whole watcher in read().
                if dev.set_nonblocking(true).is_err() {
                    continue;
                }
                self.devices.insert(path, dev);
            }
            self.scans += 1;
        }

        /// True when any device produced at least one event since the last call.
        pub fn poll_activity(&mut self) -> bool {
            // Hotplug: re-scan about once a minute at the default poll rate.
            if self.scans == 0 || self.scans.is_multiple_of(12) {
                self.scan();
            }
            self.scans = self.scans.wrapping_add(1);

            let mut active = false;
            let mut dead = Vec::new();
            for (path, dev) in self.devices.iter_mut() {
                match dev.fetch_events() {
                    Ok(events) => {
                        if events.count() > 0 {
                            active = true;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    // A device that disappeared (unplug, suspend) must not
                    // keep the whole watcher from polling the others.
                    Err(_) => dead.push(path.clone()),
                }
            }
            for path in dead {
                self.devices.remove(&path);
            }
            active
        }
    }
}

/// Run the AFK watcher forever (until killed).  Stderr gets one line per
/// distinct failure so a stopped server does not flood the journal every poll.
pub fn run(settings: Settings) -> anyhow::Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = settings;
        anyhow::bail!("stasis afk is only implemented for Linux (evdev)");
    }

    #[cfg(target_os = "linux")]
    {
        let mut monitor = linux::InputMonitor::open()?;
        let mut last_input = Utc::now();
        let mut afk = false;
        let mut afk_start = last_input;
        let mut bucket_ready = false;
        let mut last_error: Option<String> = None;

        eprintln!(
            "stasis afk: publishing to {}/buckets/{} (timeout {}s, poll {}s)",
            settings.base_url,
            settings.bucket(),
            settings.timeout.as_secs(),
            settings.poll.as_secs()
        );

        loop {
            if monitor.poll_activity() {
                last_input = Utc::now();
            }
            let now = Utc::now();
            let idle = (now - last_input).to_std().unwrap_or(Duration::ZERO);
            let next_afk = is_afk(idle, settings.timeout);
            if next_afk != afk {
                afk = next_afk;
                if afk {
                    afk_start = last_input;
                }
                eprintln!("stasis afk: {}", if afk { "afk" } else { "not-afk" });
            }

            if !bucket_ready {
                match ensure_bucket(&settings) {
                    Ok(()) => {
                        bucket_ready = true;
                        last_error = None;
                    }
                    Err(e) => {
                        if last_error.as_deref() != Some(e.as_str()) {
                            eprintln!("stasis afk: cannot create bucket: {e}");
                            last_error = Some(e);
                        }
                    }
                }
            }

            if bucket_ready {
                let start = if afk { afk_start } else { last_input };
                let event = heartbeat_event(afk, start, now);
                match post_heartbeat(&settings, &event) {
                    Ok(()) => last_error = None,
                    Err(e) => {
                        if last_error.as_deref() != Some(e.as_str()) {
                            eprintln!("stasis afk: heartbeat failed: {e}");
                            last_error = Some(e);
                        }
                        // Recreate the bucket after a server restart.
                        bucket_ready = false;
                    }
                }
            }

            std::thread::sleep(settings.poll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 31, 6, 0, 0).unwrap() + chrono::Duration::seconds(secs)
    }

    fn settings() -> Settings {
        Settings::new(
            "http://localhost:5600/api/0/",
            "box",
            Duration::from_secs(180),
            Duration::from_secs(5),
        )
    }

    #[test]
    fn bucket_name_and_pulsetime_match_activitywatch_conventions() {
        let s = settings();
        assert_eq!(s.bucket(), "aw-watcher-afk_box");
        assert_eq!(s.pulsetime_secs(), 185);
        // Trailing slash in the base URL must not double up.
        assert_eq!(s.base_url, "http://localhost:5600/api/0");
        assert_eq!(s.bucket_body()["type"], "afkstatus");
        assert_eq!(s.bucket_body()["client"], "aw-watcher-afk");
    }

    #[test]
    fn afk_threshold_is_inclusive() {
        assert!(!is_afk(Duration::from_secs(179), Duration::from_secs(180)));
        assert!(is_afk(Duration::from_secs(180), Duration::from_secs(180)));
        assert!(is_afk(Duration::from_secs(181), Duration::from_secs(180)));
    }

    #[test]
    fn heartbeat_covers_the_stretch_since_last_input() {
        let event = heartbeat_event(false, at(0), at(42));
        assert_eq!(event["data"]["status"], "not-afk");
        assert_eq!(event["duration"], 42.0);
        assert_eq!(event["timestamp"], "2026-10-31T06:00:00.000Z");

        let event = heartbeat_event(true, at(10), at(70));
        assert_eq!(event["data"]["status"], "afk");
        assert_eq!(event["duration"], 60.0);
    }

    #[test]
    fn heartbeat_never_has_negative_duration() {
        let event = heartbeat_event(true, at(10), at(5));
        assert_eq!(event["duration"], 0.0);
    }
}
