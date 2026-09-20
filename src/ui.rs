use std::sync::{Arc, Mutex};

use eframe::egui;

use crate::engine::{Engine, Snapshot, UiCmd};

/// Platform CJK font candidates: (path, face index).
#[cfg(target_os = "linux")]
const CJK_FONTS: &[(&str, u32)] = &[
    ("/usr/share/fonts/droid/DroidSansFallbackFull.ttf", 0),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
    ("/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc", 0),
];

#[cfg(target_os = "windows")]
const CJK_FONTS: &[(&str, u32)] = &[
    ("C:\\Windows\\Fonts\\msyh.ttc", 0),
    ("C:\\Windows\\Fonts\\msyh.ttf", 0),
    ("C:\\Windows\\Fonts\\simhei.ttf", 0),
    ("C:\\Windows\\Fonts\\simsun.ttc", 0),
];

#[cfg(target_os = "macos")]
const CJK_FONTS: &[(&str, u32)] = &[
    ("/System/Library/Fonts/PingFang.ttc", 0),
    ("/System/Library/Fonts/STHeiti Light.ttc", 0),
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
];

/// Install a system CJK font as a fallback and switch to the light theme.
/// Without this, Chinese labels render as tofu boxes.
///
/// The theme is *pinned*: leaving the preference on `System` lets egui re-resolve
/// the desktop theme while we also force light visuals, which repaints the same
/// window with two different palettes.
pub fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Light);
    ctx.set_visuals(egui::Visuals::light());

    let Some((path, index)) = CJK_FONTS
        .iter()
        .find(|(p, _)| std::path::Path::new(p).is_file())
    else {
        tracing::warn!("no CJK font found; labels may show as boxes");
        return;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let mut data = egui::FontData::from_owned(bytes);
    data.index = *index;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("cjk".to_owned(), Arc::new(data));
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
    tracing::info!("loaded CJK font {path}");
}

pub struct App {
    engine: Arc<Engine>,
    snapshot: Arc<Mutex<Snapshot>>,
    old_password: String,
    new_password: String,
    confirm_password: String,
    show_change: bool,
    on_top: bool,
    last_leak: Option<std::time::Instant>,
}

/// Minimum spacing between two re-arm requests.  The first re-arm should
/// restore capture; a burst of requests would only churn hook threads.
const REARM_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(2);

impl App {
    pub fn new(engine: Arc<Engine>) -> Self {
        let snapshot = engine.snapshot();
        Self {
            engine,
            snapshot,
            old_password: String::new(),
            new_password: String::new(),
            confirm_password: String::new(),
            show_change: false,
            on_top: false,
            last_leak: None,
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The engine runs on another thread and the lock swallows every input
        // event, so egui gets nothing to react to: without an explicit repaint
        // request the window keeps showing a stale frame (armed gesture, dots,
        // error messages all invisible). Poll the snapshot at 10 Hz instead.
        ctx.request_repaint_after(std::time::Duration::from_millis(100));

        let snap = self.snapshot.lock().unwrap().clone();

        // A locked backend swallows every key, so a key that reaches egui is
        // proof capture stopped — Windows stops calling a low-level hook while
        // its own process owns the foreground.  Ask the engine to re-arm; the
        // keys that leaked were never captured, so the typed password is
        // incomplete and the engine restarts the buffer.
        if snap.locked
            && ctx.input(|i| {
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
            })
            && self
                .last_leak
                .is_none_or(|at| at.elapsed() >= REARM_COOLDOWN)
        {
            self.last_leak = Some(std::time::Instant::now());
            self.engine.send(UiCmd::Rehook);
        }

        // Only touch the window level when it actually changes: sending a
        // viewport command on every frame forces a repaint loop.
        if snap.locked != self.on_top {
            self.on_top = snap.locked;
            let level = if snap.locked {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }

        if ctx.input(|i| i.viewport().close_requested()) && snap.locked {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }

    /// Opaque window background.  The eframe default is 180/255 alpha, which
    /// lets the desktop (and the previous frame's leftovers) show through.
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.window_fill().to_normalized_gamma_f32()
    }

    // `eframe` hands `ui` a bare `Ui` with no margin and no background; the
    // panel supplies both, otherwise text lands straight on an unpainted window.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| self.body(ui));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.engine.send(UiCmd::Exit);
    }
}

impl App {
    fn body(&mut self, ui: &mut egui::Ui) {
        let snap = self.snapshot.lock().unwrap().clone();

        ui.vertical_centered(|ui| {
            ui.heading("Stasis");
            ui.label("Pause the machine. Rest the human.");
            ui.add_space(16.0);

            if snap.locked {
                ui.colored_label(egui::Color32::from_rgb(255, 59, 48), "已锁定");
            } else {
                ui.colored_label(egui::Color32::from_rgb(52, 199, 89), "未锁定");
            }
            ui.add_space(12.0);

            if !snap.message.is_empty() {
                let color = if snap.ok {
                    egui::Color32::from_rgb(52, 199, 89)
                } else {
                    egui::Color32::from_rgb(255, 59, 48)
                };
                ui.colored_label(color, &snap.message);
                ui.add_space(8.0);
            }

            if snap.locked {
                if snap.unlock_mode {
                    ui.label("输入密码解锁");
                    let dots = "●".repeat(snap.password_len);
                    ui.label(egui::RichText::new(dots).size(24.0).monospace());
                    ui.add_space(8.0);
                } else {
                    ui.label("连按3次 j 解锁");
                }

                if ui.button("强制解锁（UI）").clicked() {
                    self.engine.send(UiCmd::Unlock);
                }
            } else {
                if ui.button("锁定系统").clicked() {
                    self.engine.send(UiCmd::Lock);
                }

                ui.add_space(8.0);
                if ui.button("修改密码").clicked() {
                    self.show_change = !self.show_change;
                }

                if self.show_change {
                    ui.group(|ui| {
                        ui.label("当前密码");
                        ui.add(egui::TextEdit::singleline(&mut self.old_password).password(true));
                        ui.label("新密码");
                        ui.add(egui::TextEdit::singleline(&mut self.new_password).password(true));
                        ui.label("确认新密码");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.confirm_password).password(true),
                        );
                        if ui.button("确认修改").clicked() {
                            if self.new_password != self.confirm_password {
                                tracing::warn!("Password mismatch");
                            } else {
                                self.engine.send(UiCmd::ChangePassword {
                                    old: self.old_password.clone(),
                                    new: self.new_password.clone(),
                                });
                            }
                            self.old_password.clear();
                            self.new_password.clear();
                            self.confirm_password.clear();
                        }
                    });
                }
            }

            ui.add_space(12.0);
            ui.label(format!("事件目录: {}", snap.events_dir.display()));
        });
    }
}
