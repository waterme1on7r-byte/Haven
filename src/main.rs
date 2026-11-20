#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod engine;
mod meta;
mod reader;
mod settings;

use eframe::egui;
use std::sync::Arc;

fn app_icon() -> egui::IconData {
    let (w, h) = (64u32, 64u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    let heights = [14usize, 21, 29, 38, 48, 58, 48, 38, 29, 21, 14];
    for (i, bar_h) in heights.into_iter().enumerate() {
        let x0 = 7 + i * 5;
        let y0 = 57 - bar_h;
        let color = match i {
            0..=4 => [0xD0, 0xD0, 0xD0, 0xFF],
            5 => [0xD0, 0x50, 0x5E, 0xFF],
            _ => [0xF2, 0xF2, 0xF2, 0xFF],
        };
        for y in y0..57 {
            for x in x0..(x0 + 4) {
                let n = (y * w as usize + x) * 4;
                rgba[n..n + 4].copy_from_slice(&color);
            }
        }
    }
    egui::IconData {
        rgba,
        width: w,
        height: h,
    }
}

fn main() -> eframe::Result<()> {
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
