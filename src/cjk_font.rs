//! Shared system-font fallback for both GUIs (also included by rest-break-ui).

use eframe::egui;
use std::sync::Arc;

#[cfg(target_os = "linux")]
const CJK_FONTS: &[(&str, u32)] = &[
    ("/usr/share/fonts/droid/DroidSansFallbackFull.ttf", 0),
    (
        "/usr/share/fonts/google-droid-sans-fonts/DroidSansFallbackFull.ttf",
        0,
    ),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
    (
        "/usr/share/fonts/google-noto-sans-cjk-vf-fonts/NotoSansCJK-VF.ttc",
        0,
    ),
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

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const CJK_FONTS: &[(&str, u32)] = &[];

#[cfg(target_os = "macos")]
fn load() -> Option<(egui::FontData, &'static str)> {
    use std::{fs::File, sync::OnceLock};

    static FONT: OnceLock<Option<(memmap2::Mmap, &'static str, u32)>> = OnceLock::new();
    let (bytes, path, index) = FONT
        .get_or_init(|| {
            CJK_FONTS.iter().find_map(|&(path, index)| {
                let file = File::open(path).ok()?;
                // SAFETY: candidates are read-only OS fonts on macOS's system volume.
                // Never map user-writable fonts: modifying/truncating a mapped file is unsafe.
                let bytes = unsafe { memmap2::Mmap::map(&file) }.ok()?;
                Some((bytes, path, index))
            })
        })
        .as_ref()?;
    // egui 0.36 clones FontData's Cow into its shaping/rasterization blob.
    // Borrow a process-lifetime, file-backed mapping instead of duplicating an owned TTC.
    let mut data = egui::FontData::from_static(bytes);
    data.index = *index;
    Some((data, path))
}

#[cfg(not(target_os = "macos"))]
fn load() -> Option<(egui::FontData, &'static str)> {
    CJK_FONTS.iter().find_map(|&(path, index)| {
        let mut data = egui::FontData::from_owned(std::fs::read(path).ok()?);
        data.index = index;
        Some((data, path))
    })
}

/// Install the fallback without changing the theme; return the selected path for logging.
pub(crate) fn install(ctx: &egui::Context) -> Option<&'static str> {
    let (data, path) = load()?;
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
    Some(path)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn system_font_is_shared_and_renders_chinese() {
        let (data, _) = load().expect("macOS system CJK font");
        let (again, _) = load().unwrap();
        assert!(matches!(data.font, std::borrow::Cow::Borrowed(_)));
        assert_eq!(data.font.as_ptr(), data.clone().font.as_ptr());
        assert_eq!(data.font.as_ptr(), again.font.as_ptr());

        let ctx = egui::Context::default();
        install(&ctx).unwrap();
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.ctx().fonts_mut(|fonts| {
                    assert!(fonts.has_glyphs(
                        &egui::FontId::new(14.0, family.clone()),
                        "已锁定未锁定输入密码解锁休息状态数据不可用工作时间事件目录错误确认修改",
                    ));
                });
            });
            output.textures_delta.clear();
        }
    }
}
