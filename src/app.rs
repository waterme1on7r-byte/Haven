//! UI: custom-painted pill and compact macOS menu-bar layouts.
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
const MENU_BODY_H: f32 = 244.0; // drawn height of the Mini Mixer menu
const MENU_HEIGHT: f32 = 250.0; // window room reserved for it
const MENU_WIDTH: f32 = 200.0;
const SCREEN_MARGIN: f32 = 56.0; // taskbar / Dock allowance at the screen bottom
const PLAYLIST_HEIGHT: f32 = 270.0;
const MENU_BAR_PLAYLIST_HEIGHT: f32 = 136.0;
const SETTINGS_HEIGHT: f32 = 376.0;
const EQ_HEIGHT: f32 = 316.0;
const PLAYLIST_GAP: f32 = 5.0;
const HOVER_CARD_HEIGHT: f32 = 55.0;
const CARD_SLOT: f32 = HOVER_CARD_HEIGHT + 4.0; // window room the card needs
const CARD_HIDE_AFTER: Duration = Duration::from_secs(3);
/// Drawers (playlist / settings / equalizer) are never narrower than this, so the
/// extra-small player (narrower than the settings rows) can still open them.
const MIN_PANEL_W: f32 = 378.0;

/// Window size at launch. On macOS the window permanently includes the strip the
/// hover card is drawn in (see `Haven::card_reserve`).
pub fn initial_window_size(settings: &Settings) -> [f32; 2] {
    let mut d = settings.dims();
    if cfg!(target_os = "macos") && settings.size != 3 {
        d[1] += CARD_SLOT;
    }
    d
}

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
    settings_open: bool,
    eq_open: bool,
    eq_dirty: Option<Instant>, // EQ gains changed; saved shortly after the last change
    panel_up: bool,            // playlist/settings dock above the player instead of below
    panel_was_open: bool,
    updater: crate::update::Updater,
    vol_dirty: Option<Instant>,
    panel_applied: bool,        // the last layout sent included a docked panel
    grow_sent: Option<Instant>, // upward growth: window enlarged, move still pending
    want_top: f32,
    // Context menu: anchored to the click, opens downward or upward.
    menu_rel: Option<egui::Vec2>, // click position relative to the player's top-left
    menu_up: bool,
    menu_right: bool,
    menu_rect: Option<Rect>,
    menu_close: bool,
    menu_opened_at: Option<Instant>,
    menu_applied: bool,         // the last layout sent included room for the menu
    anchor: Option<egui::Pos2>, // player top-left in screen coordinates
    target_outer: Option<egui::Pos2>, // where the last layout command put the window
    minimize_pending: Option<Instant>,
    vol_flash: Option<Instant>,
    drag_moved: bool,
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
    // Keep repainting briefly after input so play/pause state is never shown stale.
    poll_until: Option<Instant>,
    // The play/pause state the last frame actually drew; if the engine changes it
    // right after, one more frame is requested so the icon is never left stale.
    drawn_playing: bool,
    // Set once Haven has asked its window to close for an update restart.
    quit_sent: bool,
    panel_unfocused_at: Option<Instant>,
    focused_last_frame: bool,
    allow_clicks: bool,
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

/// Receives files from a second launch (e.g. double-clicked in Explorer while
/// Haven is already open). A lone launch replaces the queue and plays; launches
/// that arrive in a burst (multi-select) within 1.5 s are added to each other.
fn spawn_ipc(listener: std::net::TcpListener, engine: Engine, ctx: egui::Context) {
    std::thread::spawn(move || {
        use std::io::Read;
        let mut last: Option<Instant> = None;
        for stream in listener.incoming() {
            let Ok(mut s) = stream else {
                continue;
            };
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buf = String::new();
            let _ = (&mut s).take(1 << 20).read_to_string(&mut buf);
            let paths: Vec<PathBuf> = buf
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .collect();
            let commands: Vec<&str> = buf.lines().map(str::trim).collect();
            if commands.len() == 1 && commands[0].starts_with("@HAVEN:") {
                match &commands[0][7..] {
                    "PLAY" => engine.toggle(),
                    "PREV" => engine.prev(),
                    "NEXT" => engine.next(),
                    "SHOW" => {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    "QUIT" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                    _ => {}
                }
                ctx.request_repaint();
                continue;
            }
            let q = scan(&paths);
            if !q.is_empty() {
                if last.map_or(false, |t| t.elapsed() < Duration::from_millis(1500)) {
                    engine.append(q);
                } else {
                    engine.queue(q);
                }
                last = Some(Instant::now());
            }
            // Bring the player forward (also for a plain second launch with no files).
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    });
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
    fn menu_bar_mode(&self) -> bool {
        cfg!(target_os = "macos") && self.settings.size == 3
    }

    /// Extra window height kept below the player for the hover card on macOS.
    ///
    /// Resizing a transparent window every time the card appears or disappears (which
    /// also happens on every click and drag, since a press hides the card) makes the
    /// whole player flash on macOS. With this room always present the window geometry
    /// never changes for the card; the strip is fully transparent, so it is click-through.
    fn card_reserve(&self) -> f32 {
        if cfg!(target_os = "macos") && !self.menu_bar_mode() {
            CARD_SLOT
        } else {
            0.0
        }
    }

    /// Width of the window and of docked drawers.
    fn panel_width(&self, dims: [f32; 2]) -> f32 {
        if self.menu_bar_mode() {
            dims[0]
        } else {
            dims[0].max(MIN_PANEL_W)
        }
    }

    fn playlist_height(&self) -> f32 {
        if self.menu_bar_mode() {
            MENU_BAR_PLAYLIST_HEIGHT
        } else {
            PLAYLIST_HEIGHT
        }
    }

    pub fn new(
        cc: &eframe::CreationContext<'_>,
        args: Vec<PathBuf>,
        #[allow(unused_mut)] mut settings: Settings,
        listener: std::net::TcpListener,
    ) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        #[cfg(target_os = "macos")]
        {
            crate::macos_open::install(); // Finder "Open With" / double-click
            if settings.size == 3 {
                if crate::macos_open::set_menu_bar_mode(true) {
                    cc.egui_ctx
                        .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                } else {
                    settings.size = 2;
                    settings.save();
                }
            }
        }
        let engine = Engine::new();
        let q = scan(&args);
        if !q.is_empty() {
            engine.queue(q);
        }
        let d = initial_window_size(&settings);
        if let Some(v) = settings.volume {
            engine.shared.set_vol(v); // restore the last used volume
        }
        engine.shared.set_eq(settings.eq_on, settings.eq); // restore the equalizer
        spawn_ipc(listener, engine.clone(), cc.egui_ctx.clone()); // files from a second launch
        let updater = crate::update::Updater::new();
        updater.auto_check(); // silent, at most once a day
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
            settings_open: false,
            eq_open: false,
            eq_dirty: None,
            panel_up: false,
            panel_was_open: false,
            updater,
            vol_dirty: None,
            panel_applied: false,
            grow_sent: None,
            want_top: 0.0,
            menu_rel: None,
            menu_up: false,
            menu_right: false,
            menu_rect: None,
            menu_close: false,
            menu_opened_at: None,
            menu_applied: false,
            anchor: None,
            target_outer: None,
            minimize_pending: None,
            vol_flash: None,
            drag_moved: false,
            card_visible: false,
            card_above: true,
            card_suppressed: false,
            card_last: None,
            card_hide_at: None,
            top_pad: 0.0,
            req: [0.0, d[0], d[1]],
            layout_until: None,
            poll_until: None,
            drawn_playing: false,
            quit_sent: false,
            panel_unfocused_at: None,
            focused_last_frame: false,
            allow_clicks: false,
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
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if let Some(r) = ctx
            .input(|i| i.viewport().outer_rect)
            .filter(|_| !self.layout_busy() && !minimized)
        {
            self.anchor = Some(pos2(r.min.x, r.min.y + self.top_pad));
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
        self.menu_opened_at = None;
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
                let below = m.y - screen_y - SCREEN_MARGIN;
                let above = screen_y - 12.0;
                below < MENU_HEIGHT && above > below
            }
            _ => false,
        };
        self.menu_right = (pt.x - origin.x) + MENU_WIDTH > width;
        self.menu_rel = Some(pt - origin);
        self.menu_opened_at = Some(Instant::now());
        self.menu_rect = None;
        self.menu_close = false;
    }

    /// Round / rounded-square button face used by the Mini Mixer menu.
    fn menu_btn(
        ui: &mut egui::Ui,
        rect: Rect,
        id: &str,
        active: bool,
        circle: bool,
    ) -> egui::Response {
        let resp = ui
            .interact(rect, ui.id().with(id), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let rr = Rounding::same(if circle { rect.width() / 2.0 } else { 11.0 });
        let (fill, edge) = if active {
            (RED, RED)
        } else if resp.hovered() {
            (
                Color32::from_rgb(0x2b, 0x2c, 0x2f),
                Color32::from_rgb(0x4a, 0x4b, 0x4f),
            )
        } else {
            (BTN_FACE, PILL_EDGE)
        };
        ui.painter().rect_filled(rect, rr, fill);
        ui.painter().rect_stroke(rect, rr, Stroke::new(1.0, edge));
        resp
    }

    fn menu_bar_button(
        ui: &mut egui::Ui,
        rect: Rect,
        id: &str,
        glyph: &str,
        active: bool,
        circular: bool,
    ) -> egui::Response {
        let resp = ui
            .interact(rect, ui.id().with(id), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let painter = ui.painter();
        if circular {
            painter.circle_filled(
                rect.center(),
                rect.width() * 0.5,
                if active {
                    Color32::from_rgb(0x3b, 0x21, 0x26)
                } else if resp.hovered() {
                    Color32::from_rgb(0x2b, 0x2c, 0x2f)
                } else {
                    BTN_FACE
                },
            );
            painter.circle_stroke(
                rect.center(),
                rect.width() * 0.5,
                Stroke::new(1.0, PILL_EDGE),
            );
        } else {
            painter.rect_filled(
                rect,
                Rounding::same(11.0),
                if active {
                    Color32::from_rgb(0x3b, 0x21, 0x26)
                } else if resp.hovered() {
                    Color32::from_rgb(0x2b, 0x2c, 0x2f)
                } else {
                    WELL
                },
            );
            painter.rect_stroke(rect, Rounding::same(11.0), Stroke::new(1.0, PILL_EDGE));
        }
        Self::glyph(
            painter,
            rect.center(),
            glyph,
            if active { RED } else { ICON },
        );
        resp
    }

    /// Line-art glyphs (about 16 px box) so the menu needs no icon font.
    fn glyph(p: &egui::Painter, c: egui::Pos2, kind: &str, col: Color32) {
        let st = Stroke::new(1.6, col);
        let pt = |x: f32, y: f32| c + vec2(x, y);
        let tri = |a: (f32, f32), b: (f32, f32), d: (f32, f32)| {
            p.add(Shape::convex_polygon(
                vec![pt(a.0, a.1), pt(b.0, b.1), pt(d.0, d.1)],
                col,
                Stroke::NONE,
            ));
        };
        let line = |pts: &[(f32, f32)]| {
            p.add(Shape::line(pts.iter().map(|q| pt(q.0, q.1)).collect(), st));
        };
        let closed = |pts: &[(f32, f32)]| {
            p.add(Shape::closed_line(
                pts.iter().map(|q| pt(q.0, q.1)).collect(),
                st,
            ));
        };
        match kind {
            "prev" => {
                line(&[(-6.0, -6.0), (-6.0, 6.0)]);
                tri((7.0, -6.0), (7.0, 6.0), (-3.0, 0.0));
            }
            "next" => {
                line(&[(6.0, -6.0), (6.0, 6.0)]);
                tri((-7.0, -6.0), (-7.0, 6.0), (3.0, 0.0));
            }
            "play" => tri((-4.0, -7.0), (-4.0, 7.0), (7.0, 0.0)),
            "pause" => {
                line(&[(-3.5, -6.0), (-3.5, 6.0)]);
                line(&[(3.5, -6.0), (3.5, 6.0)]);
            }
            "shuf" => {
                line(&[(-7.0, -4.0), (-3.0, -4.0), (3.0, 4.0), (5.0, 4.0)]);
                line(&[(-7.0, 4.0), (-3.0, 4.0), (-1.0, 1.5)]);
                line(&[(1.0, -1.5), (3.0, -4.0), (5.0, -4.0)]);
                tri((4.0, -7.0), (4.0, -1.0), (8.0, -4.0));
                tri((4.0, 1.0), (4.0, 7.0), (8.0, 4.0));
            }
            "rep" => {
                line(&[(-6.0, 3.0), (-6.0, -4.0), (3.0, -4.0)]);
                tri((2.0, -7.0), (2.0, -1.0), (7.0, -4.0));
                line(&[(6.0, -3.0), (6.0, 4.0), (-3.0, 4.0)]);
                tri((-2.0, 1.0), (-2.0, 7.0), (-7.0, 4.0));
            }
            "vol" | "mute" => {
                p.add(Shape::convex_polygon(
                    vec![
                        pt(-7.0, -2.5),
                        pt(-3.5, -2.5),
                        pt(1.0, -6.0),
                        pt(1.0, 6.0),
                        pt(-3.5, 2.5),
                        pt(-7.0, 2.5),
                    ],
                    col,
                    Stroke::NONE,
                ));
                if kind == "vol" {
                    line(&[(4.0, -3.5), (6.0, 0.0), (4.0, 3.5)]);
                } else {
                    line(&[(4.0, -3.0), (9.0, 3.0)]);
                    line(&[(9.0, -3.0), (4.0, 3.0)]);
                }
            }
            "list" => {
                line(&[(-6.0, -5.0), (6.0, -5.0)]);
                line(&[(-6.0, 0.0), (6.0, 0.0)]);
                line(&[(-6.0, 5.0), (1.0, 5.0)]);
            }
            "cog" => {
                line(&[(-7.0, -3.5), (7.0, -3.5)]);
                line(&[(-7.0, 3.5), (7.0, 3.5)]);
                p.circle_filled(pt(-2.5, -3.5), 2.4, col);
                p.circle_filled(pt(3.0, 3.5), 2.4, col);
            }
            "eq" => {
                line(&[(-6.0, -6.0), (-6.0, 6.0)]);
                line(&[(0.0, -6.0), (0.0, 6.0)]);
                line(&[(6.0, -6.0), (6.0, 6.0)]);
                p.circle_filled(pt(-6.0, 1.0), 2.2, col);
                p.circle_filled(pt(0.0, -2.0), 2.2, col);
                p.circle_filled(pt(6.0, 3.0), 2.2, col);
            }
            "file" => {
                closed(&[
                    (-5.0, -7.0),
                    (1.5, -7.0),
                    (5.5, -3.0),
                    (5.5, 7.0),
                    (-5.0, 7.0),
                ]);
                line(&[(1.5, -7.0), (1.5, -3.0), (5.5, -3.0)]);
            }
            "fold" => closed(&[
                (-7.0, -4.5),
                (-2.0, -4.5),
                (0.0, -2.5),
                (7.0, -2.5),
                (7.0, 5.0),
                (-7.0, 5.0),
            ]),
            "min" => line(&[(-6.0, 5.0), (6.0, 5.0)]),
            _ => {
                // power
                line(&[(0.0, -7.0), (0.0, 0.0)]);
                let arc: Vec<(f32, f32)> = (0..=20)
                    .map(|i| {
                        let th = (40.0 + i as f32 * 14.0).to_radians();
                        (6.5 * th.sin(), 1.5 - 6.5 * th.cos())
                    })
                    .collect();
                line(&arc);
            }
        }
    }

    /// Mini Mixer context menu: transport, vertical volume fader, toggle chips
    /// and an icon-only footer, all drawn in the player's own palette.
    fn draw_menu(&mut self, ui: &mut egui::Ui, rect: Rect, ctx: &egui::Context) {
        let sh = self.engine.shared.clone();
        let p = ui.painter().clone();
        let r = Rounding::same(18.0);
        p.rect_filled(
            rect.translate(vec2(0.0, 3.0)),
            r,
            Color32::from_black_alpha(70),
        );
        p.rect_filled(rect, r, PILL);
        p.rect_stroke(rect, r, Stroke::new(1.0, PILL_EDGE));
        let pad = 12.0;
        let (left, right) = (rect.left() + pad, rect.right() - pad);

        // ---- transport (stays open so you can skip around) ----
        let ty = rect.top() + pad + 21.0;
        let playing = sh.playing.load(Relaxed) && !sh.queue_ended.load(Relaxed);
        self.drawn_playing = playing;
        let has_media = !sh.queue_info.lock().unwrap().is_empty();
        let prev = Rect::from_center_size(pos2(left + 17.0, ty), vec2(34.0, 34.0));
        let next = Rect::from_center_size(pos2(right - 17.0, ty), vec2(34.0, 34.0));
        let play = Rect::from_center_size(pos2(rect.center().x, ty), vec2(42.0, 42.0));
        if self.allow_clicks && Self::menu_btn(ui, prev, "prev", false, false).clicked() {
            self.engine.prev();
        }
        Self::glyph(&p, prev.center(), "prev", ICON);
        if self.allow_clicks
            && Self::menu_btn(ui, play, "play", has_media, true).clicked()
            && has_media
        {
            self.engine.toggle();
        }
        Self::glyph(
            &p,
            play.center(),
            if playing { "pause" } else { "play" },
            if has_media {
                Color32::WHITE
            } else {
                Color32::from_gray(0x5c)
            },
        );
        if self.allow_clicks && Self::menu_btn(ui, next, "next", false, false).clicked() {
            self.engine.next();
        }
        Self::glyph(&p, next.center(), "next", ICON);

        // ---- vertical volume fader (bottom of it toggles mute) ----
        let fader = Rect::from_min_size(pos2(left, rect.top() + pad + 52.0), vec2(34.0, 118.0));
        // Mouse wheel anywhere on the menu changes the volume.
        if ui.rect_contains_pointer(rect) {
            let dy = ctx.input(|i| i.smooth_scroll_delta.y);
            if dy.abs() > 0.01 {
                sh.set_vol(sh.vol() + dy * 0.0015);
                sh.muted.store(false, Relaxed);
            }
        }
        let muted = sh.muted.load(Relaxed);
        let vol = sh.vol().clamp(0.0, 1.0);
        let fr = Rounding::same(12.0);
        p.rect_filled(fader, fr, WELL);
        let fill_h = (fader.height() * vol).max(if vol > 0.0 { 12.0 } else { 0.0 });
        if fill_h > 0.0 {
            p.rect_filled(
                Rect::from_min_max(
                    pos2(fader.left(), fader.bottom() - fill_h),
                    fader.right_bottom(),
                ),
                fr,
                if muted {
                    Color32::from_rgb(0x5a, 0x5b, 0x5e)
                } else {
                    RED
                },
            );
        }
        // level read-out at the top of the fader
        p.text(
            pos2(fader.center().x, fader.top() + 11.0),
            Align2::CENTER_CENTER,
            if muted {
                "mute".to_string()
            } else {
                format!("{}", (vol * 100.0).round() as i32)
            },
            FontId::proportional(10.5),
            if vol > 0.88 && !muted {
                Color32::WHITE
            } else {
                Color32::from_gray(0xa0)
            },
        );
        let mute_zone = Rect::from_min_max(
            pos2(fader.left(), fader.bottom() - 30.0),
            fader.right_bottom(),
        );
        Self::glyph(
            &p,
            mute_zone.center(),
            if muted { "mute" } else { "vol" },
            Color32::WHITE,
        );
        let fresp = ui
            .interact(fader, ui.id().with("fader"), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if let Some(pos) = fresp.interact_pointer_pos() {
            if self.allow_clicks && fresp.clicked() && mute_zone.contains(pos) {
                sh.muted.store(!muted, Relaxed);
            } else if self.allow_clicks && (fresp.clicked() || fresp.dragged()) {
                sh.set_vol(((fader.bottom() - pos.y) / fader.height()).clamp(0.0, 1.0));
                sh.muted.store(false, Relaxed);
            }
        }

        // ---- toggle chips ----
        let chip_x = fader.right() + 12.0;
        let chip_h = 35.0;
        let chip_gap = (fader.height() - chip_h * 3.0) / 2.0;
        let rep = sh.repeat.load(Relaxed);
        let shuffle = sh.shuffle.load(Relaxed);
        let chips: [(&str, &str, bool, String); 3] = [
            ("c-shuf", "shuf", shuffle, "Shuffle".to_string()),
            (
                "c-rep",
                "rep",
                rep != 0,
                ["Repeat off", "Repeat all", "Repeat one"][(rep as usize).min(2)].to_string(),
            ),
            ("c-list", "list", self.playlist_open, "Playlist".to_string()),
        ];
        for (i, (id, g, active, text)) in chips.iter().enumerate() {
            let cr = Rect::from_min_size(
                pos2(chip_x, fader.top() + i as f32 * (chip_h + chip_gap)),
                vec2(right - chip_x, chip_h),
            );
            let resp = Self::menu_btn(ui, cr, id, *active, false);
            let ink = if *active { Color32::WHITE } else { ICON };
            Self::glyph(&p, pos2(cr.left() + 17.0, cr.center().y), g, ink);
            p.text(
                pos2(cr.left() + 32.0, cr.center().y),
                Align2::LEFT_CENTER,
                text,
                FontId::proportional(11.5),
                ink,
            );
            if self.allow_clicks && resp.clicked() {
                match i {
                    0 => sh.shuffle.store(!shuffle, Relaxed),
                    1 => sh.repeat.store((rep + 1) % 3, Relaxed),
                    _ => {
                        self.playlist_open = !self.playlist_open;
                        if self.playlist_open {
                            self.settings_open = false;
                            self.eq_open = false;
                        }
                        self.menu_close = true;
                    }
                }
            }
        }

        // ---- footer: icon-only, tooltips name them ----
        let sep_y = fader.bottom() + 10.0;
        p.line_segment(
            [pos2(left, sep_y), pos2(right, sep_y)],
            Stroke::new(1.0, PILL_EDGE),
        );
        let foot_y = sep_y + 9.0 + 16.0;
        let foot: [(&str, &str, &str); 5] = [
            ("f-cog", "cog", "Settings"),
            ("f-file", "file", "Open files…"),
            ("f-fold", "fold", "Open folder…"),
            ("f-min", "min", "Minimize"),
            ("f-pow", "pow", "Exit"),
        ];
        for (i, (id, g, tip)) in foot.iter().enumerate() {
            let c = pos2(left + 16.0 + i as f32 * 36.0, foot_y);
            let br = Rect::from_center_size(c, vec2(32.0, 32.0));
            let resp = Self::menu_btn(
                ui,
                br,
                id,
                i == 0 && (self.settings_open || self.eq_open),
                false,
            )
            .on_hover_text(*tip);
            let ink = if i == 4 {
                RED
            } else if i == 0 && (self.settings_open || self.eq_open) {
                Color32::WHITE
            } else {
                ICON
            };
            Self::glyph(&p, c, g, ink);
            if i == 0 && self.updater.available() {
                p.circle_filled(c + vec2(9.5, -9.5), 3.5, RED); // update waiting
            }
            if self.allow_clicks && resp.clicked() {
                match i {
                    0 => {
                        let open = self.settings_open || self.eq_open;
                        self.settings_open = !open;
                        self.eq_open = false;
                        if self.settings_open {
                            self.playlist_open = false;
                        }
                        self.menu_close = true;
                    }
                    1 => {
                        if let Some(f) = rfd::FileDialog::new()
                            .add_filter("Audio", EXTS)
                            .pick_files()
                        {
                            self.open(f, true);
                        }
                        self.menu_close = true;
                    }
                    2 => {
                        if let Some(f) = rfd::FileDialog::new().pick_folder() {
                            self.open(vec![f], true);
                        }
                        self.menu_close = true;
                    }
                    3 => {
                        // Minimize only after the window has shrunk back, otherwise
                        // it is restored at the wrong place.
                        self.minimize_pending = Some(Instant::now());
                        self.menu_close = true;
                    }
                    _ => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                }
            }
        }
    }

    fn keys(&mut self, ctx: &egui::Context) {
        let sh = self.engine.shared.clone();
        let pos = sh.pos_ms.load(Relaxed) as f64 / 1000.0;
        let dur = sh.dur_ms.load(Relaxed) as f64 / 1000.0;
        let has_media = !sh.queue_info.lock().unwrap().is_empty();
        let mut toggle_eq = false;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Space) && has_media {
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
            if i.consume_key(Modifiers::NONE, Key::E) {
                toggle_eq = true;
            }
        });
        if toggle_eq {
            self.settings.eq_on = !self.settings.eq_on;
            self.push_eq();
        }
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
        if self.menu_bar_mode() {
            self.draw_menu_bar(ui);
            return;
        }
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
        let has_media = !sh.queue_info.lock().unwrap().is_empty();
        // A native window move must begin on the same mouse-down event. Starting
        // it after egui's drag threshold is crossed is intermittent on Windows.
        let begin_window_drag = ctx.input(|i| {
            if self.allow_clicks && i.pointer.button_pressed(egui::PointerButton::Primary) {
                i.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(pt) = begin_window_drag {
            let on_progress_ring = nearest_frac(&ring, pt).0 <= sw * 2.0;
            if full.contains(pt)
                && !well.contains(pt)
                && !(has_media && brect.contains(pt))
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
        let btn = ui.interact(brect, id.with("btn"), Sense::click_and_drag());
        let btn = if has_media {
            btn.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            btn
        };
        if self.allow_clicks && btn.clicked() && has_media {
            self.engine.toggle();
        }

        let wresp = ui.interact(well, id.with("wave"), Sense::click_and_drag());
        let dur = sh.dur_ms.load(Relaxed) as f64 / 1000.0;
        let real_pos = sh.pos_ms.load(Relaxed) as f64 / 1000.0;
        // Primary button only: a right-click on the wave (it opens the menu) must
        // never scrub or seek, because every seek flushes the audio buffer.
        if self.allow_clicks && wresp.drag_started_by(egui::PointerButton::Primary) {
            self.drag_moved = false;
            if ctx.input(|i| i.modifiers.alt) && !self.settings.locked {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); // Alt/Option+drag moves the window
            } else {
                self.drag = Some(real_pos);
            }
        }
        if self.allow_clicks && wresp.dragged_by(egui::PointerButton::Primary) {
            if wresp.drag_delta().x != 0.0 {
                self.drag_moved = true;
            }
            if let Some(dp) = self.drag.as_mut() {
                *dp = (*dp - (wresp.drag_delta().x / pitch) as f64 * SECS_PER_BAR).clamp(0.0, dur);
            }
        }
        if self.allow_clicks && wresp.drag_stopped_by(egui::PointerButton::Primary) {
            if let Some(dp) = self.drag.take() {
                if self.drag_moved {
                    self.engine.seek(dp);
                }
            }
            self.drag_moved = false;
        }
        // Mouse wheel over the waveform = volume (only here).
        if wresp.hovered() {
            let dy = ctx.input(|i| i.smooth_scroll_delta.y);
            if dy.abs() > 0.01 {
                sh.set_vol(sh.vol() + dy * 0.0015);
                sh.muted.store(false, Relaxed);
                self.vol_flash = Some(Instant::now());
            }
        }
        if self.allow_clicks && wresp.clicked() {
            if let Some(pt) = wresp
                .interact_pointer_pos()
                .filter(|pt| (pt.x - cx).abs() > 4.0)
            {
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
        if self.allow_clicks && ring_resp.drag_started() {
            self.ring_active = ring_resp
                .interact_pointer_pos()
                .map_or(false, |pt| nearest_frac(&ring, pt).0 <= sw * 2.0);
        }
        if self.allow_clicks && self.ring_active && ring_resp.dragged() {
            if let Some(pt) = ring_resp.interact_pointer_pos() {
                self.drag = Some(nearest_frac(&ring, pt).1 as f64 * dur);
            }
        }
        if self.allow_clicks && ring_resp.drag_stopped() {
            if self.ring_active {
                if let Some(d) = self.drag.take() {
                    self.engine.seek(d);
                }
            }
            self.ring_active = false;
        }
        if self.allow_clicks && ring_resp.clicked() {
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
        let hot = if has_media && btn.hovered() {
            1.15
        } else {
            1.0
        };
        let icon_col = if has_media {
            ICON
        } else {
            ICON.gamma_multiply(0.3)
        };
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
        let playing_now = sh.playing.load(Relaxed) && !sh.queue_ended.load(Relaxed);
        self.drawn_playing = playing_now;
        if playing_now {
            let (bw, bh, gap) = (d * 0.105, d * 0.315, d * 0.11);
            for s in [-1.0f32, 1.0] {
                let c = pos2(bc.x + s * (gap / 2.0 + bw / 2.0), bc.y);
                p.rect_filled(Rect::from_center_size(c, vec2(bw, bh)), bw * 0.3, icon_col);
            }
        } else {
            let s = d * 0.17;
            p.add(Shape::convex_polygon(
                vec![
                    pos2(bc.x - s * 0.7, bc.y - s * 1.1),
                    pos2(bc.x - s * 0.7, bc.y + s * 1.1),
                    pos2(bc.x + s * 1.2, bc.y),
                ],
                icon_col,
                Stroke::NONE,
            ));
        }

        // ---- waveform well ----
        let wr = Rounding::same(h * 0.14);
        p.rect_filled(well, wr, WELL);
        let wp = p.with_clip_rect(well);

        // ---- waveform: five styles (centered playhead, audio scrolls underneath) ----
        let posbar = pos / SECS_PER_BAR;
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
                // Wave: one clean sine. Its height follows a smoothed loudness
                // envelope and its phase is tied to track time, so it scrolls
                // with the audio. No noise term, so the line stays smooth.
                let wl = (pitch * 4.2) as f64; // wavelength in px
                let mut pts: Vec<egui::Pos2> = Vec::new();
                let mut split = 0usize;
                let mut x = well.left();
                while x <= well.right() + 1.5 {
                    let tb = posbar + ((x - cx) / pitch) as f64;
                    let t = tb * SECS_PER_BAR;
                    let mut y = mid;
                    if t >= 0.0 && (dur <= 0.0 || t <= dur) {
                        let (core, halo) = level(t - 0.30, t + 0.30);
                        let amp = 0.06 + 0.94 * (core * 0.75 + halo * 0.25).min(1.0);
                        let ph = ((tb * pitch as f64 / wl) % 1.0) as f32 * std::f32::consts::TAU;
                        y = mid - ph.sin() * amp * maxh * 0.48;
                    }
                    if x < cx {
                        split = pts.len() + 1;
                    }
                    pts.push(pos2(x, y));
                    x += 1.5;
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
                        Stroke::new(4.0, Color32::from_gray(0x5a).gamma_multiply(0.14)),
                    ));
                    wp.add(Shape::line(
                        past,
                        Stroke::new(1.8, Color32::from_gray(0x78)),
                    ));
                }
                if fut.len() > 1 {
                    wp.add(Shape::line(
                        fut.clone(),
                        Stroke::new(4.0, BAR_FUTURE.gamma_multiply(0.14)),
                    ));
                    wp.add(Shape::line(fut, Stroke::new(1.8, BAR_FUTURE)));
                }
            } else {
                for i in (c0 - half)..=(c0 + half) {
                    let t = i as f64 * SECS_PER_BAR;
                    if i < 0 || (dur > 0.0 && t > dur) {
                        continue;
                    }
                    let x = cx + (i as f64 - posbar) as f32 * pitch;
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
                                let hh = (bw * 2.0 + hl * maxh).min(maxh);
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
                            // Skyline: round-topped bars standing on a baseline, with a
                            // short faded reflection underneath.
                            let bw = pitch * 0.30;
                            let base = well.top() + well.height() * 0.60;
                            let full_h = well.height() * 0.44;
                            let up = (bw + (0.22 + 0.78 * halo) * full_h).min(full_h);
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
                            let rtop = base + 2.0;
                            let rh = up * 0.36;
                            let mut m = egui::epaint::Mesh::default();
                            m.colored_vertex(pos2(x - bw / 2.0, rtop), col.gamma_multiply(0.34));
                            m.colored_vertex(pos2(x + bw / 2.0, rtop), col.gamma_multiply(0.34));
                            m.colored_vertex(
                                pos2(x + bw / 2.0, rtop + rh),
                                col.gamma_multiply(0.04),
                            );
                            m.colored_vertex(
                                pos2(x - bw / 2.0, rtop + rh),
                                col.gamma_multiply(0.04),
                            );
                            m.add_triangle(0, 1, 2);
                            m.add_triangle(0, 2, 3);
                            wp.add(Shape::mesh(m));
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

        // brief volume read-out after a wheel scroll
        if let Some(t) = self.vol_flash {
            if t.elapsed() < Duration::from_millis(900) {
                let txt = if sh.muted.load(Relaxed) {
                    "Muted".to_string()
                } else {
                    format!("Volume {}%", (sh.vol() * 100.0).round() as i32)
                };
                let rc = Rect::from_center_size(
                    pos2(well.center().x, well.top() + well.height() * 0.2),
                    vec2(96.0, 22.0),
                );
                p.rect_filled(rc, Rounding::same(11.0), Color32::from_black_alpha(190));
                p.text(
                    rc.center(),
                    Align2::CENTER_CENTER,
                    txt,
                    FontId::proportional(11.0),
                    Color32::from_gray(0xe6),
                );
                ctx.request_repaint_after(Duration::from_millis(100));
            } else {
                self.vol_flash = None;
            }
        }
    }

    /// Compact macOS layout styled as a menu-bar player popover.
    fn draw_menu_bar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let ctx = ui.ctx().clone();
        let painter = ui.painter().clone();
        let sh = self.engine.shared.clone();
        let frame = rect.shrink(8.0);
        let rr = Rounding::same(17.0);
        painter.rect_filled(
            frame.translate(vec2(0.0, 3.0)),
            rr,
            Color32::from_black_alpha(70),
        );
        painter.rect_filled(frame, rr, PILL);
        painter.rect_stroke(frame, rr, Stroke::new(1.0, PILL_EDGE));

        let playing = sh.playing.load(Relaxed) && !sh.queue_ended.load(Relaxed);
        self.drawn_playing = playing;
        let queue = sh.queue_info.lock().unwrap().clone();
        let has_media = !queue.is_empty();

        let info_box = Rect::from_min_max(
            pos2(frame.left() + 12.0, frame.top() + 12.0),
            pos2(frame.right() - 12.0, frame.top() + 78.0),
        );
        painter.rect_filled(info_box, Rounding::same(12.0), WELL);
        painter.rect_stroke(info_box, Rounding::same(12.0), Stroke::new(1.0, WELL_EDGE));

        let mini = Rect::from_center_size(
            pos2(info_box.left() + 34.0, info_box.center().y),
            vec2(42.0, 42.0),
        );
        let mini_resp = ui
            .interact(mini, ui.id().with("mb-mini-play"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        painter.circle_filled(mini.center(), 20.0, BTN_FACE);
        painter.circle_stroke(mini.center(), 20.0, Stroke::new(1.0, PILL_EDGE));
        painter.circle_stroke(mini.center(), 17.0, Stroke::new(2.0, RING_TRACK));
        let duration = sh.dur_ms.load(Relaxed) as f32;
        let elapsed = sh.pos_ms.load(Relaxed) as f32;
        let progress = if duration > 0.0 {
            (elapsed / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if progress > 0.0 {
            let points: Vec<_> = (0..=((progress * 48.0).ceil() as usize).max(1))
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * progress * i as f32
                            / ((progress * 48.0).ceil() as f32).max(1.0);
                    pos2(
                        mini.center().x + 17.0 * a.cos(),
                        mini.center().y + 17.0 * a.sin(),
                    )
                })
                .collect();
            painter.add(Shape::line(points, Stroke::new(2.5, RED)));
        }
        painter.circle_filled(mini.center(), 13.0, RED);
        Self::glyph(
            &painter,
            mini.center(),
            if playing { "pause" } else { "play" },
            Color32::WHITE,
        );
        if self.allow_clicks && mini_resp.clicked() && has_media {
            self.engine.toggle();
        }

        let track = self.info.clone();
        let title = track
            .as_ref()
            .map_or("Nothing playing", |t| t.title.as_str());
        let title_rect = Rect::from_min_max(
            pos2(info_box.left() + 66.0, info_box.top() + 15.0),
            pos2(info_box.right() - 10.0, info_box.top() + 37.0),
        );
        painter.text(
            title_rect.left_center(),
            Align2::LEFT_CENTER,
            fit(&ctx, title, FontId::proportional(13.0), title_rect.width()),
            FontId::proportional(13.0),
            Color32::from_gray(0xed),
        );
        let details = track.as_ref().map_or_else(
            || "Drop audio or use Open".to_string(),
            |t| {
                let pos = sh.pos_ms.load(Relaxed) as f64 / 1000.0;
                let dur = t.duration_ms as f64 / 1000.0;
                let rate = if t.sample_rate > 0 {
                    format!(" · {:.1} kHz", t.sample_rate as f32 / 1000.0)
                } else {
                    String::new()
                };
                format!(
                    "{} / {} · {}{}",
                    fmt_time(pos),
                    fmt_time(dur),
                    t.codec,
                    rate
                )
            },
        );
        painter.text(
            pos2(title_rect.left(), info_box.top() + 51.0),
            Align2::LEFT_CENTER,
            fit(
                &ctx,
                &details,
                FontId::proportional(10.5),
                title_rect.width(),
            ),
            FontId::proportional(10.5),
            Color32::from_gray(0x91),
        );

        // Drag the compact panel from its open background, leaving controls clickable.
        let control_rects = [
            mini,
            Rect::from_center_size(
                pos2(frame.left() + 31.0, frame.top() + 119.0),
                vec2(36.0, 36.0),
            ),
            Rect::from_center_size(
                pos2(frame.left() + 77.0, frame.top() + 119.0),
                vec2(36.0, 36.0),
            ),
            Rect::from_center_size(
                pos2(frame.center().x, frame.top() + 119.0),
                vec2(50.0, 50.0),
            ),
            Rect::from_center_size(
                pos2(frame.right() - 77.0, frame.top() + 119.0),
                vec2(36.0, 36.0),
            ),
            Rect::from_center_size(
                pos2(frame.right() - 31.0, frame.top() + 119.0),
                vec2(36.0, 36.0),
            ),
            Rect::from_min_max(
                pos2(frame.left() + 12.0, frame.top() + 153.0),
                pos2(frame.right() - 12.0, frame.top() + 191.0),
            ),
            Rect::from_min_max(
                pos2(frame.left() + 12.0, frame.top() + 201.0),
                pos2(frame.right() - 12.0, frame.top() + 239.0),
            ),
            Rect::from_min_max(
                pos2(frame.left() + 20.0, frame.bottom() - 43.0),
                pos2(frame.right() - 20.0, frame.bottom()),
            ),
        ];
        let pressed = ctx.input(|i| {
            if self.allow_clicks && i.pointer.button_pressed(egui::PointerButton::Primary) {
                i.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(pt) = pressed {
            if frame.contains(pt)
                && !control_rects.iter().any(|r| r.contains(pt))
                && !self.settings.locked
            {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        }

        // Main transport row.
        let shuffling = sh.shuffle.load(Relaxed);
        let repeat = sh.repeat.load(Relaxed);
        let shuffle_rect = control_rects[1];
        let shuffle_resp =
            Self::menu_bar_button(ui, shuffle_rect, "mb-shuffle", "shuf", shuffling, true);
        if self.allow_clicks && shuffle_resp.clicked() {
            sh.shuffle.store(!shuffling, Relaxed);
        }
        let prev_rect = control_rects[2];
        if self.allow_clicks
            && Self::menu_bar_button(ui, prev_rect, "mb-prev", "prev", false, true).clicked()
        {
            self.engine.prev();
        }
        let play_rect = control_rects[3];
        let play_resp = ui
            .interact(play_rect, ui.id().with("mb-play"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        painter.circle_filled(
            play_rect.center(),
            24.0,
            if has_media {
                RED
            } else {
                RED.gamma_multiply(0.35)
            },
        );
        Self::glyph(
            &painter,
            play_rect.center(),
            if playing { "pause" } else { "play" },
            Color32::WHITE.gamma_multiply(if has_media { 1.0 } else { 0.5 }),
        );
        if self.allow_clicks && play_resp.clicked() && has_media {
            self.engine.toggle();
        }
        let next_rect = control_rects[4];
        if self.allow_clicks
            && Self::menu_bar_button(ui, next_rect, "mb-next", "next", false, true).clicked()
        {
            self.engine.next();
        }
        let repeat_rect = control_rects[5];
        let repeat_resp =
            Self::menu_bar_button(ui, repeat_rect, "mb-repeat", "rep", repeat != 0, true)
                .on_hover_text(
                    ["Repeat off", "Repeat all", "Repeat one"][(repeat as usize).min(2)],
                );
        if self.allow_clicks && repeat_resp.clicked() {
            sh.repeat.store((repeat + 1) % 3, Relaxed);
        }

        // Horizontal volume control.
        let volume_box = control_rects[6];
        painter.rect_filled(volume_box, Rounding::same(11.0), WELL);
        painter.rect_stroke(
            volume_box,
            Rounding::same(11.0),
            Stroke::new(1.0, PILL_EDGE),
        );
        let muted = sh.muted.load(Relaxed);
        let speaker = Rect::from_center_size(
            pos2(volume_box.left() + 19.0, volume_box.center().y),
            vec2(28.0, 28.0),
        );
        if self.allow_clicks
            && Self::menu_bar_button(
                ui,
                speaker,
                "mb-mute",
                if muted { "mute" } else { "vol" },
                false,
                true,
            )
            .clicked()
        {
            sh.muted.store(!muted, Relaxed);
        }
        let slider = Rect::from_min_max(
            pos2(speaker.right() + 8.0, volume_box.center().y - 6.0),
            pos2(volume_box.right() - 50.0, volume_box.center().y + 6.0),
        );
        let slider_resp = ui.interact(
            slider.expand2(vec2(0.0, 8.0)),
            ui.id().with("mb-volume"),
            Sense::click_and_drag(),
        );
        let vol = sh.vol().clamp(0.0, 1.0);
        painter.rect_filled(
            slider,
            Rounding::same(6.0),
            Color32::from_rgb(0x3b, 0x3c, 0x3e),
        );
        let fill = Rect::from_min_max(
            slider.min,
            pos2(slider.left() + slider.width() * vol, slider.bottom()),
        );
        if vol > 0.0 {
            painter.rect_filled(fill, Rounding::same(6.0), RED);
        }
        painter.circle_filled(
            pos2(slider.left() + slider.width() * vol, slider.center().y),
            7.0,
            Color32::from_gray(0xe6),
        );
        if let Some(pt) = slider_resp
            .interact_pointer_pos()
            .filter(|_| self.allow_clicks && (slider_resp.clicked() || slider_resp.dragged()))
        {
            sh.set_vol(((pt.x - slider.left()) / slider.width()).clamp(0.0, 1.0));
            sh.muted.store(false, Relaxed);
        }
        painter.text(
            volume_box.right_center() - vec2(10.0, 0.0),
            Align2::RIGHT_CENTER,
            format!("{}%", (vol * 100.0).round() as u32),
            FontId::proportional(10.5),
            Color32::from_gray(0xd8),
        );

        // Playlist and equalizer shortcuts.
        let shortcuts = control_rects[7];
        let gap = 8.0;
        let eq_w = 80.0;
        let playlist_rect = Rect::from_min_max(
            shortcuts.min,
            pos2(shortcuts.right() - eq_w - gap, shortcuts.bottom()),
        );
        let eq_rect = Rect::from_min_max(
            pos2(playlist_rect.right() + gap, shortcuts.top()),
            shortcuts.max,
        );
        let playlist_resp = ui
            .interact(playlist_rect, ui.id().with("mb-playlist"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        painter.rect_filled(
            playlist_rect,
            Rounding::same(11.0),
            if self.playlist_open {
                Color32::from_rgb(0x3b, 0x21, 0x26)
            } else {
                WELL
            },
        );
        painter.rect_stroke(
            playlist_rect,
            Rounding::same(11.0),
            Stroke::new(1.0, PILL_EDGE),
        );
        Self::glyph(
            &painter,
            pos2(playlist_rect.left() + 22.0, playlist_rect.center().y),
            "list",
            if self.playlist_open { RED } else { ICON },
        );
        painter.text(
            pos2(playlist_rect.left() + 38.0, playlist_rect.center().y),
            Align2::LEFT_CENTER,
            "Playlist",
            FontId::proportional(11.0),
            Color32::from_gray(0xe0),
        );
        painter.text(
            pos2(playlist_rect.right() - 12.0, playlist_rect.center().y),
            Align2::RIGHT_CENTER,
            queue.len().to_string(),
            FontId::proportional(10.0),
            Color32::from_gray(0x85),
        );
        if self.allow_clicks && playlist_resp.clicked() {
            self.playlist_open = !self.playlist_open;
            if self.playlist_open {
                self.settings_open = false;
                self.eq_open = false;
            }
        }
        let eq_resp = ui
            .interact(eq_rect, ui.id().with("mb-eq"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        painter.rect_filled(
            eq_rect,
            Rounding::same(11.0),
            if self.eq_open {
                Color32::from_rgb(0x3b, 0x21, 0x26)
            } else {
                WELL
            },
        );
        painter.rect_stroke(eq_rect, Rounding::same(11.0), Stroke::new(1.0, PILL_EDGE));
        Self::glyph(
            &painter,
            pos2(eq_rect.left() + 18.0, eq_rect.center().y),
            "eq",
            if self.settings.eq_on { RED } else { ICON },
        );
        painter.text(
            pos2(eq_rect.left() + 34.0, eq_rect.center().y),
            Align2::LEFT_CENTER,
            "EQ",
            FontId::proportional(11.0),
            Color32::from_gray(0xe0),
        );
        if self.allow_clicks && eq_resp.clicked() {
            self.eq_open = !self.eq_open;
            if self.eq_open {
                self.settings_open = false;
                self.playlist_open = false;
            }
        }

        // Compact footer: Settings, open files, open folder, and Quit.
        let sep_y = frame.bottom() - 43.0;
        painter.line_segment(
            [
                pos2(frame.left() + 15.0, sep_y),
                pos2(frame.right() - 15.0, sep_y),
            ],
            Stroke::new(1.0, PILL_EDGE),
        );
        let footer_y = frame.bottom() - 20.0;
        let footer = [
            ("mb-settings", "cog", "Settings"),
            ("mb-open-files", "file", "Open files…"),
            ("mb-open-folder", "fold", "Open folder…"),
            ("mb-quit", "pow", "Quit"),
        ];
        for (i, (id, glyph, tip)) in footer.iter().enumerate() {
            let center = pos2(
                frame.left() + frame.width() * (i as f32 + 0.5) / 4.0,
                footer_y,
            );
            let button = Rect::from_center_size(center, vec2(30.0, 28.0));
            let active = i == 0 && self.settings_open;
            if self.allow_clicks
                && Self::menu_bar_button(ui, button, id, glyph, active, false)
                    .on_hover_text(*tip)
                    .clicked()
            {
                match i {
                    0 => {
                        self.settings_open = !self.settings_open;
                        if self.settings_open {
                            self.playlist_open = false;
                            self.eq_open = false;
                        }
                    }
                    1 => {
                        if let Some(files) = rfd::FileDialog::new()
                            .add_filter("Audio", EXTS)
                            .pick_files()
                        {
                            self.open(files, true);
                        }
                    }
                    2 => {
                        if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                            self.open(vec![folder], true);
                        }
                    }
                    _ => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                }
            }
        }
    }

    /// Short, attached queue panel used by the Menu Bar size.
    fn draw_menu_bar_playlist(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let sh = self.engine.shared.clone();
        let queue = sh.queue_info.lock().unwrap().clone();
        let index = sh.current_idx.load(Relaxed) as usize;
        let outer = rect.shrink(6.0);
        painter.rect_filled(outer, Rounding::same(16.0), PILL);
        painter.rect_stroke(outer, Rounding::same(16.0), Stroke::new(1.0, PILL_EDGE));
        let list = outer.shrink2(vec2(8.0, 7.0));
        if queue.is_empty() {
            painter.text(
                list.center(),
                Align2::CENTER_CENTER,
                "Playlist is empty",
                FontId::proportional(11.0),
                Color32::from_gray(0x85),
            );
            return;
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(list)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(list);
        egui::ScrollArea::vertical()
            .id_source("menu-bar-playlist")
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                for (i, track) in queue.iter().enumerate() {
                    let (row, response) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                    if !ui.is_rect_visible(row) {
                        continue;
                    }
                    let active = i == index;
                    if active {
                        ui.painter().rect_filled(
                            row,
                            Rounding::same(8.0),
                            Color32::from_rgb(0x3b, 0x21, 0x26),
                        );
                    }
                    let text = fit(
                        ui.ctx(),
                        &track.title,
                        FontId::proportional(10.5),
                        row.width() - 68.0,
                    );
                    ui.painter().text(
                        pos2(row.left() + 8.0, row.center().y),
                        Align2::LEFT_CENTER,
                        text,
                        FontId::proportional(10.5),
                        if active {
                            Color32::from_gray(0xed)
                        } else {
                            Color32::from_gray(0xb0)
                        },
                    );
                    if !active {
                        ui.painter().text(
                            pos2(row.right() - 8.0, row.center().y),
                            Align2::RIGHT_CENTER,
                            fmt_time(track.duration_ms as f64 / 1000.0),
                            FontId::proportional(10.0),
                            Color32::from_gray(0xc3),
                        );
                    }
                    if active {
                        let x = row.right() - 13.0;
                        ui.painter().add(Shape::convex_polygon(
                            vec![
                                pos2(x - 3.0, row.center().y - 4.0),
                                pos2(x - 3.0, row.center().y + 4.0),
                                pos2(x + 4.0, row.center().y),
                            ],
                            RED,
                            Stroke::NONE,
                        ));
                    }
                    if self.allow_clicks && response.clicked() {
                        self.engine.select(i);
                    }
                }
            });
    }

    fn draw_playlist(&mut self, ui: &mut egui::Ui) {
        if self.menu_bar_mode() {
            self.draw_menu_bar_playlist(ui);
            return;
        }
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
        if self.allow_clicks && clear_resp.clicked() {
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
        if self.allow_clicks && close_resp.clicked() {
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
                    if self.allow_clicks && row.1.clicked() {
                        self.engine.select(n);
                    }
                }
            });
    }
}

impl Haven {
    /// Whole-row switch: label on the left, pill switch on the right.
    fn switch_row(
        ui: &mut egui::Ui,
        row: Rect,
        id: &str,
        label: &str,
        on: bool,
        allow_clicks: bool,
    ) -> bool {
        let resp = ui
            .interact(row, ui.id().with(id), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter();
        p.text(
            row.left_center(),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.0),
            if resp.hovered() {
                Color32::WHITE
            } else {
                Color32::from_gray(0xe0)
            },
        );
        let track = Rect::from_center_size(row.right_center() - vec2(17.0, 0.0), vec2(34.0, 18.0));
        p.rect_filled(
            track,
            Rounding::same(9.0),
            if on {
                RED
            } else {
                Color32::from_rgb(0x3a, 0x3b, 0x3e)
            },
        );
        let kx = if on {
            track.right() - 9.0
        } else {
            track.left() + 9.0
        };
        p.circle_filled(
            pos2(kx, track.center().y),
            7.0,
            if on {
                Color32::WHITE
            } else {
                Color32::from_gray(0x9a)
            },
        );
        allow_clicks && resp.clicked()
    }

    /// Small text button used in the settings drawer.
    fn pill_button(
        ui: &mut egui::Ui,
        rect: Rect,
        id: &str,
        text: &str,
        primary: bool,
        enabled: bool,
        allow_clicks: bool,
    ) -> bool {
        let resp = ui.interact(rect, ui.id().with(id), Sense::click());
        let resp = if enabled {
            resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp
        };
        let (fill, ink) = if !enabled {
            (
                Color32::from_rgb(0x1f, 0x20, 0x22),
                Color32::from_gray(0x6a),
            )
        } else if primary {
            (
                if resp.hovered() {
                    Color32::from_rgb(0xe0, 0x5c, 0x6a)
                } else {
                    RED
                },
                Color32::WHITE,
            )
        } else if resp.hovered() {
            (Color32::from_rgb(0x3a, 0x3b, 0x3e), Color32::WHITE)
        } else {
            (
                Color32::from_rgb(0x2f, 0x30, 0x33),
                Color32::from_gray(0xe0),
            )
        };
        ui.painter().rect_filled(rect, Rounding::same(10.0), fill);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(12.0),
            ink,
        );
        enabled && allow_clicks && resp.clicked()
    }

    /// Tiny drawing of each waveform style for the picker tiles.
    fn wave_thumb(p: &egui::Painter, r: Rect, style: u8) {
        const H: [f32; 7] = [0.4, 0.8, 0.55, 1.0, 0.5, 0.75, 0.35];
        let ink = Color32::from_gray(0xd8);
        let aw = r.width() * 0.62;
        let left = r.center().x - aw / 2.0;
        let maxh = r.height() * 0.58;
        let cy = r.center().y;
        match style {
            0 => {
                for (i, v) in H.iter().enumerate() {
                    let x = left + (i as f32 + 0.5) * aw / 7.0;
                    p.rect_filled(
                        Rect::from_center_size(pos2(x, cy), vec2(3.0, v * maxh)),
                        1.5,
                        ink,
                    );
                }
            }
            1 => {
                for i in 0..12 {
                    let v = 0.25 + 0.75 * (i as f32 * 1.7).sin().abs();
                    let x = left + (i as f32 + 0.5) * aw / 12.0;
                    p.rect_filled(
                        Rect::from_center_size(pos2(x, cy), vec2(1.2, v * maxh)),
                        0.6,
                        ink,
                    );
                }
            }
            2 => {
                let base = cy + maxh * 0.18;
                for (i, v) in H.iter().enumerate() {
                    let x = left + (i as f32 + 0.5) * aw / 7.0;
                    let hh = v * maxh * 0.78;
                    p.rect_filled(
                        Rect::from_min_max(pos2(x - 1.5, base - hh), pos2(x + 1.5, base)),
                        Rounding {
                            nw: 1.5,
                            ne: 1.5,
                            sw: 0.0,
                            se: 0.0,
                        },
                        ink,
                    );
                    p.rect_filled(
                        Rect::from_min_max(
                            pos2(x - 1.5, base + 1.5),
                            pos2(x + 1.5, base + 1.5 + hh * 0.34),
                        ),
                        0.0,
                        ink.gamma_multiply(0.22),
                    );
                }
            }
            3 => {
                for (i, v) in H.iter().enumerate() {
                    let x = left + (i as f32 + 0.5) * aw / 7.0;
                    for k in 0..((v * 4.0).round() as i32) {
                        for sg in [-1.0f32, 1.0] {
                            p.rect_filled(
                                Rect::from_center_size(
                                    pos2(x, cy + sg * k as f32 * 3.4),
                                    vec2(3.0, 2.0),
                                ),
                                0.8,
                                ink,
                            );
                        }
                    }
                }
            }
            _ => {
                let pts: Vec<egui::Pos2> = (0..=24)
                    .map(|i| {
                        let t = i as f32 / 24.0;
                        pos2(
                            left + t * aw,
                            cy - (t * std::f32::consts::TAU * 2.5).sin() * maxh * 0.45,
                        )
                    })
                    .collect();
                p.add(Shape::line(pts, Stroke::new(1.4, ink)));
            }
        }
    }

    /// Settings drawer: docks under the player exactly like the playlist.
    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let outer = Rounding::same(22.0);
        painter.rect_filled(rect.shrink(8.0), outer, PILL);
        painter.rect_stroke(rect.shrink(8.0), outer, Stroke::new(1.0, PILL_EDGE));
        let (left, right) = (rect.left() + 27.0, rect.right() - 27.0);

        let header = Rect::from_min_max(
            pos2(left, rect.top() + 13.0),
            pos2(right, rect.top() + 43.0),
        );
        painter.text(
            header.left_center(),
            Align2::LEFT_CENTER,
            "Settings",
            FontId::proportional(16.0),
            Color32::from_gray(0xe6),
        );
        let close = Rect::from_center_size(
            pos2(header.right() - 8.0, header.center().y),
            vec2(31.0, 31.0),
        );
        let close_resp = ui
            .interact(close, ui.id().with("settings-close"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let (fill, ink) = if close_resp.hovered() {
            (Color32::from_rgb(0x43, 0x23, 0x2a), RED)
        } else {
            (
                Color32::from_rgb(0x18, 0x19, 0x1a),
                Color32::from_gray(0xc8),
            )
        };
        painter.rect_filled(close, Rounding::same(10.0), fill);
        painter.rect_stroke(close, Rounding::same(10.0), Stroke::new(1.0, PILL_EDGE));
        for (a, b) in [((-4.0, -4.0), (4.0, 4.0)), ((4.0, -4.0), (-4.0, 4.0))] {
            painter.line_segment(
                [
                    close.center() + vec2(a.0, a.1),
                    close.center() + vec2(b.0, b.1),
                ],
                Stroke::new(1.8, ink),
            );
        }
        if self.allow_clicks && close_resp.clicked() {
            self.settings_open = false; // window shrinks itself (sync_layout)
        }

        let label = |y: f32, text: &str| {
            painter.text(
                pos2(left, y),
                Align2::LEFT_TOP,
                text,
                FontId::proportional(10.0),
                Color32::from_gray(0x82),
            );
        };
        let row_h = 34.0;
        let mut y = rect.top() + 54.0;

        label(y, "WINDOW");
        y += 15.0;
        let r1 = Rect::from_min_size(pos2(left, y), vec2(right - left, row_h));
        if Self::switch_row(
            ui,
            r1,
            "on-top",
            "Always on top",
            self.settings.on_top,
            self.allow_clicks,
        ) {
            self.settings.on_top = !self.settings.on_top;
            self.settings.save();
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                    if self.settings.on_top {
                        egui::WindowLevel::AlwaysOnTop
                    } else {
                        egui::WindowLevel::Normal
                    },
                ));
        }
        y += row_h;
        let r2 = Rect::from_min_size(pos2(left, y), vec2(right - left, row_h));
        if Self::switch_row(
            ui,
            r2,
            "lock",
            "Lock position",
            self.settings.locked,
            self.allow_clicks,
        ) {
            self.settings.locked = !self.settings.locked;
            self.settings.save();
        }
        y += row_h + 6.0;

        label(y, "APPEARANCE");
        y += 15.0;
        let r3 = Rect::from_min_size(pos2(left, y), vec2(right - left, row_h));
        painter.text(
            r3.left_center(),
            Align2::LEFT_CENTER,
            "Size",
            FontId::proportional(13.0),
            Color32::from_gray(0xe0),
        );
        #[cfg(target_os = "macos")]
        let size_options = [
            (4u8, "X-Small"),
            (0, "Small"),
            (1, "Med"),
            (2, "Big"),
            (3, "Menu Bar"),
        ];
        #[cfg(not(target_os = "macos"))]
        let size_options = [(4u8, "X-Small"), (0, "Small"), (1, "Med"), (2, "Big")];
        let cell_w = if cfg!(target_os = "macos") {
            54.0
        } else {
            51.0
        };
        let seg_w = cell_w * size_options.len() as f32 + 4.0;
        let seg = Rect::from_min_size(pos2(right - seg_w, r3.center().y - 15.0), vec2(seg_w, 30.0));
        painter.rect_filled(seg, Rounding::same(10.0), WELL);
        for (slot, &(n, name)) in size_options.iter().enumerate() {
            let cell = Rect::from_min_size(
                pos2(seg.left() + 2.0 + slot as f32 * cell_w, seg.top() + 2.0),
                vec2(cell_w, 26.0),
            );
            let resp = ui
                .interact(cell, ui.id().with(("size", n)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let active = self.settings.size == n;
            if active {
                painter.rect_filled(
                    cell,
                    Rounding::same(8.0),
                    Color32::from_rgb(0x3a, 0x3b, 0x3e),
                );
            }
            painter.text(
                cell.center(),
                Align2::CENTER_CENTER,
                name,
                FontId::proportional(11.0),
                if active {
                    Color32::WHITE
                } else if resp.hovered() {
                    Color32::from_gray(0xc0)
                } else {
                    Color32::from_gray(0x8d)
                },
            );
            if self.allow_clicks && resp.clicked() && !active {
                #[cfg(target_os = "macos")]
                let was_menu_bar = self.menu_bar_mode();
                #[cfg(target_os = "macos")]
                let old_size = self.settings.size;
                self.settings.size = n;
                #[cfg(target_os = "macos")]
                if was_menu_bar != (n == 3) {
                    let changed = crate::macos_open::set_menu_bar_mode(n == 3);
                    if !changed {
                        self.settings.size = old_size;
                    } else if n == 3 {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    } else {
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
                self.settings.save();
            }
        }
        y += row_h;
        let r_eq = Rect::from_min_size(pos2(left, y), vec2(right - left, row_h));
        let eq_resp = ui
            .interact(r_eq, ui.id().with("eq-row"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        painter.text(
            r_eq.left_center(),
            Align2::LEFT_CENTER,
            "Equalizer",
            FontId::proportional(13.0),
            if eq_resp.hovered() {
                Color32::WHITE
            } else {
                Color32::from_gray(0xe0)
            },
        );
        let chev = r_eq.right_center() - vec2(8.0, 0.0);
        let chev_ink = if eq_resp.hovered() {
            Color32::WHITE
        } else {
            Color32::from_gray(0x9a)
        };
        painter.line_segment(
            [chev + vec2(-3.0, -5.0), chev + vec2(2.0, 0.0)],
            Stroke::new(1.6, chev_ink),
        );
        painter.line_segment(
            [chev + vec2(2.0, 0.0), chev + vec2(-3.0, 5.0)],
            Stroke::new(1.6, chev_ink),
        );
        painter.text(
            chev - vec2(14.0, 0.0),
            Align2::RIGHT_CENTER,
            if self.settings.eq_on { "On" } else { "Off" },
            FontId::proportional(12.0),
            if self.settings.eq_on {
                RED
            } else {
                Color32::from_gray(0x8d)
            },
        );
        if self.allow_clicks && eq_resp.clicked() {
            self.settings_open = false;
            self.eq_open = true; // same slot, the player does not move
        }
        y += row_h + 6.0;

        label(y, "WAVEFORM");
        y += 15.0;
        let gap = 6.0;
        let tw = ((right - left) - gap * 4.0) / 5.0;
        for n in 0..5u8 {
            let tile = Rect::from_min_size(pos2(left + n as f32 * (tw + gap), y), vec2(tw, 38.0));
            let resp = ui
                .interact(tile, ui.id().with(("wave", n)), Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            painter.rect_filled(tile, Rounding::same(10.0), WELL);
            if self.settings.wave == n {
                painter.rect_stroke(tile, Rounding::same(10.0), Stroke::new(1.5, RED));
            } else if resp.hovered() {
                painter.rect_stroke(tile, Rounding::same(10.0), Stroke::new(1.0, PILL_EDGE));
            }
            Self::wave_thumb(&painter, tile, n);
            if self.allow_clicks && resp.clicked() {
                self.settings.wave = n;
                self.settings.save();
            }
        }
        y += 38.0 + 8.0;

        use crate::update::{State, Updater};
        label(y, "UPDATES");
        y += 15.0;
        let r4 = Rect::from_min_size(pos2(left, y), vec2(right - left, row_h));
        let st = self.updater.snapshot();
        let (text, hot, btn, primary, enabled): (String, bool, &str, bool, bool) = match &st {
            State::Idle => (
                format!("Haven v{}", Updater::current()),
                false,
                "Check",
                false,
                true,
            ),
            State::Checking => ("Checking for updates…".into(), false, "…", false, false),
            State::UpToDate => (
                format!("v{} · up to date", Updater::current()),
                false,
                "Check",
                false,
                true,
            ),
            State::Available(r) => (
                format!("Version {} is available", r.version),
                true,
                if r.asset_url.is_some() {
                    "Update"
                } else {
                    "Open page"
                },
                true,
                true,
            ),
            State::Downloading(_, d, t) => (
                if *t > 0 {
                    format!("Downloading… {}%", (d * 100 / t).min(100))
                } else {
                    "Downloading…".into()
                },
                false,
                "…",
                false,
                false,
            ),
            State::Installing => (
                "Installing — Haven restarts by itself…".into(),
                true,
                "…",
                false,
                false,
            ),
            State::Downloaded(_, reason) => (
                format!("Automatic install unavailable: {reason}"),
                true,
                "Open",
                false,
                true,
            ),
            State::Failed(msg, Some(_)) => {
                (format!("Update failed: {msg}"), false, "Retry", false, true)
            }
            State::Failed(msg, None) => (
                format!("Couldn't check: {msg}"),
                false,
                "Retry",
                false,
                true,
            ),
        };
        painter.text(
            r4.left_center(),
            Align2::LEFT_CENTER,
            fit(
                ui.ctx(),
                &text,
                FontId::proportional(12.0),
                r4.width() - 108.0,
            ),
            FontId::proportional(12.0),
            if hot { RED } else { Color32::from_gray(0xb4) },
        );
        let br = Rect::from_center_size(pos2(r4.right() - 48.0, r4.center().y), vec2(96.0, 28.0));
        if Self::pill_button(
            ui,
            br,
            "update-btn",
            btn,
            primary,
            enabled,
            self.allow_clicks,
        ) {
            match &st {
                State::Available(r) => self.updater.download(r.clone()),
                State::Failed(_, Some(r)) => self.updater.download(r.clone()),
                State::Downloaded(p, _) => crate::update::open_path_or_url(&p.to_string_lossy()),
                _ => self.updater.check(),
            }
        }

        // signature
        painter.text(
            pos2(rect.center().x, rect.bottom() - 21.0),
            Align2::CENTER_CENTER,
            "WTMLJR",
            FontId::proportional(10.0),
            Color32::from_gray(0x5e),
        );
    }

    /// Sends the equalizer state to the audio thread and schedules a save.
    fn push_eq(&mut self) {
        self.engine
            .shared
            .set_eq(self.settings.eq_on, self.settings.eq);
        self.eq_dirty = Some(Instant::now());
    }

    /// Equalizer drawer: ten faders (+/-12 dB), docked exactly like Settings.
    fn draw_eq(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let outer = Rounding::same(22.0);
        painter.rect_filled(rect.shrink(8.0), outer, PILL);
        painter.rect_stroke(rect.shrink(8.0), outer, Stroke::new(1.0, PILL_EDGE));
        let (left, right) = (rect.left() + 27.0, rect.right() - 27.0);

        let header = Rect::from_min_max(
            pos2(left, rect.top() + 13.0),
            pos2(right, rect.top() + 43.0),
        );
        painter.text(
            header.left_center(),
            Align2::LEFT_CENTER,
            "Equalizer",
            FontId::proportional(16.0),
            Color32::from_gray(0xe6),
        );
        // close: back to Settings
        let close = Rect::from_center_size(
            pos2(header.right() - 8.0, header.center().y),
            vec2(31.0, 31.0),
        );
        let close_resp = ui
            .interact(close, ui.id().with("eq-close"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        let (fill, ink) = if close_resp.hovered() {
            (Color32::from_rgb(0x43, 0x23, 0x2a), RED)
        } else {
            (
                Color32::from_rgb(0x18, 0x19, 0x1a),
                Color32::from_gray(0xc8),
            )
        };
        painter.rect_filled(close, Rounding::same(10.0), fill);
        painter.rect_stroke(close, Rounding::same(10.0), Stroke::new(1.0, PILL_EDGE));
        for (a, b) in [((-4.0, -4.0), (4.0, 4.0)), ((4.0, -4.0), (-4.0, 4.0))] {
            painter.line_segment(
                [
                    close.center() + vec2(a.0, a.1),
                    close.center() + vec2(b.0, b.1),
                ],
                Stroke::new(1.8, ink),
            );
        }
        if self.allow_clicks && close_resp.clicked() {
            self.eq_open = false;
            self.settings_open = true;
        }
        // on / off switch
        let on = self.settings.eq_on;
        let track = Rect::from_center_size(
            pos2(close.left() - 12.0 - 17.0, header.center().y),
            vec2(34.0, 18.0),
        );
        let sresp = ui
            .interact(
                track.expand2(vec2(6.0, 8.0)),
                ui.id().with("eq-onoff"),
                Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        painter.rect_filled(
            track,
            Rounding::same(9.0),
            if on {
                RED
            } else {
                Color32::from_rgb(0x3a, 0x3b, 0x3e)
            },
        );
        let kx = if on {
            track.right() - 9.0
        } else {
            track.left() + 9.0
        };
        painter.circle_filled(
            pos2(kx, track.center().y),
            7.0,
            if on {
                Color32::WHITE
            } else {
                Color32::from_gray(0x9a)
            },
        );
        if self.allow_clicks && sresp.clicked() {
            self.settings.eq_on = !self.settings.eq_on;
            self.push_eq();
        }

        // presets
        let presets: [(&str, [f32; 10]); 4] = [
            ("Flat", [0.0; 10]),
            ("Bass", [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
            (
                "Vocal",
                [-2.0, -2.0, -1.0, 0.0, 2.0, 3.0, 3.0, 2.0, 0.0, -1.0],
            ),
            ("Treble", [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0]),
        ];
        let cur = self.settings.eq;
        let matched = presets
            .iter()
            .position(|(_, g)| g.iter().zip(cur.iter()).all(|(a, b)| (a - b).abs() < 0.05));
        let gap = 6.0;
        let cw = ((right - left) - gap * 4.0) / 5.0;
        for n in 0..5usize {
            let chip = Rect::from_min_size(
                pos2(left + n as f32 * (cw + gap), rect.top() + 52.0),
                vec2(cw, 26.0),
            );
            let resp = ui.interact(chip, ui.id().with(("eq-preset", n)), Sense::click());
            let resp = if n < 4 {
                resp.on_hover_cursor(egui::CursorIcon::PointingHand)
            } else {
                resp
            };
            let (name, active) = if n < 4 {
                (presets[n].0, matched == Some(n))
            } else {
                ("Custom", matched.is_none())
            };
            painter.rect_filled(
                chip,
                Rounding::same(13.0),
                Color32::from_rgb(0x20, 0x21, 0x23),
            );
            painter.rect_stroke(
                chip,
                Rounding::same(13.0),
                Stroke::new(1.0, if active { RED } else { PILL_EDGE }),
            );
            painter.text(
                chip.center(),
                Align2::CENTER_CENTER,
                name,
                FontId::proportional(11.0),
                if active {
                    Color32::WHITE
                } else {
                    Color32::from_gray(0xc0)
                },
            );
            if n < 4 && self.allow_clicks && resp.clicked() {
                self.settings.eq = presets[n].1;
                if n > 0 {
                    self.settings.eq_on = true;
                }
                self.push_eq();
            }
        }

        // faders
        let well = Rect::from_min_max(
            pos2(left, rect.top() + 88.0),
            pos2(right, rect.top() + 296.0),
        );
        painter.rect_filled(well, Rounding::same(14.0), WELL);
        painter.rect_stroke(well, Rounding::same(14.0), Stroke::new(1.0, WELL_EDGE));
        let (tt, tb) = (well.top() + 28.0, well.bottom() - 28.0);
        let mid = (tt + tb) / 2.0;
        let half = (tb - tt) / 2.0;
        painter.line_segment(
            [
                pos2(well.left() + 10.0, mid),
                pos2(well.right() - 10.0, mid),
            ],
            Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3b, 0x3e)),
        );
        let on = self.settings.eq_on;
        let dim = if on { 1.0 } else { 0.45 };
        let short = [
            "32", "64", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
        ];
        let long = [
            "32 Hz", "64 Hz", "125 Hz", "250 Hz", "500 Hz", "1 kHz", "2 kHz", "4 kHz", "8 kHz",
            "16 kHz",
        ];
        let col_w = (well.width() - 16.0) / 10.0;
        let mut readout: Option<usize> = None;
        for i in 0..10usize {
            let cx = well.left() + 8.0 + (i as f32 + 0.5) * col_w;
            let col = Rect::from_min_max(
                pos2(cx - col_w / 2.0, tt - 12.0),
                pos2(cx + col_w / 2.0, tb + 12.0),
            );
            let resp = ui
                .interact(col, ui.id().with(("eq-band", i)), Sense::click_and_drag())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let mut g = self.settings.eq[i];
            if self.allow_clicks && (resp.dragged() || resp.clicked()) {
                if let Some(pt) = resp.interact_pointer_pos() {
                    let v = ((mid - pt.y) / half * 12.0).clamp(-12.0, 12.0);
                    let v = (v * 2.0).round() / 2.0; // half-dB steps
                    g = if v.abs() < 0.5 { 0.0 } else { v };
                }
            }
            if self.allow_clicks && resp.double_clicked() {
                g = 0.0;
            }
            if resp.hovered() {
                let dy = ui.ctx().input(|inp| inp.smooth_scroll_delta.y);
                if dy.abs() > 0.01 {
                    g = (((g + dy * 0.012).clamp(-12.0, 12.0)) * 10.0).round() / 10.0;
                }
            }
            if (g - self.settings.eq[i]).abs() > 1e-4 {
                self.settings.eq[i] = g;
                self.push_eq();
            }
            if resp.hovered() || (self.allow_clicks && resp.dragged()) {
                readout = Some(i);
            }
            let g = self.settings.eq[i];
            let y = mid - g / 12.0 * half;
            painter.line_segment(
                [pos2(cx, tt), pos2(cx, tb)],
                Stroke::new(2.0, Color32::from_rgb(0x3a, 0x3b, 0x3e)),
            );
            painter.line_segment(
                [pos2(cx, mid), pos2(cx, y)],
                Stroke::new(2.0, RED.gamma_multiply(dim)),
            );
            painter.circle(
                pos2(cx, y),
                7.0,
                Color32::from_gray(0xe4).gamma_multiply(dim),
                Stroke::new(2.0, WELL),
            );
            painter.text(
                pos2(cx, well.bottom() - 13.0),
                Align2::CENTER_CENTER,
                short[i],
                FontId::proportional(9.5),
                Color32::from_gray(0x9a),
            );
        }
        let (txt, ink) = match readout {
            Some(i) => (
                format!("{}   {:+.1} dB", long[i], self.settings.eq[i]),
                Color32::from_gray(0xe0),
            ),
            None => (
                "\u{b1}12 dB \u{b7} double-click resets a band".to_string(),
                Color32::from_gray(0x82),
            ),
        };
        painter.text(
            pos2(well.center().x, well.top() + 14.0),
            Align2::CENTER_CENTER,
            txt,
            FontId::proportional(10.5),
            ink,
        );
    }

    /// Height of the docked Playlist / Settings / Equalizer panel including its gap (0 if none).
    fn panel_total(&self) -> f32 {
        if self.playlist_open {
            self.playlist_height() + PLAYLIST_GAP
        } else if self.settings_open {
            SETTINGS_HEIGHT + PLAYLIST_GAP
        } else if self.eq_open {
            EQ_HEIGHT + PLAYLIST_GAP
        } else {
            0.0
        }
    }

    /// The hover card sits above the player, or above the panel when the panel
    /// is docked upward, so it never covers (or hangs off the screen below) them.
    fn card_rect(&self, player: Rect, dims: [f32; 2], base_h: f32) -> Rect {
        let inset = dims[1] * 0.124; // same side inset as the pill
        let (l, r) = (player.left() + inset, player.right() - inset);
        if self.card_above {
            let up = if self.panel_up {
                self.panel_total()
            } else {
                0.0
            };
            let bottom = player.top() - up - 2.0;
            Rect::from_min_max(pos2(l, bottom - HOVER_CARD_HEIGHT), pos2(r, bottom))
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
            let (outer, monitor) = ctx.input(|i| {
                let v = i.viewport();
                (v.outer_rect, v.monitor_size)
            });
            // Prefer above the player (above the panel when it is docked upward);
            // go below only if there is room on screen, otherwise skip the card.
            let up = if self.panel_up {
                self.panel_total()
            } else {
                0.0
            };
            let ptop = outer.map(|o| o.min.y + self.top_pad);
            // macOS keeps its card room below the player (see `card_reserve`).
            let room_above = !cfg!(target_os = "macos")
                && ptop.map_or(true, |y| y - up >= CARD_SLOT + 8.0);
            let room_below = match (ptop, monitor) {
                (Some(y), Some(m)) => {
                    let below_panel = if self.panel_up {
                        0.0
                    } else {
                        self.panel_total()
                    };
                    m.y - (y + self.settings.dims()[1] + below_panel) - SCREEN_MARGIN >= CARD_SLOT
                }
                _ => true,
            };
            if !room_above && !room_below {
                return;
            }
            self.card_above = room_above;
            self.card_visible = true;
            self.card_last = Some(rel);
            self.card_hide_at = None;
        }
    }

    /// Sizes the OS window for what is shown (card, upward/downward menu,
    /// playlist). Growing upward also moves the window up by the same amount and
    /// the player is drawn lower by it, so the player never moves on screen.
    /// Player and docked-panel rectangles for the current content offset.
    fn rects(&self, area: Rect, dims: [f32; 2], panel_hh: f32) -> (Rect, Rect) {
        let player = Rect::from_min_size(
            pos2(area.left(), area.top() + self.top_pad),
            vec2(dims[0], dims[1]),
        );
        let panel = Rect::from_min_size(
            pos2(
                area.left(),
                if self.panel_up {
                    player.top() - PLAYLIST_GAP - panel_hh
                } else {
                    player.bottom() + PLAYLIST_GAP
                },
            ),
            vec2(self.panel_width(dims), panel_hh),
        );
        (player, panel)
    }

    /// Chooses once, when the panel opens, whether it docks below or above the
    /// player: below if it fits on screen, otherwise above.
    fn decide_panel_up(&self, ctx: &egui::Context, panel_total: f32, dims: [f32; 2]) -> bool {
        let (outer, monitor) = ctx.input(|i| {
            let v = i.viewport();
            (v.outer_rect, v.monitor_size)
        });
        // `anchor` is the player's reliable screen position, even mid-resize.
        let ptop = self
            .anchor
            .map(|a| a.y)
            .or_else(|| outer.map(|o| o.min.y + self.top_pad));
        match (ptop, monitor) {
            (Some(ptop), Some(m)) => {
                let below = m.y - (ptop + dims[1]) - SCREEN_MARGIN;
                let above = ptop - 8.0;
                below < panel_total && above > below
            }
            _ => false,
        }
    }

    /// Sizes and positions the OS window for what is shown (card, menu, panel).
    ///
    /// It runs BEFORE drawing, and every change of the content offset is made in
    /// the same frame as the window move, so the player never appears displaced:
    ///  * growing upward: first enlarge the window downward (invisible), then move
    ///    it up and shift the content together;
    ///  * shrinking: move the window down and shift the content together first,
    ///    and only then cut the transparent leftover.
    fn sync_layout(&mut self, ctx: &egui::Context, dims: [f32; 2], panel_total: f32) {
        if ctx.input(|i| i.viewport().minimized.unwrap_or(false)) {
            return; // never resize or move a minimized window
        }
        // Height from the player's top down; a panel docked above adds to `top`.
        let base_h = dims[1] + if self.panel_up { 0.0 } else { panel_total };
        let reserve = self.card_reserve();
        let (mut top, mut bottom) = (if self.panel_up { panel_total } else { 0.0 }, reserve);
        if reserve == 0.0 && self.card_visible {
            if self.card_above {
                top += CARD_SLOT; // above the player, or above an upward panel
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
        let width = if panel_total > 0.0 {
            self.panel_width(dims)
        } else {
            dims[0]
        };
        let want = [top, width, top + base_h + bottom];
        self.want_top = want[0];
        if want
            .iter()
            .zip(self.req.iter())
            .all(|(a, b)| (a - b).abs() <= 0.5)
        {
            return;
        }
        // Menu / panel changes go through at once; only the cosmetic hover card
        // waits for the window to settle.
        let urgent =
            self.menu_rel.is_some() || self.menu_applied || panel_total > 0.0 || self.panel_applied;
        if !urgent && (self.layout_busy() || self.pos_changed.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(120));
            return;
        }
        // Absolute target from the remembered player position, so a stale
        // outer_rect can never make the window drift.
        let anchor = self.anchor.or_else(|| {
            ctx.input(|i| i.viewport().outer_rect)
                .map(|o| pos2(o.min.x, o.min.y + self.top_pad))
        });
        let Some(anchor) = anchor else {
            return;
        };
        self.menu_applied = self.menu_rel.is_some();
        self.panel_applied = panel_total > 0.0;
        let top_delta = want[0] - self.req[0];

        if top_delta > 0.5 {
            // Phase 1: make the window tall enough first (no visible change).
            if (self.req[1] - want[1]).abs() > 0.5 || (self.req[2] - want[2]).abs() > 0.5 {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(want[1], want[2])));
                self.req[1] = want[1];
                self.req[2] = want[2];
                self.grow_sent = Some(Instant::now());
                ctx.request_repaint_after(Duration::from_millis(16));
                return;
            }
            let big_enough = ctx
                .input(|i| i.viewport().inner_rect)
                .map_or(true, |r| r.height() >= want[2] - 1.5);
            let waited = self
                .grow_sent
                .map_or(true, |t| t.elapsed() > Duration::from_millis(300));
            if !(big_enough || waited) {
                ctx.request_repaint_after(Duration::from_millis(16));
                return;
            }
            // Phase 2: shift the content and move the window in this same frame.
            self.top_pad = want[0];
            self.req[0] = want[0];
            self.grow_sent = None;
            let target = pos2(anchor.x, anchor.y - want[0]);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(target));
            self.target_outer = Some(target);
        } else if top_delta < -0.5 {
            // Phase A: shift the content and move the window down together; the
            // window stays tall for now and shrinks on a following frame.
            self.top_pad = want[0];
            self.req[0] = want[0];
            let target = pos2(anchor.x, anchor.y - want[0]);
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(target));
            self.target_outer = Some(target);
        } else {
            // Size only (bottom room, width): the top-left stays put, nothing jumps.
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(want[1], want[2])));
            self.req[1] = want[1];
            self.req[2] = want[2];
            self.target_outer = None;
        }
        self.layout_until = Some(Instant::now() + Duration::from_millis(250));
        ctx.request_repaint_after(Duration::from_millis(30));
    }

    /// The menu is only drawn once the window has really grown (or after a short
    /// timeout), so it never appears squeezed against the old window border.
    fn menu_ready(&self, ctx: &egui::Context) -> bool {
        let Some(t) = self.menu_opened_at else {
            return true;
        };
        if t.elapsed() > Duration::from_millis(400) {
            return true;
        }
        ctx.input(|i| {
            let v = i.viewport();
            let size_ok = v.inner_rect.map_or(true, |r| {
                (r.height() - self.req[2]).abs() < 2.0 && (r.width() - self.req[1]).abs() < 2.0
            });
            let pos_ok = match (self.target_outer, v.outer_rect) {
                (Some(t), Some(o)) => (o.min - t).length() < 2.0,
                _ => true,
            };
            size_ok && pos_ok && (self.req[0] - self.want_top).abs() < 0.5
        })
    }
}

impl eframe::App for Haven {
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.pos_changed.is_some() || self.vol_dirty.is_some() || self.eq_dirty.is_some() {
            self.settings.save();
        }
    }

    fn clear_color(&self, _v: &egui::Visuals) -> [f32; 4] {
        egui::Rgba::TRANSPARENT.to_array()
    }

    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        let focused_now = ctx.input(|i| i.focused);
        // The OS may deliver the activation click to a previously unfocused
        // overlay. Require a focused frame before we accept any control click.
        self.allow_clicks = self.focused_last_frame && focused_now;
        self.focused_last_frame = focused_now;

        let panel_open = self.playlist_open || self.settings_open || self.eq_open;
        if panel_open && !focused_now {
            let since = self.panel_unfocused_at.get_or_insert_with(Instant::now);
            let elapsed = since.elapsed();
            if elapsed >= Duration::from_secs(10) {
                self.playlist_open = false;
                self.settings_open = false;
                self.eq_open = false;
                self.panel_unfocused_at = None;
            } else {
                ctx.request_repaint_after(Duration::from_secs(10) - elapsed);
            }
        } else {
            self.panel_unfocused_at = None;
        }
        self.sync_track(ctx);
        self.keys(ctx);
        self.track_position(ctx);
        if self.layout_until.map_or(false, |t| Instant::now() >= t) {
            self.layout_until = None;
        }
        let dims = Settings::dims_for(self.settings.size);
        let panel_h = if self.playlist_open {
            Some(self.playlist_height())
        } else if self.settings_open {
            Some(SETTINGS_HEIGHT)
        } else if self.eq_open {
            Some(EQ_HEIGHT)
        } else {
            None
        };
        let panel_total = panel_h.map_or(0.0, |h| h + PLAYLIST_GAP);
        if panel_h.is_some() && !self.panel_was_open {
            self.panel_up = self.decide_panel_up(ctx, panel_total, dims);
        }
        self.panel_was_open = panel_h.is_some();
        let panel_up_h = if self.panel_up { panel_total } else { 0.0 };
        // height from the player's top downwards (a panel above is not part of it)
        let base_h = dims[1] + if self.panel_up { 0.0 } else { panel_total };
        let area = ctx.screen_rect();
        let panel_hh = panel_h.unwrap_or_else(|| self.playlist_height());
        let (player_rect, playlist_rect) = self.rects(area, dims, panel_hh);

        // Right-press opens the context menu where the pointer is.
        let mut opened_now = false;
        let secondary = ctx.input(|i| {
            if self.allow_clicks && i.pointer.button_pressed(egui::PointerButton::Secondary) {
                i.pointer.interact_pos()
            } else {
                None
            }
        });
        if let Some(pt) = secondary {
            if player_rect.contains(pt) || (panel_h.is_some() && playlist_rect.contains(pt)) {
                self.open_menu(ctx, pt, player_rect.min, dims[0]);
                opened_now = true;
            }
        }
        // Losing focus closes the menu immediately. The first 250 ms are ignored
        // because some systems report "unfocused" for a moment after the click.
        if self.menu_rel.is_some()
            && !opened_now
            && self
                .menu_opened_at
                .map_or(true, |t| t.elapsed() > Duration::from_millis(250))
            && !ctx.input(|i| i.focused)
        {
            self.close_menu();
        }

        let card_rect = self.card_rect(player_rect, dims, base_h);
        if self.menu_bar_mode() {
            self.card_visible = false;
            self.card_hide_at = None;
        } else {
            self.update_card(ctx, player_rect, card_rect);
        }
        let show_card = self.card_visible && self.menu_rel.is_none();
        let card_info = if show_card { self.info.clone() } else { None };

        // Window move/resize happens here, BEFORE drawing, so the content offset
        // and the window geometry change in the same frame (no one-frame jump).
        self.sync_layout(ctx, dims, panel_total);
        let (player_rect, playlist_rect) = self.rects(area, dims, panel_hh);
        let card_rect = self.card_rect(player_rect, dims, base_h);

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
                } else if self.settings_open {
                    let mut settings_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(playlist_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    self.draw_settings(&mut settings_ui);
                } else if self.eq_open {
                    let mut eq_ui = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(playlist_rect)
                            .layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    self.draw_eq(&mut eq_ui);
                }
                if let Some(info) = &card_info {
                    Self::paint_card(ui, card_rect, info);
                }
            });

        // Context menu: its own area so it can open up or down at the click.
        if let Some(rel) = self.menu_rel {
            if !self.menu_ready(ctx) {
                self.menu_rect = None;
                ctx.request_repaint_after(Duration::from_millis(16));
            } else {
                // Top-left is computed here (no pivot) because the size is fixed.
                let x = player_rect.left() + rel.x - if self.menu_right { MENU_WIDTH } else { 0.0 };
                let y = player_rect.top() + rel.y - if self.menu_up { MENU_BODY_H } else { 0.0 };
                let out = egui::Area::new(egui::Id::new("haven-context-menu"))
                    .order(egui::Order::Foreground)
                    .fixed_pos(pos2(x, y))
                    .show(ctx, |ui| {
                        let (rect, _) =
                            ui.allocate_exact_size(vec2(MENU_WIDTH, MENU_BODY_H), Sense::hover());
                        self.draw_menu(ui, rect, ctx);
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
        }

        // Remember the volume (written 600 ms after the last change).
        let vnow = self.engine.shared.vol();
        if self
            .settings
            .volume
            .map_or(true, |v| (v - vnow).abs() > 0.004)
        {
            self.settings.volume = Some(vnow);
            self.vol_dirty = Some(Instant::now());
        }
        if let Some(t) = self.vol_dirty {
            if t.elapsed() >= Duration::from_millis(600) {
                self.settings.save();
                self.vol_dirty = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(650));
            }
        }
        if let Some(t) = self.eq_dirty {
            if t.elapsed() >= Duration::from_millis(500) {
                self.settings.save();
                self.eq_dirty = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(550));
            }
        }
        if self.updater.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        if let Some(t) = self.minimize_pending {
            let settled = self.menu_rel.is_none()
                && !self.layout_busy()
                && (self.req[0] - panel_up_h).abs() < 0.5
                && (self.req[2] - (panel_up_h + base_h + self.card_reserve())).abs() < 0.5;
            if settled || t.elapsed() > Duration::from_millis(1500) {
                self.minimize_pending = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
        // The engine thread applies play/pause a moment after the click. Without
        // this the button kept showing the old state (until the mouse moved), so
        // a second click toggled playback back off.
        let had_input = ctx.input(|i| {
            i.pointer.any_pressed()
                || i.pointer.any_released()
                || i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Key { pressed: true, .. }))
        });
        if had_input {
            self.poll_until = Some(Instant::now() + Duration::from_millis(700));
        }
        if let Some(t) = self.poll_until {
            if Instant::now() < t {
                ctx.request_repaint_after(Duration::from_millis(30));
            } else {
                self.poll_until = None;
            }
        }
        // The update is installing: close the window (the helper script restarts Haven).
        if self.updater.installing() && !self.quit_sent {
            self.quit_sent = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let playing_after = self.engine.shared.playing.load(Relaxed)
            && !self.engine.shared.queue_ended.load(Relaxed);
        if playing_after != self.drawn_playing {
            ctx.request_repaint(); // e.g. the last track just ended: redraw the play icon
        }
        if playing_after {
            let frame_interval = if cfg!(target_os = "macos") { 16 } else { 33 };
            ctx.request_repaint_after(Duration::from_millis(frame_interval));
        } else if self.drag.is_some() {
            ctx.request_repaint_after(Duration::from_millis(30));
        }
    }
}
