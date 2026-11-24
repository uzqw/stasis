//! Desktop notifications for upcoming scheduled rests.
//!
//! Scheduled rests give the user a one-minute heads-up before `EVIOCGRAB` /
//! input hooks freeze the keyboard and mouse.  Notifications are best-effort
//! and fire-and-forget on a detached worker thread so a hung D-Bus session or
//! missing helper binary can never stall the engine loop or leave zombie
//! child processes behind.

use std::process::Command;
use std::thread;

/// Details of an upcoming scheduled rest entering its pre-lock warning window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreLockNotice {
    pub session_id: String,
    pub seconds_until_lock: u64,
    pub rest_minutes: u64,
}

/// Build the user-facing notification headline and body.
pub fn format_pre_lock(notice: &PreLockNotice) -> (String, String) {
    let summary = "即将锁屏休息".to_string();
    let minutes = notice.rest_minutes.max(1);
    let body = if notice.seconds_until_lock >= 45 {
        format!("1 分钟后将锁定键鼠休息 {minutes} 分钟，请提前保存当前工作。")
    } else {
        let secs = notice.seconds_until_lock.max(1);
        format!("{secs} 秒后将锁定键鼠休息 {minutes} 分钟，请提前保存当前工作。")
    };
    (summary, body)
}

/// Dispatch a non-blocking desktop notification for an upcoming lock.
pub fn send_pre_lock(notice: &PreLockNotice) {
    let (summary, body) = format_pre_lock(notice);
    tracing::info!(
        session_id = %notice.session_id,
        seconds_until_lock = notice.seconds_until_lock,
        rest_minutes = notice.rest_minutes,
        "sending pre-lock desktop notification"
    );
    thread::spawn(move || {
        dispatch_os_notification(&summary, &body);
    });
}

#[cfg(target_os = "linux")]
fn dispatch_os_notification(summary: &str, body: &str) {
    // Multi-monitor path on Omarchy: `omarchy.notifications` only renders its
    // D-Bus toast deck on the single focused output, whereas `stasis.rest` has
    // a live bar-widget instance on every connected screen and broadcasts its
    // layer-shell pre-lock banner across all monitors simultaneously.
    let mut shell_cmd = Command::new("omarchy-shell");
    if std::env::var_os("OMARCHY_PATH").is_none()
        && std::path::Path::new("/usr/share/omarchy/shell/shell.qml").exists()
    {
        shell_cmd.env("OMARCHY_PATH", "/usr/share/omarchy");
    }
    let multi_screen_ok = shell_cmd
        .args(["stasis.rest", "notify", summary, body])
        .output()
        .is_ok_and(|out| {
            out.status.success()
                && String::from_utf8_lossy(&out.stdout)
                    .trim()
                    .starts_with("ok:")
        });
    if multi_screen_ok {
        return;
    }

    // Fallback when the `stasis.rest` bar widget is not mounted: Omarchy's
    // single-display notification helper, then standard freedesktop notify-send.
    let omarchy_ok = Command::new("omarchy-notification-send")
        .args([
            "--app-name",
            "Stasis",
            "-g",
            "󰒲",
            "-u",
            "normal",
            "-t",
            "15000",
            "--transient",
            summary,
            body,
        ])
        .status()
        .is_ok_and(|s| s.success());
    if omarchy_ok {
        return;
    }

    // Standard freedesktop fallback for KDE, GNOME, Sway, and other desktops.
    let _ = Command::new("notify-send")
        .args([
            "--app-name=Stasis",
            "--urgency=normal",
            "--expire-time=15000",
            "--transient",
            "--",
            summary,
            body,
        ])
        .status();
}

#[cfg(target_os = "macos")]
fn dispatch_os_notification(summary: &str, body: &str) {
    // Pass strings via `argv` rather than interpolating into AppleScript source.
    let _ = Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            "--",
            summary,
            body,
        ])
        .status();
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn dispatch_os_notification(_summary: &str, _body: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_one_minute_lead_cleanly() {
        let (summary, body) = format_pre_lock(&PreLockNotice {
            session_id: "s1".into(),
            seconds_until_lock: 60,
            rest_minutes: 10,
        });
        assert_eq!(summary, "即将锁屏休息");
        assert_eq!(body, "1 分钟后将锁定键鼠休息 10 分钟，请提前保存当前工作。");
    }

    #[test]
    fn formats_short_remaining_seconds_when_under_45s() {
        let (_summary, body) = format_pre_lock(&PreLockNotice {
            session_id: "s1".into(),
            seconds_until_lock: 25,
            rest_minutes: 15,
        });
        assert_eq!(body, "25 秒后将锁定键鼠休息 15 分钟，请提前保存当前工作。");
    }

    #[test]
    fn clamps_zero_values_to_at_least_one() {
        let (_summary, body) = format_pre_lock(&PreLockNotice {
            session_id: "s1".into(),
            seconds_until_lock: 0,
            rest_minutes: 0,
        });
        assert_eq!(body, "1 秒后将锁定键鼠休息 1 分钟，请提前保存当前工作。");
    }
}
