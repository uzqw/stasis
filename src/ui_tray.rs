//! Tray-icon mode: menu-bar icon + native NSMenu dropdown.
//!
//! The menu is the entire UI surface — no egui window is shown unless the
//! user explicitly asks for password change (`--windowed` still gives the
//! old full window). The rest-break status.json file is polled every minute
//! and rendered as menu items; the tray icon shows fatigue minutes using a
//! real font (template image, so it follows the menu bar appearance).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem},
    Icon, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use crate::engine::{Engine, Snapshot, UiCmd};

const MENU_LOCK_ID: &str = "lock";
const MENU_UNLOCK_ID: &str = "unlock";
const MENU_QUIT_ID: &str = "quit";
const MENU_STATUS_ID: &str = "status";
const MENU_INFO_ID: &str = "info";
const MENU_ERROR_ID: &str = "error";

/// rest-break status.json location, relative to the stasis binary's parent.
fn rest_break_status_path() -> Option<std::path::PathBuf> {
    // Known locations for rest-break's status.json, in priority order.
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let stasis_root = exe.parent().unwrap_or(&exe).to_path_buf();
    let home = std::env::var("HOME").unwrap_or_default();

    let candidates = [
        stasis_root.join("rest-break/target/release/status.json"),
        stasis_root.join("rest-break/rest-break-status.json"),
        std::path::PathBuf::from(&home).join("stasis/rest-break/target/release/status.json"),
        std::path::PathBuf::from(&home).join("stasis/rest-break/rest-break-status.json"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

#[derive(serde::Deserialize)]
struct RestBreakStatus {
    #[serde(default)]
    fatigue_minutes: f64,
    #[serde(default, rename = "fatigueMinutes")]
    fatigue_minutes_snake: f64,
    #[serde(default)]
    work_minutes: f64,
    #[serde(default, rename = "workMinutes")]
    work_minutes_snake: f64,
    #[serde(default)]
    next_rest_minutes: i64,
    #[serde(default, rename = "nextRestMinutes")]
    next_rest_minutes_snake: i64,
    /// Cooldown window: fatigue accumulation paused until this instant.
    #[serde(default, rename = "cooldownUntil")]
    cooldown_until: String,
    /// Circadian coefficient applied to the fatigue model.
    #[serde(default)]
    circadian: f64,
    /// "ok" / "error" — error carries `reason`.
    #[serde(default)]
    state: String,
    #[serde(default)]
    reason: String,
}

impl RestBreakStatus {
    fn load(path: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    fn fatigue_minutes(&self) -> f64 {
        if self.fatigue_minutes_snake > 0.0 {
            self.fatigue_minutes_snake
        } else {
            self.fatigue_minutes
        }
    }
    fn work_minutes(&self) -> f64 {
        if self.work_minutes_snake > 0.0 {
            self.work_minutes_snake
        } else {
            self.work_minutes
        }
    }
    fn next_rest_minutes(&self) -> i64 {
        if self.next_rest_minutes_snake > 0 {
            self.next_rest_minutes_snake
        } else {
            self.next_rest_minutes
        }
    }
    /// Minutes left in the cooldown window, if still active.
    fn cooldown_remaining_minutes(&self) -> Option<i64> {
        if self.cooldown_until.is_empty() {
            return None;
        }
        let until = chrono::DateTime::parse_from_rfc3339(&self.cooldown_until).ok()?;
        let now = chrono::Utc::now();
        let secs = (until.with_timezone(&chrono::Utc) - now).num_seconds();
        (secs > 0).then_some((secs + 59) / 60)
    }
    /// Error line for the menu, without the redundant "error:" prefix.
    fn error_text(&self) -> Option<String> {
        if self.state == "error" && !self.reason.is_empty() {
            let reason = self
                .reason
                .strip_prefix("error:")
                .unwrap_or(&self.reason)
                .trim();
            Some(format!("⚠ {reason}"))
        } else {
            None
        }
    }
}

pub struct TrayApp {
    engine: Arc<Engine>,
    snapshot: Arc<Mutex<Snapshot>>,
    _tray: TrayIcon,
    tray_rx: crossbeam_channel::Receiver<TrayIconEvent>,
    menu_rx: crossbeam_channel::Receiver<MenuEvent>,
    status_item: MenuItem,
    info_item: MenuItem,
    error_item: MenuItem,
    lock_item: MenuItem,
    unlock_item: MenuItem,
    last_menu_refresh: std::time::Instant,
    /// Last fatigue value painted into the icon — skip set_icon when unchanged.
    last_icon_minutes: Option<u32>,
}

// Hack-Regular — the same monospace font egui embeds; digits render like a
// small clock readout. MIT-licensed (https://github.com/source-foundry/Hack).
static FONT_BYTES: &[u8] = include_bytes!("../fonts/Hack-Regular.ttf");

/// Cap height budget for the icon buffer (points ≈ @2x pixels). macOS
/// downscales to ~18 pt in the menu bar; 2-digit and 3-digit texts share the
/// same cap so a 3-digit value shrinks instead of overflowing.
const ICON_HEIGHT: usize = 28;
const ICON_MAX_WIDTH: usize = 56;
const ICON_PAD: usize = 4;

/// Render fatigue minutes with a real font as a template icon: solid black
/// glyphs, anti-aliased alpha, auto-fitted to 2 or 3 digits. Template mode
/// lets macOS recolor it for dark/light menu bars automatically.
fn make_minutes_icon(minutes: u32) -> Option<Icon> {
    let (rgba, w, h) = render_minutes_rgba(minutes)?;
    Icon::from_rgba(rgba, w, h).ok()
}

/// Rasterize the minutes text into an RGBA buffer (black + alpha coverage).
fn render_minutes_rgba(minutes: u32) -> Option<(Vec<u8>, u32, u32)> {
    use ab_glyph::{Font, ScaleFont};

    let minutes = minutes.min(999);
    let text = minutes.to_string();

    let font = ab_glyph::FontVec::try_from_vec(FONT_BYTES.to_vec()).ok()?;

    let width_for = |size: f32| {
        let scaled = font.as_scaled(ab_glyph::PxScale::from(size));
        text.chars()
            .map(|ch| scaled.h_advance(scaled.glyph_id(ch)))
            .sum::<f32>()
    };

    // Largest font size whose full advance width fits the cap.
    let mut size = 22.0f32;
    while size > 8.0 && width_for(size) > (ICON_MAX_WIDTH - ICON_PAD * 2) as f32 {
        size -= 2.0;
    }

    let scaled = font.as_scaled(ab_glyph::PxScale::from(size));
    let ascent = scaled.ascent();
    let descent = scaled.descent(); // negative
    let w = (width_for(size).ceil() as usize + ICON_PAD * 2).min(ICON_MAX_WIDTH);
    let h = ((ascent - descent).ceil() as usize + ICON_PAD * 2).min(ICON_HEIGHT);
    let mut rgba = vec![0u8; w * h * 4];

    let mut x = ICON_PAD as f32;
    let baseline_y = ICON_PAD as f32 + ascent;
    for ch in text.chars() {
        let glyph = scaled.scaled_glyph(ch);
        let glyph = ab_glyph::Glyph {
            position: ab_glyph::point(x, baseline_y),
            ..glyph
        };
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            // draw offsets are relative to px_bounds().min.
            outlined.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    let idx = (py as usize * w + px as usize) * 4;
                    let alpha = (coverage * 255.0) as u8;
                    // Template image: shape comes from alpha; RGB is ignored.
                    rgba[idx + 3] = rgba[idx + 3].saturating_add(alpha);
                }
            });
        }
        x += scaled.h_advance(scaled.glyph_id(ch));
    }

    Some((rgba, w as u32, h as u32))
}

impl TrayApp {
    pub fn new(engine: Arc<Engine>) -> Self {
        let snapshot = engine.snapshot();
        let (tray_tx, tray_rx) = crossbeam_channel::unbounded();
        TrayIconEvent::set_event_handler(Some(move |e| {
            let _ = tray_tx.send(e);
        }));

        let menu = Menu::new();
        let status_item = MenuItem::with_id(MENU_STATUS_ID, "…", false, None);
        let info_item = MenuItem::with_id(MENU_INFO_ID, "", false, None);
        let error_item = MenuItem::with_id(MENU_ERROR_ID, "", false, None);
        let lock_item = MenuItem::with_id(MENU_LOCK_ID, "锁定", true, None);
        let unlock_item = MenuItem::with_id(MENU_UNLOCK_ID, "解锁", true, None);
        let quit_item = MenuItem::with_id(MENU_QUIT_ID, "退出 Stasis", true, None);
        let sep = tray_icon::menu::PredefinedMenuItem::separator();
        let _ = menu.append_items(&[
            &status_item,
            &info_item,
            &error_item,
            &sep,
            &lock_item,
            &unlock_item,
            &quit_item,
        ]);

        let (menu_tx, menu_rx) = crossbeam_channel::unbounded();
        MenuEvent::set_event_handler(Some(move |e| {
            let _ = menu_tx.send(e);
        }));

        let tray = TrayIconBuilder::new()
            .with_menu_on_left_click(true)
            .with_menu(Box::new(menu.clone()))
            .with_tooltip("Stasis")
            .with_icon_as_template(true)
            .with_icon(make_minutes_icon(0).expect("initial icon renders"))
            .build()
            .expect("tray icon");

        Self {
            engine,
            snapshot,
            _tray: tray,
            tray_rx,
            menu_rx,
            status_item,
            info_item,
            error_item,
            lock_item,
            unlock_item,
            last_menu_refresh: std::time::Instant::now()
                .checked_sub(Duration::from_secs(5))
                .unwrap_or_else(std::time::Instant::now),
            last_icon_minutes: None,
        }
    }

    fn refresh_menu(&mut self) {
        let snap = self.snapshot.lock().unwrap().clone();
        let locked = snap.locked;
        let status_text = if locked { "已锁定" } else { "未锁定" };
        let rb_status = rest_break_status_path().and_then(|p| RestBreakStatus::load(&p));

        self.status_item.set_text(format!("Stasis · {status_text}"));
        match rb_status {
            Some(rb) => {
                let fatigue = rb.fatigue_minutes();
                let work = rb.work_minutes();
                let next_rest = rb.next_rest_minutes();
                let mut info = format!(
                    "疲劳 {fatigue:.0} 分钟 · 系数 {:.1} · 工作 {work:.0} 分钟 · 下次休息 {next_rest} 分钟后",
                    rb.circadian
                );
                if let Some(cd) = rb.cooldown_remaining_minutes() {
                    info.push_str(&format!(" · 冷却剩余 {cd} 分钟"));
                }
                self.info_item.set_text(info);

                // Error line: surface rest-break failures instead of hiding them.
                self.error_item
                    .set_text(rb.error_text().unwrap_or_default());

                // Update tray icon only when the displayed value changed —
                // set_icon pokes AppKit layout, so keep it rare.
                let mins = fatigue.round().clamp(0.0, 999.0) as u32;
                if self.last_icon_minutes != Some(mins) {
                    if let Some(icon) = make_minutes_icon(mins) {
                        let _ = self._tray.set_icon(Some(icon));
                        self.last_icon_minutes = Some(mins);
                    }
                }
            }
            None => {
                self.info_item.set_text(String::new());
                self.error_item.set_text(String::new());
            }
        }

        self.lock_item.set_enabled(!locked);
        self.unlock_item.set_enabled(locked);
    }

    fn poll_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.tray_rx.try_recv() {
            tracing::debug!("tray event: {ev:?}");
        }
        while let Ok(ev) = self.menu_rx.try_recv() {
            match ev.id.0.as_str() {
                MENU_LOCK_ID => self.engine.send(UiCmd::Lock),
                MENU_UNLOCK_ID => self.engine.send(UiCmd::Unlock),
                MENU_QUIT_ID => {
                    self.engine.send(UiCmd::Exit);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                _ => {}
            }
        }
    }
}

impl eframe::App for TrayApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(500));
        self.poll_events(ctx);

        // Refresh menu + icon every 60 s.
        if self.last_menu_refresh.elapsed() >= Duration::from_secs(60) {
            self.refresh_menu();
            self.last_menu_refresh = std::time::Instant::now();
        }
    }

    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Never called — viewport is invisible; menu is the only UI.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minutes_icon_renders_2_and_3_digits() {
        for mins in [0u32, 7, 66, 104, 999] {
            let (rgba, w, h) = render_minutes_rgba(mins).expect("renders");
            assert!(w > 0 && h > 0, "{mins}: sane size {w}x{h}");
            let painted = rgba.chunks(4).filter(|px| px[3] > 0).count();
            assert!(painted > 50, "{mins}: expected painted pixels, got {painted}");
        }
    }

    #[test]
    fn cooldown_and_error_parsing() {
        let rb: RestBreakStatus = serde_json::from_str(
            r#"{"fatigueMinutes": 66.5, "workMinutes": 90.0, "nextRestMinutes": 10,
                "cooldownUntil": "2999-01-01T00:00:00+08:00", "state": "error",
                "reason": "error:锁定失败：lock failed", "circadian": 1.4}"#,
        )
        .unwrap();
        assert!(rb.cooldown_remaining_minutes().is_some());
        assert_eq!(rb.error_text().unwrap(), "⚠ 锁定失败：lock failed");
        assert!((rb.circadian - 1.4).abs() < f64::EPSILON);
        let rb2: RestBreakStatus = serde_json::from_str(
            r#"{"fatigueMinutes": 1.0, "cooldownUntil": "2020-01-01T00:00:00+08:00"}"#,
        )
        .unwrap();
        assert!(rb2.cooldown_remaining_minutes().is_none());
        assert!(rb2.error_text().is_none());
    }
}

/// Run the tray-mode app. `windowed` falls back to the original 400×620 UI.
pub fn run(engine: Arc<Engine>) {
    // Viewport stays invisible — tray icon + NSMenu is the only UI.
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1.0, 1.0])
            .with_visible(false)
            .with_decorations(false)
            .with_title("Stasis"),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Stasis",
        native_options,
        Box::new(|cc| {
            crate::ui::install_style(&cc.egui_ctx);
            Ok(Box::new(TrayApp::new(engine)))
        }),
    );
}
