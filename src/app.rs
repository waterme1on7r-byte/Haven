//! UI: a single custom-painted pill matching the Haven design.
use crate::{
    engine::Engine,
    meta::{self, TrackInfo, Waveform, BINS_PER_SEC},
    settings::Settings,
};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Key, Modifiers, Rect, Rounding, Sense, Shape, Stroke,
};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::Ordering::Relaxed, Arc},
    time::{Duration, Instant},
};

const EXTS: &[&str] = &[
    "flac", "m4a", "mp4", "wav", "aif", "aiff", "mp3", "aac", "ogg", "oga",
];

// Design tokens (sampled from the reference image)
const PILL: Color32 = Color32::from_rgb(0x29, 0x2a, 0x2c);
const PILL_EDGE: Color32 = Color32::from_rgb(0x38, 0x39, 0x3b);
const WELL: Color32 = Color32::from_rgb(0x14, 0x15, 0x16);
const WELL_EDGE: Color32 = Color32::from_rgb(0x34, 0x35, 0x37);
const BTN_FACE: Color32 = Color32::from_rgb(0x20, 0x21, 0x23);
const ICON: Color32 = Color32::from_rgb(0xd2, 0xd2, 0xd2);
const RED: Color32 = Color32::from_rgb(0xd0, 0x50, 0x5e);
const BAR_FUTURE: Color32 = Color32::from_rgb(0xe4, 0xe4, 0xe4);
const ACCENT: Color32 = Color32::from_rgb(0xab, 0x43, 0x54); // progress ring
const RING_TRACK: Color32 = Color32::from_rgb(0x2f, 0x30, 0x32);
// Slightly wider time bins keep the moving waveform calm at every window size.
const SECS_PER_BAR: f64 = 0.125;
const MENU_VIEWPORT_HEIGHT: f32 = 520.0;
const PLAYLIST_HEIGHT: f32 = 270.0;
const PLAYLIST_GAP: f32 = 8.0;

pub struct Haven {
    engine: Engine,
    info: Option<TrackInfo>,
    wf: Option<Arc<Waveform>>,
    seen_gen: u64,
    drag: Option<f64>,
    ring_active: bool,
    settings: Settings,
    pos_changed: Option<Instant>,
    playlist_open: bool,
    context_menu_open: bool,
}

fn scan(paths: &[PathBuf]) -> Vec<PathBuf> {
    fn walk(p: &Path, out: &mut Vec<PathBuf>) {
        if p.is_dir() {
            let mut e: Vec<PathBuf> = std::fs::read_dir(p)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .collect();
            e.sort_by_key(|p| p.to_string_lossy().to_lowercase());
            for x in e {
                walk(&x, out);
            }
        } else if p
            .extension()
            .and_then(|e| e.to_str())
            .map_or(false, |e| EXTS.contains(&e.to_lowercase().as_str()))
        {
            out.push(p.to_path_buf());
        }
    }
    let mut out = vec![];
    for p in paths {
        walk(p, &mut out);
    }
    out
}

/// Rounded-rect outline as a polyline, clockwise, starting at the left edge midpoint (9 o'clock).
fn ring_pts(r: Rect, c: f32) -> Vec<egui::Pos2> {
    use std::f32::consts::PI;
    fn arc(v: &mut Vec<egui::Pos2>, cx: f32, cy: f32, rad: f32, a0: f32, a1: f32) {
        for i in 1..=14 {
            let a = a0 + (a1 - a0) * i as f32 / 14.0;
            v.push(pos2(cx + rad * a.cos(), cy + rad * a.sin()));
        }
    }
    let (x0, y0, x1, y1, cy) = (r.left(), r.top(), r.right(), r.bottom(), r.center().y);
    let mut v = vec![pos2(x0, cy), pos2(x0, y0 + c)];
    arc(&mut v, x0 + c, y0 + c, c, PI, 1.5 * PI);
    v.push(pos2(x1 - c, y0));
    arc(&mut v, x1 - c, y0 + c, c, 1.5 * PI, 2.0 * PI);
    v.push(pos2(x1, y1 - c));
    arc(&mut v, x1 - c, y1 - c, c, 0.0, 0.5 * PI);
    v.push(pos2(x0 + c, y1));
    arc(&mut v, x0 + c, y1 - c, c, 0.5 * PI, PI);
    v.push(pos2(x0, cy));
    v
}

/// The first `f` (0..1) of the polyline's length.
fn take_frac(p: &[egui::Pos2], f: f32) -> Vec<egui::Pos2> {
    let total: f32 = p.windows(2).map(|w| w[0].distance(w[1])).sum();
    let target = total * f;
    let (mut acc, mut out) = (0.0f32, vec![p[0]]);
    for w in p.windows(2) {
        let d = w[0].distance(w[1]);
        if acc + d >= target {
            let t = if d > 0.0 { (target - acc) / d } else { 0.0 };
            out.push(w[0].lerp(w[1], t));
            return out;
        }
        acc += d;
        out.push(w[1]);
    }
    out
}

/// (distance to ring, fraction along ring) for the closest point to `pt`.
fn nearest_frac(p: &[egui::Pos2], pt: egui::Pos2) -> (f32, f32) {
    let total: f32 = p.windows(2).map(|w| w[0].distance(w[1])).sum();
    let (mut acc, mut best) = (0.0f32, (f32::MAX, 0.0f32));
    for w in p.windows(2) {
        let ab = w[1] - w[0];
        let len2 = ab.length_sq();
        let t = if len2 > 0.0 {
            ((pt - w[0]).dot(ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let d = (w[0] + ab * t).distance(pt);
        if d < best.0 {
            best = (d, (acc + ab.length() * t) / total);
        }
        acc += ab.length();
    }
    best
}

fn fmt_time(s: f64) -> String {
    let s = s.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

impl Haven {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Vec<PathBuf>, settings: Settings) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let engine = Engine::new();
        let q = scan(&args);
        if !q.is_empty() {
            engine.queue(q);
        }
        Self {
            engine,
            info: None,
            wf: None,
            seen_gen: 0,
            drag: None,
            ring_active: false,
            settings,
            pos_changed: None,
            playlist_open: false,
            context_menu_open: false,
        }
    }

    fn open(&self, paths: Vec<PathBuf>, append: bool) {
        let q = scan(&paths);
        if q.is_empty() {
            return;
        }
        if append {
            self.engine.append(q)
        } else {
            self.engine.queue(q)
        }
    }

    /// Remember where the window sits; written 500 ms after it stops moving.
    fn track_position(&mut self, ctx: &egui::Context) {
        if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
            let p = (r.min.x.round(), r.min.y.round());
            if self.settings.pos != Some(p) {
                self.settings.pos = Some(p);
                self.pos_changed = Some(Instant::now());
            }
        }
        if let Some(t) = self.pos_changed {
            if t.elapsed() >= Duration::from_millis(500) {
                self.settings.save();
                self.pos_changed = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(550));
            }
        }
    }

    fn sync_track(&mut self, ctx: &egui::Context) {
        let g = self.engine.shared.track_gen.load(Relaxed);
        if g != self.seen_gen {
            self.seen_gen = g;
            self.info = self.engine.shared.info.lock().unwrap().clone();
            self.wf = self
                .info
                .as_ref()
                .map(|i| meta::spawn_waveform(i.path.clone())); // old Arc dropped -> old job stops
            if let Some(i) = &self.info {
                // Keep the app name first so Windows' volume mixer groups the session
                // under a recognizable Haven label when it uses the window title.
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("Haven — {}", i.title)));
            }
        }
    }

    fn show_context_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.context_menu_open = true;
        let sh = self.engine.shared.clone();
        if ui.button("Previous").clicked() {
            self.engine.prev();
            ui.close_menu();
        }
        if ui.button("Next").clicked() {
            self.engine.next();
            ui.close_menu();
        }
        ui.separator();
        if ui
            .button(if self.playlist_open {
                "Hide playlist"
            } else {
                "Playlist"
            })
            .clicked()
        {
            self.playlist_open = !self.playlist_open;
            ui.close_menu();
        }
        let mut v = sh.vol();
        if ui
            .add(
                egui::Slider::new(&mut v, 0.0..=1.0)
                    .text("Volume")
                    .show_value(false),
            )
            .changed()
        {
            sh.set_vol(v);
        }
        let mut m = sh.muted.load(Relaxed);
        if ui.checkbox(&mut m, "Mute").changed() {
            sh.muted.store(m, Relaxed);
        }
        let mut s = sh.shuffle.load(Relaxed);
        if ui.checkbox(&mut s, "Shuffle").changed() {
            sh.shuffle.store(s, Relaxed);
        }
        let mut rep = sh.repeat.load(Relaxed);
        let before = rep;
        ui.radio_value(&mut rep, 0u8, "Repeat off");
        ui.radio_value(&mut rep, 1u8, "Repeat all");
        ui.radio_value(&mut rep, 2u8, "Repeat one");
        if rep != before {
            sh.repeat.store(rep, Relaxed);
        }
        ui.separator();
        ui.menu_button("Size", |ui| {
            for (n, label) in [(0u8, "Small"), (1, "Medium"), (2, "Big")] {
                if ui.radio(self.settings.size == n, label).clicked() {
                    self.settings.size = n;
                    self.settings.save();
                    let dims = Settings::dims_for(n);
                    let desired_height = dims[1]
                        + if self.playlist_open {
                            PLAYLIST_HEIGHT + PLAYLIST_GAP
                        } else {
                            0.0
                        };
                    let height = if self.context_menu_open {
                        desired_height.max(MENU_VIEWPORT_HEIGHT)
                    } else {
                        desired_height
                    };
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(dims[0], height)));
                    ui.close_menu();
                }
            }
        });
        let mut top = self.settings.on_top;
        if ui.checkbox(&mut top, "Always on top").changed() {
            self.settings.on_top = top;
            self.settings.save();
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(if top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            }));
        }
        let mut lock = self.settings.locked;
        if ui.checkbox(&mut lock, "Lock position").changed() {
            self.settings.locked = lock;
            self.settings.save();
        }
        ui.separator();
        if ui.button("Open files…").clicked() {
            if let Some(f) = rfd::FileDialog::new()
                .add_filter("Audio", EXTS)
                .pick_files()
            {
                self.open(f, false);
            }
            ui.close_menu();
        }
        if ui.button("Open folder…").clicked() {
            if let Some(f) = rfd::FileDialog::new().pick_folder() {
                self.open(vec![f], false);
            }
            ui.close_menu();
        }
        if ui.button("Minimize").clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            ui.close_menu();
        }
        if ui.button("Exit").clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let sh = self.engine.shared.clone();
        let pos = sh.pos_ms.load(Relaxed) as f64 / 1000.0;
        let dur = sh.dur_ms.load(Relaxed) as f64 / 1000.0;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Space) {
                self.engine.toggle();
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowRight) {
                self.engine.next();
            }
            if i.consume_key(Modifiers::SHIFT, Key::ArrowLeft) {
                self.engine.prev();
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                self.engine.seek((pos + 5.0).min(dur));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowLeft) {
                self.engine.seek((pos - 5.0).max(0.0));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                sh.set_vol(sh.vol() + 0.05);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                sh.set_vol(sh.vol() - 0.05);
            }
            if i.consume_key(Modifiers::NONE, Key::M) {
                sh.muted.store(!sh.muted.load(Relaxed), Relaxed);
            }
        });
        let (dropped, shift) = ctx.input(|i| {
            (
                i.raw
                    .dropped_files
                    .iter()
                    .filter_map(|f| f.path.clone())
                    .collect::<Vec<_>>(),
                i.modifiers.shift,
            )
        });
        if !dropped.is_empty() {
            self.open(dropped, shift);
        } // Shift+drop appends to the queue
    }

    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let sh = self.engine.shared.clone();
        let p = ui.painter().clone();
        let id = ui.id();

        // ---- geometry (everything scales with window height) ----
        let full = ui.max_rect();
        let pill = full.shrink(full.height() * 0.124); // room for the progress ring (scales with size)
        let h = pill.height();
        let d = h * 0.68;
        let r = d / 2.0;
        let bc = pos2(pill.left() + h * 0.20 + r, pill.center().y);
        let brect = Rect::from_center_size(bc, vec2(d, d));
        let well = Rect::from_min_max(
            pos2(bc.x + r + h * 0.125, pill.top() + h * 0.07),
            pos2(pill.right() - h * 0.075, pill.bottom() - h * 0.07),
        );
        let pitch = (well.width() * 0.039).max(8.0);
        let cx = well.center().x;

        // progress ring: hugs the pill, stroke and gap scale with height
        let sw = (h * 0.045).max(4.0);
        let off = h * 0.09;
        let ring_rect = pill.expand(off);
        let ring = ring_pts(ring_rect, h * 0.17 + off);
        // The full player surface is the fallback target for both dragging and
        // opening the menu. Later controls take precedence for their own clicks.
        let full_resp = ui.interact(full, id.with("win"), Sense::click_and_drag());
        if full_resp.drag_started() && !self.settings.locked {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        // added before the pill's own widgets so they win where they overlap
        let ring_resp = ui.interact(
            ring_rect.expand(sw),
            id.with("ring"),
            Sense::click_and_drag(),
        );

        // ---- interaction ----
        if ui
            .interact(pill, id.with("bg"), Sense::drag())
            .drag_started()
            && !self.settings.locked
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        let btn = ui
            .interact(brect, id.with("btn"), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if btn.clicked() {
            self.engine.toggle();
        }
        if btn.drag_started() && !self.settings.locked {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let wresp = ui.interact(well, id.with("wave"), Sense::click_and_drag());
        let dur = sh.dur_ms.load(Relaxed) as f64 / 1000.0;
        let real_pos = sh.pos_ms.load(Relaxed) as f64 / 1000.0;
        if wresp.drag_started() {
            if ctx.input(|i| i.modifiers.alt) && !self.settings.locked {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); // Alt/Option+drag moves the window
            } else {
                self.drag = Some(real_pos);
            }
        }
        if wresp.dragged() {
            if let Some(dp) = self.drag.as_mut() {
                *dp = (*dp - (wresp.drag_delta().x / pitch) as f64 * SECS_PER_BAR).clamp(0.0, dur);
            }
        }
        if wresp.drag_stopped() {
            if let Some(dp) = self.drag.take() {
                self.engine.seek(dp);
            }
        }
        if wresp.clicked() {
            if let Some(pt) = wresp.interact_pointer_pos() {
                self.engine
                    .seek((real_pos + ((pt.x - cx) / pitch) as f64 * SECS_PER_BAR).clamp(0.0, dur));
            }
        }

        let near = ring_resp
            .hover_pos()
            .map_or(false, |pt| nearest_frac(&ring, pt).0 <= sw * 2.0);
        if near {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if ring_resp.drag_started() {
            self.ring_active = ring_resp
                .interact_pointer_pos()
                .map_or(false, |pt| nearest_frac(&ring, pt).0 <= sw * 2.0);
            if !self.ring_active && !self.settings.locked {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); // grabbed the gap, not the ring
            }
        }
        if self.ring_active && ring_resp.dragged() {
            if let Some(pt) = ring_resp.interact_pointer_pos() {
                self.drag = Some(nearest_frac(&ring, pt).1 as f64 * dur);
            }
        }
        if ring_resp.drag_stopped() {
            if self.ring_active {
                if let Some(d) = self.drag.take() {
                    self.engine.seek(d);
                }
            }
            self.ring_active = false;
        }
        if ring_resp.clicked() {
            if let Some(pt) = ring_resp.interact_pointer_pos() {
                let (dist, f) = nearest_frac(&ring, pt);
                if dist <= sw * 2.0 {
                    self.engine.seek(f as f64 * dur);
                }
            }
        }
        let pos = self.drag.unwrap_or(real_pos);

        // ---- pill ----
        let pr = Rounding::same(h * 0.17);
        p.rect_filled(
            pill.translate(vec2(0.0, 4.0)),
            pr,
            Color32::from_black_alpha(70),
        );
        p.rect_filled(pill, pr, PILL);
        p.rect_stroke(pill, pr, Stroke::new(1.0, PILL_EDGE));

        // ---- progress ring (design A: halo) ----
        let frac = if dur > 0.0 {
            (pos / dur).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        p.add(Shape::line(ring.clone(), Stroke::new(sw, RING_TRACK)));
        if frac > 0.002 {
            let done = take_frac(&ring, frac);
            p.add(Shape::line(done.clone(), Stroke::new(sw, ACCENT)));
            for e in [done[0], *done.last().unwrap()] {
                p.circle_filled(e, sw / 2.0, ACCENT);
            } // round caps
        }

        // ---- button ----
        let hot = if btn.hovered() { 1.15 } else { 1.0 };
        p.circle(
            bc,
            r,
            Color32::from_rgb(0x0d, 0x0d, 0x0e),
            Stroke::new(1.0, Color32::from_rgb(0x3c, 0x3d, 0x3f)),
        );
        p.circle(
            bc,
            r * 0.90,
            BTN_FACE.gamma_multiply(hot),
            Stroke::new(r * 0.04, Color32::from_rgb(0x08, 0x08, 0x09)),
        );
        if sh.playing.load(Relaxed) {
            let (bw, bh, gap) = (d * 0.105, d * 0.315, d * 0.11);
            for s in [-1.0f32, 1.0] {
                let c = pos2(bc.x + s * (gap / 2.0 + bw / 2.0), bc.y);
                p.rect_filled(Rect::from_center_size(c, vec2(bw, bh)), bw * 0.3, ICON);
            }
        } else {
            let s = d * 0.17;
            p.add(Shape::convex_polygon(
                vec![
                    pos2(bc.x - s * 0.7, bc.y - s * 1.1),
                    pos2(bc.x - s * 0.7, bc.y + s * 1.1),
                    pos2(bc.x + s * 1.2, bc.y),
                ],
                ICON,
                Stroke::NONE,
            ));
        }

        // ---- waveform well ----
        let wr = Rounding::same(h * 0.14);
        p.rect_filled(well, wr, WELL);
        let wp = p.with_clip_rect(well);

        // bars (centered playhead, waveform scrolls underneath)
        let posbar = pos / SECS_PER_BAR;
        let animation_time = ctx.input(|i| i.time);
        let half = (well.width() / 2.0 / pitch).ceil() as i64 + 2;
        let c0 = posbar.floor() as i64;
        let mid = well.center().y;
        let maxh = well.height() * 0.72;
        let bw = pitch * 0.2;
        {
            let peaks = self.wf.as_ref().map(|w| w.peaks.read().unwrap());
            for i in (c0 - half)..=(c0 + half) {
                let t = i as f64 * SECS_PER_BAR;
                if i < 0 || (dur > 0.0 && t > dur) {
                    continue;
                }
                let x = cx + (i as f64 - posbar) as f32 * pitch;
                let b0 = (t * BINS_PER_SEC) as usize;
                let b1 = (((t + SECS_PER_BAR) * BINS_PER_SEC).ceil() as usize).max(b0 + 1);
                let pk = peaks.as_ref().map_or(0.0, |v| {
                    v.iter().skip(b0).take(b1 - b0).fold(0f32, |a, b| a.max(*b))
                });
                let base_height = (bw * 2.0 + pk.powf(0.6) * maxh).min(maxh);
                // Stable pseudo-random selection keeps the song's waveform silhouette intact.
                // Animate only a few future bars, away from the remaining-time label.
                let hash = (i as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(self.seen_gen.wrapping_mul(0xBF58_476D_1CE4_E5B9));
                let over_remaining_label = x >= well.right() - 68.0;
                let animate = x > cx && !over_remaining_label && hash % 7 == 0;
                let phase = ((hash >> 16) % 6283) as f64 / 1000.0;
                let swell = if animate {
                    // Smooth sinusoidal swell: one slow cycle per 18 seconds.
                    1.0 + 0.22
                        * ((animation_time * std::f64::consts::TAU / 18.0 + phase).sin() * 0.5
                            + 0.5) as f32
                } else {
                    1.0
                };
                let bh = (base_height * swell).min(maxh);
                let col = if x < cx {
                    let fade = ((well.right() - x) / (well.width() * 0.5)).clamp(0.35, 1.0);
                    Color32::from_gray(0x5a).gamma_multiply(fade)
                } else {
                    BAR_FUTURE
                };
                wp.rect_filled(
                    Rect::from_center_size(pos2(x, mid), vec2(bw, bh)),
                    bw / 2.0,
                    col,
                );
            }
        }

        // playhead
        wp.line_segment(
            [pos2(cx, well.top()), pos2(cx, well.bottom())],
            Stroke::new(2.5, RED),
        );
        let elapsed = fmt_time(pos);
        let remaining = format!("−{}", fmt_time((dur - pos).max(0.0)));
        let time_font = FontId::proportional((h * 0.105).clamp(9.0, 14.0));
        wp.text(
            pos2(well.left() + 15.0, well.bottom() - 8.0),
            Align2::LEFT_BOTTOM,
            elapsed,
            time_font.clone(),
            Color32::from_gray(0xb4),
        );
        wp.text(
            pos2(well.right() - 15.0, well.bottom() - 8.0),
            Align2::RIGHT_BOTTOM,
            remaining,
            time_font,
            Color32::from_gray(0xb4),
        );
        p.rect_stroke(well, wr, Stroke::new(1.0, WELL_EDGE));
        let t = well.bottom() + 1.0;
        p.add(Shape::convex_polygon(
            vec![pos2(cx, t - 14.0), pos2(cx - 9.0, t), pos2(cx + 9.0, t)],
            RED,
            Stroke::NONE,
        ));

        full_resp.context_menu(|ui| self.show_context_menu(ui, &ctx));
        btn.context_menu(|ui| self.show_context_menu(ui, &ctx));
        ring_resp.context_menu(|ui| self.show_context_menu(ui, &ctx));
        wresp.context_menu(|ui| self.show_context_menu(ui, &ctx));

        if near && dur > 0.0 {
            ring_resp.on_hover_text_at_pointer(format!("{} / {}", fmt_time(pos), fmt_time(dur)));
        }

        // Track details live in a tooltip so the visual design stays untouched.
        if let Some(i) = &self.info {
            wresp.on_hover_text(i.tooltip());
        }

        // A hover-only top-edge target presents the move hint without intercepting
        // clicks on the progress ring. Window dragging is handled by full_resp.
        let grip = Rect::from_min_max(
            full.min,
            pos2(full.right(), (full.top() + 44.0).min(full.bottom())),
        );
        let grip_resp = ui.interact(grip, id.with("top-grip"), Sense::hover());
        // The drag hint is a small Haven-styled toast at the top of the player,
        // shown only over the dedicated grip instead of following the pointer.
        if grip_resp.hovered() {
            let hint =
                Rect::from_center_size(pos2(full.center().x, full.top() + 18.0), vec2(160.0, 25.0));
            p.rect_filled(
                hint,
                Rounding::same(12.0),
                Color32::from_rgb(0x20, 0x21, 0x23),
            );
            p.rect_stroke(hint, Rounding::same(12.0), Stroke::new(1.0, PILL_EDGE));
            p.text(
                hint.center(),
                Align2::CENTER_CENTER,
                if self.settings.locked {
                    "Position locked"
                } else {
                    "Drag to move Haven"
                },
                FontId::proportional(11.0),
                Color32::from_gray(0xd2),
            );
        }
    }

    fn draw_playlist(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ctx = ui.ctx().clone();
        ui.interact(
            rect,
            ui.id().with("playlist-context"),
            Sense::click_and_drag(),
        )
        .context_menu(|ui| self.show_context_menu(ui, &ctx));
        let painter = ui.painter().clone();
        let sh = self.engine.shared.clone();
        let queue = sh.queue_info.lock().unwrap().clone();
        let idx = sh.current_idx.load(Relaxed) as usize;
        let info = self.info.clone();
        let outer = Rounding::same(22.0);
        painter.rect_filled(rect.shrink(8.0), outer, Color32::from_rgb(0x29, 0x2a, 0x2c));
        painter.rect_stroke(rect.shrink(8.0), outer, Stroke::new(1.0, PILL_EDGE));

        let header = Rect::from_min_max(
            pos2(rect.left() + 27.0, rect.top() + 13.0),
            pos2(rect.right() - 27.0, rect.top() + 43.0),
        );
        painter.text(
            header.left_center(),
            Align2::LEFT_CENTER,
            "Playlist",
            FontId::proportional(16.0),
            Color32::from_gray(0xe6),
        );
        let total_ms: u64 = queue.iter().map(|t| t.duration_ms).sum();
        painter.text(
            header.left_center() + vec2(72.0, 0.0),
            Align2::LEFT_CENTER,
            format!(
                "{} tracks · {}",
                queue.len(),
                fmt_time(total_ms as f64 / 1000.0)
            ),
            FontId::proportional(12.0),
            Color32::from_gray(0x82),
        );
        let close = Rect::from_center_size(
            pos2(header.right() - 8.0, header.center().y),
            vec2(31.0, 31.0),
        );
        let close_resp = ui
            .interact(close, ui.id().with("playlist-close"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Close playlist");
        close_resp.context_menu(|ui| self.show_context_menu(ui, &ctx));
        let close_fill = if close_resp.hovered() {
            Color32::from_rgb(0x43, 0x23, 0x2a)
        } else {
            Color32::from_rgb(0x18, 0x19, 0x1a)
        };
        let close_ink = if close_resp.hovered() {
            RED
        } else {
            Color32::from_gray(0xc8)
        };
        painter.rect_filled(close, Rounding::same(10.0), close_fill);
        painter.rect_stroke(close, Rounding::same(10.0), Stroke::new(1.0, PILL_EDGE));
        let cross = 4.0;
        painter.line_segment(
            [
                close.center() + vec2(-cross, -cross),
                close.center() + vec2(cross, cross),
            ],
            Stroke::new(1.8, close_ink),
        );
        painter.line_segment(
            [
                close.center() + vec2(cross, -cross),
                close.center() + vec2(-cross, cross),
            ],
            Stroke::new(1.8, close_ink),
        );
        if close_resp.clicked() {
            self.playlist_open = false;
            let d = Settings::dims_for(self.settings.size);
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(d[0], d[1])));
        }

        let inner = Rect::from_min_max(
            pos2(rect.left() + 25.0, rect.top() + 48.0),
            pos2(rect.right() - 25.0, rect.bottom() - 19.0),
        );
        painter.rect_filled(
            inner,
            Rounding::same(16.0),
            Color32::from_rgb(0x14, 0x15, 0x16),
        );
        let info_h = 42.0;
        let current = Rect::from_min_max(
            inner.min + vec2(10.0, 5.0),
            pos2(inner.right() - 10.0, inner.top() + info_h),
        );
        if let Some(t) = info {
            painter.text(
                pos2(current.left(), current.center().y - 6.0),
                Align2::LEFT_CENTER,
                &t.title,
                FontId::proportional(13.0),
                Color32::from_gray(0xed),
            );
            painter.text(
                pos2(current.left(), current.center().y + 9.0),
                Align2::LEFT_CENTER,
                if t.artist.is_empty() {
                    t.codec
                } else {
                    t.artist
                },
                FontId::proportional(11.0),
                Color32::from_gray(0x87),
            );
            painter.text(
                pos2(current.right(), current.center().y),
                Align2::RIGHT_CENTER,
                format!(
                    "{} / {}",
                    fmt_time(sh.pos_ms.load(Relaxed) as f64 / 1000.0),
                    fmt_time(t.duration_ms as f64 / 1000.0)
                ),
                FontId::proportional(11.0),
                Color32::from_gray(0x96),
            );
        }
        painter.line_segment(
            [
                pos2(inner.left() + 8.0, inner.top() + info_h),
                pos2(inner.right() - 8.0, inner.top() + info_h),
            ],
            Stroke::new(1.0, Color32::from_gray(0x2e)),
        );
        let list = Rect::from_min_max(
            pos2(inner.left() + 4.0, inner.top() + info_h + 3.0),
            inner.max - vec2(4.0, 4.0),
        );
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(list)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        egui::ScrollArea::vertical()
            .id_source("playlist-scroll")
            .show(&mut child, |ui| {
                for (n, t) in queue.iter().enumerate() {
                    let row =
                        ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                    row.1.context_menu(|ui| self.show_context_menu(ui, &ctx));
                    let active = n == idx;
                    if active {
                        ui.painter().rect_filled(
                            row.0.shrink2(vec2(0.0, 1.0)),
                            Rounding::same(8.0),
                            Color32::from_rgb(0x32, 0x1c, 0x20),
                        );
                    }
                    if active {
                        ui.painter().rect_filled(
                            Rect::from_min_max(
                                row.0.min + vec2(1.0, 7.0),
                                row.0.min + vec2(3.0, 25.0),
                            ),
                            Rounding::same(2.0),
                            RED,
                        );
                    }
                    let fg = if active {
                        Color32::from_gray(0xe8)
                    } else {
                        Color32::from_gray(0xa4)
                    };
                    ui.painter().text(
                        pos2(row.0.left() + 18.0, row.0.center().y),
                        Align2::LEFT_CENTER,
                        if active {
                            "♫".to_string()
                        } else {
                            format!("{}", n + 1)
                        },
                        FontId::proportional(11.0),
                        if active {
                            RED
                        } else {
                            Color32::from_gray(0x70)
                        },
                    );
                    ui.painter().text(
                        pos2(row.0.left() + 52.0, row.0.center().y),
                        Align2::LEFT_CENTER,
                        &t.title,
                        FontId::proportional(12.0),
                        fg,
                    );
                    ui.painter().text(
                        pos2(row.0.left() + row.0.width() * 0.53, row.0.center().y),
                        Align2::LEFT_CENTER,
                        &t.artist,
                        FontId::proportional(11.0),
                        Color32::from_gray(0x78),
                    );
                    ui.painter().text(
                        pos2(row.0.right() - 12.0, row.0.center().y),
                        Align2::RIGHT_CENTER,
                        fmt_time(t.duration_ms as f64 / 1000.0),
                        FontId::proportional(11.0),
                        Color32::from_gray(0x85),
                    );
                    if row.1.clicked() {
                        self.engine.select(n);
                    }
                }
            });
    }
}

impl eframe::App for Haven {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.pos_changed.is_some() {
            self.settings.save();
        }
    }

    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        self.sync_track(ctx);
        self.keys(ctx);
        self.track_position(ctx);
        let menu_was_open = self.context_menu_open;
        self.context_menu_open = false;
        let dims = Settings::dims_for(self.settings.size);
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let opacity = if ctx.input(|i| i.focused) { 1.0 } else { 0.5 };
                ui.set_opacity(opacity);
                // Use fixed viewport-relative rectangles so a resize cannot cause
                // the playlist's layout cursor to restart over the player.
                let area = ui.max_rect();
                let player_rect = Rect::from_min_size(area.min, vec2(dims[0], dims[1]));
                let mut player_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(player_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                self.draw(&mut player_ui);
                if self.playlist_open {
                    let playlist_rect = Rect::from_min_size(
                        pos2(area.left(), area.top() + dims[1] + PLAYLIST_GAP),
                        vec2(dims[0], PLAYLIST_HEIGHT),
                    );
                    let mut playlist_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(playlist_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    self.draw_playlist(&mut playlist_ui);
                }
            });
        if menu_was_open != self.context_menu_open {
            let height = dims[1]
                + if self.playlist_open {
                    PLAYLIST_HEIGHT + PLAYLIST_GAP
                } else {
                    0.0
                };
            let height = if self.context_menu_open {
                height.max(MENU_VIEWPORT_HEIGHT)
            } else {
                height
            };
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(dims[0], height)));
        }
        if self.engine.shared.playing.load(Relaxed) {
            ctx.request_repaint_after(Duration::from_millis(33)); // smoother waveform updates
        } else if self.drag.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}
