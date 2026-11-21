#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod engine;
mod meta;
mod reader;
mod settings;

use eframe::egui;
use std::{net::TcpListener, sync::Arc};

fn app_icon() -> egui::IconData {
    // assets/haven.svg rasterised to 128x128 RGBA with transparent corners
    // (regenerate assets/haven-128.rgba whenever the SVG changes).
    egui::IconData {
        rgba: include_bytes!("../assets/haven-128.rgba").to_vec(),
        width: 128,
        height: 128,
    }
}

fn main() -> eframe::Result<()> {
    // The bound loopback port is owned only while Haven is running. A second
    // launch exits immediately, keeping one player instance on Windows and macOS.
    let _single_instance = match TcpListener::bind("127.0.0.1:45831") {
        Ok(listener) => listener,
        Err(_) => return Ok(()),
    };
    let args: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    let settings = settings::Settings::load();
    let mut vp = egui::ViewportBuilder::default()
        .with_title("Haven")
        .with_inner_size(settings.dims())
        .with_min_inner_size([340.0, 110.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_drag_and_drop(true)
        .with_icon(Arc::new(app_icon()));
    if settings.on_top {
        vp = vp.with_always_on_top();
    }
    if let Some((x, y)) = settings.pos {
        vp = vp.with_position([x, y]);
    }
    let opts = eframe::NativeOptions {
        viewport: vp,
        ..Default::default()
    };
    eframe::run_native(
        "Haven",
        opts,
        Box::new(move |cc| Ok(Box::new(app::Haven::new(cc, args, settings)))),
    )
}
