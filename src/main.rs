#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod engine;
mod meta;
mod reader;
mod settings;
mod update;

use eframe::egui;
use std::{net::TcpListener, path::PathBuf, sync::Arc, time::Duration};

fn app_icon() -> egui::IconData {
    // assets/haven.svg rasterised to 128x128 RGBA with transparent corners
    // (regenerate assets/haven-128.rgba whenever the SVG changes).
    egui::IconData {
        rgba: include_bytes!("../assets/haven-128.rgba").to_vec(),
        width: 128,
        height: 128,
    }
}

/// A second launch (e.g. a file double-clicked in Explorer while Haven is open)
/// hands its files to the running instance and exits.
fn forward_to_running(args: &[PathBuf]) {
    use std::io::Write;
    let addr: std::net::SocketAddr = "127.0.0.1:45831".parse().unwrap();
    let Ok(mut s) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else {
        return;
    };
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut msg = String::new();
    for p in args {
        let abs = if p.is_absolute() {
            p.clone()
        } else {
            cwd.join(p)
        };
        msg.push_str(&abs.to_string_lossy());
        msg.push('\n');
    }
    let _ = s.write_all(msg.as_bytes());
}

fn main() -> eframe::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    // The bound loopback port is owned only while Haven is running. A second
    // launch passes its files to that instance and exits, keeping one player
    // instance on Windows and macOS.
    let listener = match TcpListener::bind("127.0.0.1:45831") {
        Ok(listener) => listener,
        Err(_) => {
            forward_to_running(&args);
            return Ok(());
        }
    };
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
        Box::new(move |cc| Ok(Box::new(app::Haven::new(cc, args, settings, listener)))),
    )
}
