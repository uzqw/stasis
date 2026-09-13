#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use tracing::{error, info};

fn main() {
    let data_dir = match stasis::config::Config::config_path() {
        Ok(p) => p.parent().unwrap_or(&p).to_path_buf(),
        Err(_) => std::env::temp_dir().join("stasis"),
    };
    let _ = std::fs::create_dir_all(&data_dir);
    let file_appender = tracing_appender::rolling::daily(&data_dir, "stasis.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Stasis starting");

    let cfg = match stasis::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            error!("Failed to load config: {}", e);
            stasis::config::Config::default()
        }
    };
    if cfg.password.is_empty() {
        info!("No password set; locking is disabled until one is configured");
    }

    let engine = Arc::new(stasis::engine::Engine::new(cfg));
    let snapshot = engine.snapshot();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 620.0])
            .with_resizable(false)
            .with_title("Stasis"),
        ..Default::default()
    };

    let _ = eframe::run_native(
        "Stasis",
        native_options,
        Box::new(|cc| {
            stasis::ui::install_style(&cc.egui_ctx);
            Ok(Box::new(stasis::ui::App::new(engine, snapshot)))
        }),
    );
}
