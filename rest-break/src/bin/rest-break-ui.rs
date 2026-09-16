//! rest-break 常驻状态 UI：置顶、可拖动的小窗，一行摘要 + 悬停/点击展开详情。
//! 只读 status.json（每 1s 重新读取自动刷新），不写判定状态、不抓输入。
//! 仅在 `ui` feature 下编译：
//! `cargo run --manifest-path rest-break/Cargo.toml --features ui --bin rest-break-ui`。
//!
//! 配置（环境变量）：
//! - `STATUS_FILE`：status.json 路径（与 cron 一致；缺省 ./status.json）。
//! - `RB_UI_CORNER`：初始角落 `ne|nw|se|sw`，缺省 `se`（与现役 overlay 相同）。
//! - `RB_UI_MARGIN`：离屏幕边缘的物理像素边距，缺省 `90`。
//! - `RB_UI_FONT_SIZE`：字号（egui points），缺省 `14.0`。
//! - `RB_UI_TEXT_COLOR`：`#rrggbb` 文本颜色，缺省纯白 `#ffffff`。

use eframe::egui;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// 平台 CJK 字体候选：(路径, face index)，与根 crate `src/ui.rs` 同表。
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

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
const CJK_FONTS: &[(&str, u32)] = &[];

/// 挂一个系统 CJK 字体做 fallback，否则中文标签渲染成豆腐块。
fn install_fonts(ctx: &egui::Context) {
    let Some((path, index)) = CJK_FONTS
        .iter()
        .find(|(p, _)| std::path::Path::new(p).is_file())
    else {
        eprintln!("rest-break-ui: 未找到 CJK 字体，中文可能显示为方框");
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
}

#[derive(Clone, Copy, PartialEq)]
enum Corner {
    Ne,
    Nw,
    Se,
    Sw,
}

struct Config {
    status_path: PathBuf,
    corner: Corner,
    margin: f32,
    font_size: f32,
    text_color: egui::Color32,
}

impl Config {
    fn from_env() -> Self {
        let status_path = std::env::var("STATUS_FILE")
            .ok()
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./status.json"));
        let corner = match std::env::var("RB_UI_CORNER").as_deref() {
            Ok("ne") => Corner::Ne,
            Ok("nw") => Corner::Nw,
            Ok("sw") => Corner::Sw,
            _ => Corner::Se,
        };
        let margin = std::env::var("RB_UI_MARGIN")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(90.0);
        let font_size = std::env::var("RB_UI_FONT_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(14.0);
        let text_color = std::env::var("RB_UI_TEXT_COLOR")
            .ok()
            .and_then(|v| parse_color(&v))
            .unwrap_or(egui::Color32::WHITE);
        Config {
            status_path,
            corner,
            margin,
            font_size,
            text_color,
        }
    }
}

fn parse_color(hex: &str) -> Option<egui::Color32> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

/// 依据屏幕尺寸与角落计算窗口左上角坐标（egui points，即物理像素/缩放）。
fn corner_position(
    corner: Corner,
    margin: f32,
    screen: egui::Vec2,
    size: egui::Vec2,
) -> egui::Pos2 {
    let x = match corner {
        Corner::Ne | Corner::Se => screen.x - size.x - margin,
        Corner::Nw | Corner::Sw => margin,
    };
    let y = match corner {
        Corner::Ne | Corner::Nw => margin,
        Corner::Se | Corner::Sw => screen.y - size.y - margin,
    };
    egui::pos2(x.max(0.0), y.max(0.0))
}

/// 粗估文本宽度：CJK 全宽、ASCII 半宽；够用来定窗口宽度即可。
fn text_width(text: &str, font_size: f32) -> f32 {
    text.chars()
        .map(|c| (if c.is_ascii() { 0.55 } else { 1.0 }) * font_size)
        .sum()
}

struct App {
    config: Config,
    status: Result<rest_break::ui::UiStatus, String>,
    hover: rest_break::ui::HoverState,
    positioned: bool,
    above_sent: bool,
    last_size: egui::Vec2,
}

impl App {
    fn new(config: Config) -> Self {
        App {
            status: rest_break::ui::load(&config.status_path),
            hover: rest_break::ui::HoverState::default(),
            positioned: false,
            above_sent: false,
            last_size: egui::vec2(0.0, 0.0),
            config,
        }
    }

    fn summary(&self) -> String {
        match &self.status {
            Ok(s) => rest_break::ui::summary_line(s),
            Err(_) => "休息状态数据不可用".to_string(),
        }
    }

    fn window_size(&self) -> egui::Vec2 {
        let detail_size = self.config.font_size - 2.0;
        let mut width = text_width(&self.summary(), self.config.font_size);
        let mut lines = 0.0;
        if self.hover.expanded() {
            match &self.status {
                Ok(s) => {
                    let items = rest_break::ui::detail_lines(s);
                    lines = items.len() as f32;
                    for (k, v) in items {
                        width = width.max(text_width(&format!("{k}: {v}"), detail_size));
                    }
                }
                Err(e) => {
                    lines = 1.0;
                    width = width.max(text_width(&format!("错误: {e}"), detail_size));
                }
            }
        }
        let height = if self.hover.expanded() {
            46.0 + lines * (self.config.font_size + 8.0)
        } else {
            46.0
        };
        egui::vec2((width + 28.0).max(200.0), height)
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 每 1s 重读 status.json，实现自动刷新。
        ctx.request_repaint_after(Duration::from_secs(1));
        self.status = rest_break::ui::load(&self.config.status_path);

        // 建窗后补发一次置顶：winit 在建窗前设置 _NET_WM_STATE_ABOVE，
        // KWin/XWayland 可能错过，地图后重发才可靠（与根 crate 锁屏 UI 同法）。
        if !self.above_sent {
            self.above_sent = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
        }

        // 悬停展开与点击钉住在 `ui()` 里判定（需要指针与点击响应）。

        // 首帧按配置角落定位（需要显示器尺寸，只能在 logic 里做）。
        if !self.positioned {
            self.positioned = true;
            if let Some(screen) = ctx.input(|i| i.viewport().monitor_size) {
                let pos = corner_position(
                    self.config.corner,
                    self.config.margin,
                    screen,
                    self.window_size(),
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            }
        }

        // 展开态切换时同步窗口高度；只在变化时发命令，避免每帧重绘循环。
        let size = self.window_size();
        if size != self.last_size {
            self.last_size = size;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }

        // 指针已移出且未钉住时，按剩余宽限时间安排一次重绘完成收起。
        let now_ms = ctx.input(|i| (i.time * 1000.0) as u64);
        if let Some(ms) = self.hover.collapse_in_ms(now_ms) {
            ctx.request_repaint_after(Duration::from_millis(ms));
        }

        // 增长后若越出屏幕（底部/右侧），按需移回屏内；保持用户拖动的位置，
        // 不无条件吸附回角落。每帧收敛一次，避免尺寸生效晚一帧导致偏移。
        if let Some(screen) = ctx.input(|i| i.viewport().monitor_size)
            && let Some(rect) = ctx.input(|i| i.viewport().outer_rect)
        {
            let mut pos = rect.min;
            if rect.max.x > screen.x + 1.0 {
                pos.x -= rect.max.x - screen.x;
            }
            if rect.max.y > screen.y + 1.0 {
                pos.y -= rect.max.y - screen.y;
            }
            pos = egui::pos2(pos.x.max(0.0), pos.y.max(0.0));
            if (pos - rect.min).length() > 1.0 {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                ctx.request_repaint();
            }
        }
    }

    /// 透明背景：eframe 默认用不透明视觉底色，会盖住桌面。
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // 交互判定必须用视口局部坐标：`hover_pos` 是视口局部坐标，而
        // `viewport().inner_rect` 是屏幕坐标（Wayland 上恒为 None），
        // 两者混用会把「指针在窗内」永远判成假（leg 1 悬停失效的根因）。
        let hovered = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .is_some_and(|p| i.viewport_rect().contains(p))
        });
        // 背景可拖动，方便移动窗口；同时当点击目标（拖动需要按住并移动，
        // 快速点按只算点击，不会触发 WM 拖动而吞掉松手事件）。
        // 用 interact 而非 allocate_rect：后者会把布局游标推到窗口底部，
        // 让后续内容全被裁剪（窗口只剩底色）。
        let drag = ui.interact(
            ui.max_rect(),
            ui.id().with("bg-drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.dragged() {
            ctx_drag(&ctx);
        }
        // 点击用原始指针事件而非 `Response::clicked()`：文字 Label 会盖在背景层
        // 之上，egui 的命中测试只把「最上层 widget」算作 hovered，指针落在文字上
        // 时背景 widget 拿不到 click。窗口本身就是点击目标，直接取本帧的点击。
        let (now_ms, clicked) =
            ctx.input(|i| ((i.time * 1000.0) as u64, i.pointer.primary_clicked()));
        let was_expanded = self.hover.expanded();
        self.hover.update(hovered, clicked, now_ms);
        if self.hover.expanded() != was_expanded {
            // 窗口尺寸在 `logic()` 里发送，需要下一帧才会生效。
            ctx.request_repaint();
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(egui::Color32::TRANSPARENT))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(self.summary())
                                .color(self.config.text_color)
                                .size(self.config.font_size),
                        )
                        .selectable(false),
                    );

                    if self.hover.expanded() {
                        ui.add_space(6.0);
                        match &self.status {
                            Ok(s) => {
                                for (k, v) in rest_break::ui::detail_lines(s) {
                                    ui.label(
                                        egui::RichText::new(format!("{k}: {v}"))
                                            .color(self.config.text_color)
                                            .size(self.config.font_size - 2.0),
                                    );
                                }
                            }
                            Err(e) => {
                                ui.label(
                                    egui::RichText::new(format!("错误: {e}"))
                                        .color(egui::Color32::LIGHT_RED)
                                        .size(self.config.font_size - 2.0),
                                );
                            }
                        }
                    }
                });
            });
    }
}

fn ctx_drag(ctx: &egui::Context) {
    // Moves the window with the left mouse button until released.
    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
}

fn main() -> eframe::Result<()> {
    let config = Config::from_env();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rest-break-ui")
            .with_app_id("rest-break-ui")
            .with_always_on_top()
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_inner_size([340.0, 46.0])
            .with_active(true),
        ..Default::default()
    };
    eframe::run_native(
        "rest-break-ui",
        options,
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx);
            Ok(Box::new(App::new(config)))
        }),
    )
}
