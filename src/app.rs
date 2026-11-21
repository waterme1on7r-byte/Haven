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
const MENU_HEIGHT: f32 = 400.0; // room reserved for the context menu
const MENU_WIDTH: f32 = 190.0;
const PLAYLIST_HEIGHT: f32 = 270.0;
const PLAYLIST_GAP: f32 = 5.0;
const HOVER_CARD_HEIGHT: f32 = 55.0;
const CARD_SLOT: f32 = HOVER_CARD_HEIGHT + 4.0; // window room the card needs
const CARD_HIDE_AFTER: Duration = Duration::from_secs(3);

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
    // Context menu: anchored to the click, opens downward or upward.
    menu_rel: Option<egui::Vec2>, // click position relative to the player's top-left
    menu_up: bool,
    menu_right: bool,
    menu_rect: Option<Rect>,
    menu_close: bool,
    menu_unfocused_since: Option<Instant>,
    // Hover metadata card shown outside the player.
    card_visible: bool,
    card_above: bool,
    card_suppressed: bool,
    card_last: Option<egui::Vec2>,
    card_hide_at: Option<Instant>,
    // Window layout: extra room above the player (card / upward menu).
    top_pad: f32,
    req: [f32; 3], // last requested [top_pad, width, height]
    layout_until: Option<Instant>,
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

/// Truncates `text` with an ellipsis so it is at most `max_w` points wide.
fn fit(ctx: &egui::Context, text: &str, font: FontId, max_w: f32) -> String {
    let width = |s: &str| {
        ctx.fonts(|f| {
            f.layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    if max_w <= 0.0 {
        return String::new();
    }
    if width(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    while lo < hi {
        let m = (lo + hi + 1) / 2;
        let cand = chars[..m].iter().collect::<String>() + "…";
        if width(&cand) <= max_w {
            lo = m;
        } else {
            hi = m - 1;
        }
    }
    chars[..lo]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
        + "…"
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
        let d = settings.dims();
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
            menu_rel: None,
            menu_up: false,
            menu_right: false,
            menu_rect: None,
            menu_close: false,
            menu_unfocused_since: None,
            card_visible: false,
            card_above: true,
            card_suppressed: false,
            card_last: None,
            card_hide_at: None,
            top_pad: 0.0,
            req: [0.0, d[0], d[1]],
            layout_until: None,
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
        // Skip while the window is being resized for the card / menu: the outer
        // rect is not trustworthy then. Store the player's top, not the window's.
        if let Some(r) = ctx
            .input(|i| i.viewport().outer_rect)
            .filter(|_| !self.layout_busy())
        {
            let p = (r.min.x.round(), (r.min.y + self.top_pad).round());
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

    fn layout_busy(&self) -> bool {
        self.layout_until.map_or(false, |t| Instant::now() < t)
    }

    fn close_menu(&mut self) {
        self.menu_rel = None;
        self.menu_rect = None;
        self.menu_close = false;
        self.menu_unfocused_since = None;
    }

    /// Opens the menu at `pt`. It opens downward unless the screen has too little
    /// room below the click, in which case it opens upward (the app never moves).
    fn open_menu(&mut self, ctx: &egui::Context, pt: egui::Pos2, origin: egui::Pos2, width: f32) {
        let (outer, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.monitor_size)
        });
        self.menu_up = match (outer, monitor) {
            (Some(o), Some(m)) => {
                let screen_y = o.min.y + pt.y;
                let below = m.y - screen_y - 12.0;
                let above = screen_y - 12.0;
                below < MENU_HEIGHT && above > below
            }
            _ => false,
        };
        self.menu_right = (pt.x - origin.x) + MENU_WIDTH > width;
        self.menu_rel = Some(pt - origin);
        self.menu_rect = None;
        self.menu_close = false;
    }

    fn show_context_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let sh = self.engine.shared.clone();
        if ui.button("Previous").clicked() {
            self.engine.prev();
            self.menu_close = true;
        }
        if ui.button("Next").clicked() {
            self.engine.next();
            self.menu_close = true;
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
            self.menu_close = true;
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
        // The window resizes itself from `settings.size` (see sync_layout).
        ui.horizontal(|ui| {
            ui.label("Size");
            for (n, label) in [(0u8, "S"), (1, "M"), (2, "B")] {
                if ui
                    .selectable_label(self.settings.size == n, label)
                    .clicked()
                {
                    self.settings.size = n;
                    self.settings.save();
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Wave");
            for n in 0..5u8 {
                if ui
                    .selectable_label(self.settings.wave == n, format!("{}", n + 1))
                    .clicked()
                {
                    self.settings.wave = n;
                    self.settings.save();
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
                self.open(f, true);
            }
            self.menu_close = true;
        }
        if ui.button("Open folder…").clicked() {
            if let Some(f) = rfd::FileDialog::new().pick_folder() {
                self.open(vec![f], true);
            }
            self.menu_close = true;
        }
        if ui.button("Minimize").clicked() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            self.menu_close = true;
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
        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect::<Vec<_>>()
        });
        if !dropped.is_empty() {
            self.open(dropped, true); // new media is added; what is playing keeps playing
        }
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
        // A native window move must begin on the same mouse-down event. Starting
        // it after egui's drag threshold is crossed is intermittent on Windows.
        let begin_window_drag = ctx.input(|i| {
            if i.pointer.button_pressed(egui::PointerButton::Primary) {
                i.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(pt) = begin_window_drag {
            let on_progress_ring = nearest_frac(&ring, pt).0 <= sw * 2.0;
            if full.contains(pt)
                && !well.contains(pt)
                && !brect.contains(pt)
                && !on_progress_ring
                && !self.menu_rect.map_or(false, |r| r.contains(pt))
                && !self.settings.locked
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }
        // added before the pill's own widgets so they win where they overlap
        let ring_resp = ui.interact(
            ring_rect.expand(sw),
            id.with("ring"),
            Sense::click_and_drag(),
        );

        // ---- interaction ----
        let btn = ui
            .interact(brect, id.with("btn"), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if btn.clicked() {
            self.engine.toggle();
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

        // ---- waveform: five styles (centered playhead, audio scrolls underneath) ----
        let posbar = pos / SECS_PER_BAR;
        let animation_time = ctx.input(|i| i.time);
        let half = (well.width() / 2.0 / pitch).ceil() as i64 + 2;
        let c0 = posbar.floor() as i64;
        let mid = well.center().y;
        let maxh = well.height() * 0.72;
        let style = self.settings.wave;
        {
            let peaks_g = self.wf.as_ref().map(|w| w.peaks.read().unwrap());
            let rms_g = self.wf.as_ref().map(|w| w.rms.read().unwrap());
            let pv: Option<&Vec<f32>> = peaks_g.as_deref();
            let rv: Option<&Vec<f32>> = rms_g.as_deref();
            let ref_p = pv
                .map_or(1.0, |v| v.iter().cloned().fold(0.0f32, f32::max))
                .max(0.05);
            let ref_r = rv
                .map_or(1.0, |v| v.iter().cloned().fold(0.0f32, f32::max))
                .max(0.02);
            // (core, halo) 0..1 for a time window: core follows RMS loudness,
            // halo adds the true peaks. Normalised per track so dynamics show.
            let level = |t0: f64, t1: f64| -> (f32, f32) {
                let b0 = (t0.max(0.0) * BINS_PER_SEC) as usize;
                let b1 = ((t1.max(0.0) * BINS_PER_SEC).ceil() as usize).max(b0 + 1);
                let (Some(p), Some(r)) = (pv, rv) else {
                    return (0.0, 0.0);
                };
                let end = b1.min(p.len()).min(r.len());
                if b0 >= end {
                    return (0.0, 0.0);
                }
                let pk = p[b0..end].iter().cloned().fold(0.0f32, f32::max) / ref_p;
                let rm = r[b0..end].iter().sum::<f32>() / (end - b0) as f32 / ref_r;
                let core = rm.clamp(0.0, 1.0).powf(1.15);
                let halo = (core * 0.6 + pk.clamp(0.0, 1.0).powf(1.6) * 0.4).max(core);
                (core, halo)
            };

            if style == 4 {
                // Scope: a continuous line with a carrier, like a zoomed-in waveform.
                let mut pts: Vec<egui::Pos2> = Vec::new();
                let mut split = 0usize;
                let mut x = well.left();
                while x <= well.right() + 2.0 {
                    let tb = posbar + ((x - cx) / pitch) as f64;
                    let t = tb * SECS_PER_BAR;
                    let mut y = mid;
                    if t >= 0.0 && (dur <= 0.0 || t <= dur) {
                        let (core, halo) = level(t - SECS_PER_BAR * 0.5, t + SECS_PER_BAR * 0.5);
                        let amp = (core * 0.7 + halo * 0.3).min(1.0);
                        let ph = tb as f32 * pitch * 1.15;
                        let k = (tb * (pitch as f64 / 2.0)) as i64 as u64;
                        let noise = ((k.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 40) as f32
                            / 16_777_216.0
                            - 0.5)
                            * 0.7;
                        let w = 0.55 * ph.sin() + 0.25 * (ph * 2.31 + 1.1).sin() + noise;
                        y = mid - w.clamp(-1.0, 1.0) * amp * maxh * 0.5;
                    }
                    if x < cx {
                        split = pts.len() + 1;
                    }
                    pts.push(pos2(x, y));
                    x += 2.0;
                }
                let split = split.min(pts.len());
                let (past, fut) = pts.split_at(split.max(1).min(pts.len()));
                let mut past = past.to_vec();
                let mut fut = fut.to_vec();
                if let (Some(l), Some(f)) = (past.last().cloned(), fut.first().cloned()) {
                    past.push(f);
                    fut.insert(0, l);
                }
                if past.len() > 1 {
                    wp.add(Shape::line(
                        past.clone(),
                        Stroke::new(3.4, Color32::from_gray(0x5a).gamma_multiply(0.16)),
                    ));
                    wp.add(Shape::line(
                        past,
                        Stroke::new(1.5, Color32::from_gray(0x78)),
                    ));
                }
                if fut.len() > 1 {
                    wp.add(Shape::line(
                        fut.clone(),
                        Stroke::new(3.4, BAR_FUTURE.gamma_multiply(0.16)),
                    ));
                    wp.add(Shape::line(fut, Stroke::new(1.5, BAR_FUTURE)));
                }
            } else {
                for i in (c0 - half)..=(c0 + half) {
                    let t = i as f64 * SECS_PER_BAR;
                    if i < 0 || (dur > 0.0 && t > dur) {
                        continue;
                    }
                    let x = cx + (i as f64 - posbar) as f32 * pitch;
                    // Stable pseudo-random choice: a few future bars gently swell.
                    let hash = (i as u64)
                        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        .wrapping_add(self.seen_gen.wrapping_mul(0xBF58_476D_1CE4_E5B9));
                    let over_remaining_label = x >= well.right() - 68.0;
                    let animate = x > cx && !over_remaining_label && hash % 7 == 0;
                    let phase = ((hash >> 16) % 6283) as f64 / 1000.0;
                    let swell = if animate {
                        1.0 + 0.22
                            * ((animation_time * std::f64::consts::TAU / 18.0 + phase).sin() * 0.5
                                + 0.5) as f32
                    } else {
                        1.0
                    };
                    let played = x < cx;
                    let fade = ((well.right() - x) / (well.width() * 0.5)).clamp(0.35, 1.0);
                    let ink = |future: Color32, past: u8| -> Color32 {
                        if played {
                            Color32::from_gray(past).gamma_multiply(fade)
                        } else {
                            future
                        }
                    };
                    let (core, halo) = level(t, t + SECS_PER_BAR);
                    let core = (core * swell).min(1.0);
                    let halo = (halo * swell).min(1.0);
                    match style {
                        0 => {
                            // Pulse: peak halo around a bright RMS core.
                            let bw = pitch * 0.34;
                            let hh = (bw + halo * maxh).min(maxh);
                            let ch = (bw * 0.8 + core * maxh).min(hh);
                            wp.rect_filled(
                                Rect::from_center_size(pos2(x, mid), vec2(bw, hh)),
                                bw / 2.0,
                                ink(BAR_FUTURE.gamma_multiply(0.38), 0x3a),
                            );
                            wp.rect_filled(
                                Rect::from_center_size(pos2(x, mid), vec2(bw, ch)),
                                bw / 2.0,
                                ink(BAR_FUTURE, 0x62),
                            );
                        }
                        1 => {
                            // Needles: hairlines at twice the density, with tip dots.
                            let bw = (pitch * 0.11).max(1.4);
                            for sidx in 0..2 {
                                let ts = t + SECS_PER_BAR * 0.5 * sidx as f64;
                                let (c, hl) = level(ts, ts + SECS_PER_BAR * 0.5);
                                let hh = (bw * 2.0 + (hl * swell).min(1.0) * maxh).min(maxh);
                                let xs = x + (sidx as f32 - 0.5) * pitch * 0.5;
                                let col = ink(BAR_FUTURE.gamma_multiply(0.55 + 0.45 * c), 0x62);
                                wp.rect_filled(
                                    Rect::from_center_size(pos2(xs, mid), vec2(bw, hh)),
                                    bw / 2.0,
                                    col,
                                );
                                for sg in [-1.0f32, 1.0] {
                                    wp.circle_filled(pos2(xs, mid + sg * hh / 2.0), bw * 0.95, col);
                                }
                            }
                        }
                        2 => {
                            // Skyline: bars grow from a baseline, faint reflection below.
                            let bw = pitch * 0.30;
                            let base = well.top() + well.height() * 0.62;
                            let up = (bw + halo * maxh * 0.66).min(well.height() * 0.56);
                            let down = (up * 0.38).min(well.height() * 0.26);
                            let col = ink(BAR_FUTURE, 0x62);
                            wp.rect_filled(
                                Rect::from_min_max(
                                    pos2(x - bw / 2.0, base - up),
                                    pos2(x + bw / 2.0, base),
                                ),
                                Rounding {
                                    nw: bw / 2.0,
                                    ne: bw / 2.0,
                                    sw: 0.0,
                                    se: 0.0,
                                },
                                col,
                            );
                            wp.rect_filled(
                                Rect::from_min_max(
                                    pos2(x - bw / 2.0, base + 2.0),
                                    pos2(x + bw / 2.0, base + 2.0 + down),
                                ),
                                Rounding {
                                    nw: 0.0,
                                    ne: 0.0,
                                    sw: bw / 2.0,
                                    se: bw / 2.0,
                                },
                                col.gamma_multiply(0.28),
                            );
                        }
                        _ => {
                            // Segments: stacked LED-style blocks, mirrored around the middle.
                            let seg = (maxh / 16.0).max(3.0);
                            let gap = (seg * 0.38).max(1.5);
                            let sw = pitch * 0.46;
                            let n = ((halo * maxh) / (seg + gap)).round() as i32;
                            let rows = ((n.max(1) as f32) / 2.0).ceil() as i32;
                            for r in 0..rows {
                                let dy = (r as f32 + 0.5) * (seg + gap);
                                let lit = dy * 2.0 <= core * maxh + seg;
                                let k = 1.0 - 0.45 * (r as f32 / rows.max(1) as f32);
                                let a = if lit { 1.0 } else { 0.45 };
                                let col = ink(BAR_FUTURE.gamma_multiply(k * a), 0x62)
                                    .gamma_multiply(if played { a } else { 1.0 });
                                for sg in [-1.0f32, 1.0] {
                                    wp.rect_filled(
                                        Rect::from_center_size(
                                            pos2(x, mid + sg * dy),
                                            vec2(sw, seg),
                                        ),
                                        sw * 0.35,
                                        col,
                                    );
                                }
                            }
                        }
                    }
                }
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
    }

    fn draw_playlist(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ctx = ui.ctx().clone();
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
        let clear = Rect::from_center_size(
            pos2(header.right() - 51.0, header.center().y),
            vec2(45.0, 31.0),
        );
        let clear_resp = ui
            .interact(clear, ui.id().with("playlist-clear"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let clear_fill = if clear_resp.hovered() {
            Color32::from_rgb(0x43, 0x23, 0x2a)
        } else {
            Color32::from_rgb(0x18, 0x19, 0x1a)
        };
        painter.rect_filled(clear, Rounding::same(10.0), clear_fill);
        painter.rect_stroke(clear, Rounding::same(10.0), Stroke::new(1.0, PILL_EDGE));
        painter.text(
            clear.center(),
            Align2::CENTER_CENTER,
            "Clear",
            FontId::proportional(10.0),
            if clear_resp.hovered() {
                RED
            } else {
                Color32::from_gray(0xc8)
            },
        );
        if clear_resp.clicked() {
            self.engine.clear();
        }
        let close = Rect::from_center_size(
            pos2(header.right() - 8.0, header.center().y),
            vec2(31.0, 31.0),
        );
        let close_resp = ui
            .interact(close, ui.id().with("playlist-close"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
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
            self.playlist_open = false; // the window shrinks itself (sync_layout)
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
            // Leave room for the "0:00 / 0:00" readout on the right.
            let text_w = current.width() - 84.0;
            painter.text(
                pos2(current.left(), current.center().y - 6.0),
                Align2::LEFT_CENTER,
                fit(&ctx, &t.title, FontId::proportional(13.0), text_w),
                FontId::proportional(13.0),
                Color32::from_gray(0xed),
            );
            let sub_line = if t.artist.is_empty() {
                t.codec.clone()
            } else {
                t.artist.clone()
            };
            painter.text(
                pos2(current.left(), current.center().y + 9.0),
                Align2::LEFT_CENTER,
                fit(&ctx, &sub_line, FontId::proportional(11.0), text_w),
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
        child.set_clip_rect(list);
        egui::ScrollArea::vertical()
            .id_source("playlist-scroll")
            .show(&mut child, |ui| {
                for (n, t) in queue.iter().enumerate() {
                    let row =
                        ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                    if !ui.is_rect_visible(row.0) {
                        continue;
                    }
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
                    let title_clip = Rect::from_min_max(
                        pos2(row.0.left() + 50.0, row.0.top()),
                        pos2(row.0.left() + row.0.width() * 0.51, row.0.bottom()),
                    );
                    ui.painter().with_clip_rect(title_clip).text(
                        pos2(row.0.left() + 52.0, row.0.center().y),
                        Align2::LEFT_CENTER,
                        fit(
                            &ctx,
                            &t.title,
                            FontId::proportional(12.0),
                            row.0.width() * 0.51 - 56.0,
                        ),
                        FontId::proportional(12.0),
                        fg,
                    );
                    let artist_clip = Rect::from_min_max(
                        pos2(row.0.left() + row.0.width() * 0.53, row.0.top()),
                        pos2(row.0.right() - 56.0, row.0.bottom()),
                    );
                    ui.painter().with_clip_rect(artist_clip).text(
                        pos2(row.0.left() + row.0.width() * 0.53, row.0.center().y),
                        Align2::LEFT_CENTER,
                        fit(
                            &ctx,
                            &t.artist,
                            FontId::proportional(11.0),
                            row.0.width() * 0.47 - 62.0,
                        ),
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

impl Haven {
    fn card_rect(&self, player: Rect, dims: [f32; 2], base_h: f32) -> Rect {
        let inset = dims[1] * 0.124; // same side inset as the pill
        let (l, r) = (player.left() + inset, player.right() - inset);
        if self.card_above {
            Rect::from_min_max(
                pos2(l, player.top() - HOVER_CARD_HEIGHT - 2.0),
                pos2(r, player.top() - 2.0),
            )
        } else {
            let y = player.top() + base_h + 2.0;
            Rect::from_min_max(pos2(l, y), pos2(r, y + HOVER_CARD_HEIGHT))
        }
    }

    fn paint_card(ui: &egui::Ui, rect: Rect, info: &TrackInfo) {
        let ctx = ui.ctx().clone();
        let p = ui.painter();
        let rr = Rounding::same(13.0);
        p.rect_filled(
            rect.translate(vec2(0.0, 2.0)),
            rr,
            Color32::from_black_alpha(60),
        );
        p.rect_filled(rect, rr, Color32::from_rgb(0x20, 0x21, 0x23));
        p.rect_stroke(rect, rr, Stroke::new(1.0, PILL_EDGE));
        let text_w = rect.width() - 14.0 - 34.0;
        p.text(
            rect.left_top() + vec2(14.0, 12.0),
            Align2::LEFT_TOP,
            fit(&ctx, &info.title, FontId::proportional(12.0), text_w),
            FontId::proportional(12.0),
            Color32::from_gray(0xe7),
        );
        let secondary = if info.artist.is_empty() {
            info.codec.clone()
        } else {
            format!("{}  ·  {}", info.artist, info.codec)
        };
        p.text(
            rect.left_bottom() + vec2(14.0, -12.0),
            Align2::LEFT_BOTTOM,
            fit(&ctx, &secondary, FontId::proportional(10.0), text_w),
            FontId::proportional(10.0),
            Color32::from_gray(0x98),
        );
        p.circle_filled(rect.right_center() - vec2(14.0, 0.0), 4.0, RED);
    }

    /// Hover card state machine. It appears when the pointer is anywhere over the
    /// player; once the mouse moves after it appeared it goes away after 3 s, and
    /// stays away until the pointer has left the player.
    fn update_card(&mut self, ctx: &egui::Context, player: Rect, card: Rect) {
        if self.layout_busy() {
            return; // window is resizing: pointer coordinates are unreliable
        }
        let ptr = ctx.input(|i| i.pointer.hover_pos());
        let pressed = ctx.input(|i| i.pointer.any_pressed());
        let inside = ptr.map_or(false, |p| {
            player.contains(p) || (self.card_visible && card.contains(p))
        });
        if self.menu_rel.is_some() || pressed {
            if inside {
                self.card_suppressed = true;
            }
            self.card_visible = false;
        }
        if !inside {
            self.card_suppressed = false;
            self.card_visible = false;
        }
        let Some(p) = ptr.filter(|_| inside) else {
            self.card_hide_at = None;
            self.card_last = None;
            return;
        };
        let rel = p - player.min;
        if self.card_visible {
            if let Some(last) = self.card_last {
                if (rel - last).length() > 1.5 && self.card_hide_at.is_none() {
                    self.card_hide_at = Some(Instant::now() + CARD_HIDE_AFTER);
                }
            }
            self.card_last = Some(rel);
            if let Some(t) = self.card_hide_at {
                let now = Instant::now();
                if now >= t {
                    self.card_visible = false;
                    self.card_suppressed = true;
                    self.card_hide_at = None;
                } else {
                    ctx.request_repaint_after(t - now);
                }
            }
        } else if !self.card_suppressed && self.menu_rel.is_none() && self.info.is_some() {
            let outer = ctx.input(|i| i.viewport().outer_rect);
            // Prefer above the app; go below if the screen has no room above.
            self.card_above = outer.map_or(true, |o| o.min.y + self.top_pad >= CARD_SLOT + 8.0);
            self.card_visible = true;
            self.card_last = Some(rel);
            self.card_hide_at = None;
        }
    }

    /// Sizes the OS window for what is shown (card, upward/downward menu,
    /// playlist). Growing upward also moves the window up by the same amount and
    /// the player is drawn lower by it, so the player never moves on screen.
    fn sync_layout(&mut self, ctx: &egui::Context, dims: [f32; 2], base_h: f32) {
        let (mut top, mut bottom) = (0.0f32, 0.0f32);
        if self.card_visible {
            if self.card_above {
                top = CARD_SLOT;
            } else {
                bottom = CARD_SLOT;
            }
        }
        if let Some(rel) = self.menu_rel {
            if self.menu_up {
                top = top.max((MENU_HEIGHT - rel.y).max(0.0));
            } else {
                bottom = bottom.max((rel.y + MENU_HEIGHT - base_h).max(0.0));
            }
        }
        let req = [top, dims[0], top + base_h + bottom];
        let changed = req
            .iter()
            .zip(self.req.iter())
            .any(|(a, b)| (a - b).abs() > 0.5);
        if !changed {
            return;
        }
        // Wait while busy or while the user is still moving the window.
        if self.layout_busy() || self.pos_changed.is_some() {
            ctx.request_repaint_after(Duration::from_millis(120));
            return;
        }
        let outer = ctx.input(|i| i.viewport().outer_rect);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(req[1], req[2])));
        let delta = req[0] - self.req[0];
        if delta.abs() > 0.5 {
            if let Some(o) = outer {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(
                    o.min.x,
                    o.min.y - delta,
                )));
            }
        }
        self.req = req;
        self.top_pad = req[0];
        self.layout_until = Some(Instant::now() + Duration::from_millis(250));
        ctx.request_repaint_after(Duration::from_millis(260));
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
        if self.layout_until.map_or(false, |t| Instant::now() >= t) {
            self.layout_until = None;
        }
        let dims = Settings::dims_for(self.settings.size);
        let base_h = dims[1]
            + if self.playlist_open {
                PLAYLIST_HEIGHT + PLAYLIST_GAP
            } else {
                0.0
            };
        let area = ctx.screen_rect();
        let player_rect = Rect::from_min_size(
            pos2(area.left(), area.top() + self.top_pad),
            vec2(dims[0], dims[1]),
        );
        let playlist_rect = Rect::from_min_size(
            pos2(area.left(), player_rect.bottom() + PLAYLIST_GAP),
            vec2(dims[0], PLAYLIST_HEIGHT),
        );

        // Right-press opens the context menu where the pointer is.
        let mut opened_now = false;
        let secondary = ctx.input(|i| {
            if i.pointer.button_pressed(egui::PointerButton::Secondary) {
                i.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(pt) = secondary {
            if player_rect.contains(pt) || (self.playlist_open && playlist_rect.contains(pt)) {
                self.open_menu(ctx, pt, player_rect.min, dims[0]);
                opened_now = true;
            }
        }
        // A menu left open in an unfocused window closes after 3 s.
        if self.menu_rel.is_some() && !ctx.input(|i| i.focused) {
            let since = self.menu_unfocused_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_secs(3) {
                self.close_menu();
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        } else {
            self.menu_unfocused_since = None;
        }

        let card_rect = self.card_rect(player_rect, dims, base_h);
        self.update_card(ctx, player_rect, card_rect);
        let show_card = self.card_visible && self.menu_rel.is_none();
        let card_info = if show_card { self.info.clone() } else { None };

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let opacity = if ctx.input(|i| i.focused) { 1.0 } else { 0.5 };
                ui.set_opacity(opacity);
                // Fixed rectangles so a resize cannot restart the layout cursor.
                let mut player_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(player_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                self.draw(&mut player_ui);
                if self.playlist_open {
                    let mut playlist_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(playlist_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    self.draw_playlist(&mut playlist_ui);
                }
                if let Some(info) = &card_info {
                    Self::paint_card(ui, card_rect, info);
                }
            });

        // Context menu: its own area so it can open up or down at the click.
        if let Some(rel) = self.menu_rel {
            let pos = pos2(player_rect.left() + rel.x, player_rect.top() + rel.y);
            let pivot = match (self.menu_right, self.menu_up) {
                (false, false) => Align2::LEFT_TOP,
                (true, false) => Align2::RIGHT_TOP,
                (false, true) => Align2::LEFT_BOTTOM,
                (true, true) => Align2::RIGHT_BOTTOM,
            };
            let out = egui::Area::new(egui::Id::new("haven-context-menu"))
                .order(egui::Order::Foreground)
                .pivot(pivot)
                .fixed_pos(pos)
                .show(ctx, |ui| {
                    egui::Frame::menu(ui.style()).show(ui, |ui| {
                        ui.set_min_width(MENU_WIDTH - 16.0);
                        ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                            self.show_context_menu(ui, ctx)
                        });
                    });
                });
            self.menu_rect = Some(out.response.rect);
            let esc = ctx.input(|i| i.key_pressed(Key::Escape));
            let outside = !opened_now
                && ctx.input(|i| i.pointer.any_pressed())
                && ctx
                    .input(|i| i.pointer.interact_pos())
                    .map_or(false, |p| !out.response.rect.contains(p));
            if self.menu_close || esc || outside {
                self.close_menu();
            }
        }

        self.sync_layout(ctx, dims, base_h);
        if self.engine.shared.playing.load(Relaxed) {
            ctx.request_repaint_after(Duration::from_millis(33)); // smoother waveform updates
        } else if self.drag.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}
