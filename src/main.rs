#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod engine;
mod meta;
mod reader;
mod settings;

use eframe::egui;
use std::{net::TcpListener, sync::Arc};

fn app_icon() -> egui::IconData {
    let (w, h) = (64u32, 64u32);
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    // Signal: Haven's selected system icon, with a dark rounded tile and a red beat.
    for y in 0..h as usize {
        for x in 0..w as usize {
            let dx = x.min(63 - x);
            let dy = y.min(63 - y);
            if dx >= 8 || dy >= 8 || (dx - 8).pow(2) + (dy - 8).pow(2) <= 64 {
                let n = (y * w as usize + x) * 4;
                rgba[n..n + 4].copy_from_slice(&[0x17, 0x18, 0x1a, 0xff]);
            }
        }
    }
    let heights = [30usize, 42, 24, 52, 35, 24];
    for (i, bar_h) in heights.into_iter().enumerate() {
        let x0 = 10 + i * 9;
        let y0 = 48 - bar_h;
        let color = match i {
            0 => [0xae, 0xb1, 0xb5, 0xff],
            1 => [0xd8, 0xda, 0xdd, 0xff],
            2 => [0xb9, 0xbd, 0xc1, 0xff],
            3 => [0xd3, 0x4d, 0x61, 0xff],
            4 => [0xf2, 0xf3, 0xf4, 0xff],
            _ => [0xc8, 0xcb, 0xcf, 0xff],
        };
        for y in y0..48 {
            for x in x0..(x0 + 6) {
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
