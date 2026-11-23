//! Audio engine: one decoder thread feeds a lock-free ring buffer; the cpal
//! callback only pops samples (no allocation, no locks, no decoding).
use crate::{
    meta::{self, TrackInfo},
    reader::Reader,
};
use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SampleRate};
use rtrb::{Producer, RingBuffer};
use std::{
    path::PathBuf,
    sync::{
        atomic::{
            AtomicBool, AtomicU32, AtomicU64, AtomicU8,
            Ordering::{AcqRel, Acquire, Relaxed, Release},
        },
        mpsc::{self, Receiver, Sender, TryRecvError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub struct Shared {
    pub playing: AtomicBool,
    /// True after the final track reached EOF, separate from the output state
    /// while the callback drains its last buffered samples.
    pub queue_ended: AtomicBool,
    pub pos_ms: AtomicU64,
    pub dur_ms: AtomicU64,
    vol: AtomicU32,
    pub muted: AtomicBool,
    pub shuffle: AtomicBool,
    pub repeat: AtomicU8, // 0 off, 1 all, 2 one
    pub track_gen: AtomicU64,
    pub info: Mutex<Option<TrackInfo>>,
    pub queue_info: Mutex<Vec<TrackInfo>>,
    pub current_idx: AtomicU64,
    flush_req: AtomicU64,
    flush_ack: AtomicU64,
    eq_on: AtomicBool,
    eq_gain: [AtomicU32; 10],
    eq_gen: AtomicU64,
    /// Id of the system's current default output device (0 = unknown). Kept fresh by a
    /// small watcher thread so the audio thread never has to ask the OS itself.
    default_dev: AtomicU64,
    /// Set by the stream's error callback when its device went away (unplugged etc.).
    dev_lost: AtomicBool,
}

/// Centre frequencies of the ten equalizer bands (Hz), one octave apart.
pub const EQ_FREQS: [f32; 10] = [
    32.0, 64.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

impl Shared {
    fn new() -> Self {
        Self {
            playing: AtomicBool::new(false),
            queue_ended: AtomicBool::new(false),
            pos_ms: AtomicU64::new(0),
            dur_ms: AtomicU64::new(0),
            vol: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            shuffle: AtomicBool::new(false),
            repeat: AtomicU8::new(0),
            track_gen: AtomicU64::new(0),
            info: Mutex::new(None),
            queue_info: Mutex::new(vec![]),
            current_idx: AtomicU64::new(0),
            flush_req: AtomicU64::new(0),
            flush_ack: AtomicU64::new(0),
            eq_on: AtomicBool::new(false),
            eq_gain: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            eq_gen: AtomicU64::new(1),
            default_dev: AtomicU64::new(0),
            dev_lost: AtomicBool::new(false),
        }
    }
    /// Sets the equalizer (gains in dB). The decoder thread picks it up on its next block.
    pub fn set_eq(&self, on: bool, gains: [f32; 10]) {
        for (a, g) in self.eq_gain.iter().zip(gains.iter()) {
            a.store(g.clamp(-12.0, 12.0).to_bits(), Relaxed);
        }
        self.eq_on.store(on, Relaxed);
        self.eq_gen.fetch_add(1, Release);
    }
    pub fn vol(&self) -> f32 {
        f32::from_bits(self.vol.load(Relaxed))
    }
    pub fn set_vol(&self, v: f32) {
        self.vol.store(v.clamp(0.0, 1.0).to_bits(), Relaxed)
    }
}

enum Cmd {
    Queue(Vec<PathBuf>, usize),
    Append(Vec<PathBuf>),
    Select(usize),
    /// Explicit target state (never a blind flip, so queued clicks cannot cancel out).
    Play(bool),
    Next,
    Prev,
    Seek(f64),
    Clear,
}

#[derive(Clone)]
pub struct Engine {
    tx: Sender<Cmd>,
    pub shared: Arc<Shared>,
}

impl Engine {
    pub fn new() -> Self {
        let shared = Arc::new(Shared::new());
        let (tx, rx) = mpsc::channel();
        let s = shared.clone();
        // cpal::Stream is !Send, so the whole core is created *inside* its thread.
        thread::Builder::new()
            .name("haven-audio".into())
            .spawn(move || Core::new(s).run(rx))
            .expect("spawn audio thread");
        // Follows the system's default output device (headphones plugged in, output
        // switched in the OS...). The audio thread compares it with the open stream.
        let w = shared.clone();
        let _ = thread::Builder::new()
            .name("haven-devwatch".into())
            .spawn(move || loop {
                w.default_dev.store(default_dev_id(), Relaxed);
                thread::sleep(Duration::from_millis(200));
            });
        Self { tx, shared }
    }
    pub fn queue(&self, p: Vec<PathBuf>) {
        let _ = self.tx.send(Cmd::Queue(p, 0));
    }
    pub fn append(&self, p: Vec<PathBuf>) {
        let _ = self.tx.send(Cmd::Append(p));
    }
    pub fn select(&self, index: usize) {
        let _ = self.tx.send(Cmd::Select(index));
    }
    /// Play/pause. The new state is decided here, from what the button shows, and is
    /// applied at once (so the icon flips immediately); the engine thread gets the
    /// explicit target. Extra clicks while the engine is busy can no longer cancel
    /// each other out.
    pub fn toggle(&self) {
        let want = self.shared.queue_ended.load(Relaxed) || !self.shared.playing.load(Relaxed);
        self.shared.playing.store(want, Relaxed);
        let _ = self.tx.send(Cmd::Play(want));
    }
    pub fn next(&self) {
        let _ = self.tx.send(Cmd::Next);
    }
    pub fn prev(&self) {
        let _ = self.tx.send(Cmd::Prev);
    }
    pub fn seek(&self, secs: f64) {
        let _ = self.tx.send(Cmd::Seek(secs.max(0.0)));
    }
    pub fn clear(&self) {
        let _ = self.tx.send(Cmd::Clear);
    }
}

struct Out {
    _stream: cpal::Stream,
    prod: Producer<f32>,
    rate: u32,
    cap: usize,
    dev: u64, // id of the device this stream plays on
}

fn fnv1a(b: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for x in b {
        h ^= *x as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Stable id of an output device (hash of its name; never 0).
fn dev_id(d: &cpal::Device) -> u64 {
    d.name().map_or(0, |n| fnv1a(n.as_bytes()) | 1)
}

fn default_dev_id() -> u64 {
    cpal::default_host()
        .default_output_device()
        .map_or(0, |d| dev_id(&d))
}

/// Opens the default device at the file's native rate when the device supports it
/// (no resampling), otherwise at the device default.
fn open_out(want: u32, sh: &Arc<Shared>) -> Result<Out> {
    let dev = cpal::default_host()
        .default_output_device()
        .ok_or_else(|| anyhow!("no output device"))?;
    let mut chosen = None;
    for r in dev.supported_output_configs()? {
        if r.sample_format() == SampleFormat::F32
            && r.channels() >= 2
            && r.min_sample_rate().0 <= want
            && want <= r.max_sample_rate().0
        {
            chosen = Some(r.with_sample_rate(SampleRate(want)));
            break;
        }
    }
    let cfg = match chosen {
        Some(c) => c,
        None => dev.default_output_config()?,
    };
    if cfg.sample_format() != SampleFormat::F32 {
        return Err(anyhow!("device has no f32 output"));
    }
    let rate = cfg.sample_rate().0;
    let ch = cfg.channels() as usize;
    let dev_key = dev_id(&dev);
    let s_err = sh.clone();
    let cap = rate as usize * 2 * 3 / 10; // ~300 ms of stereo
    let (prod, mut cons) = RingBuffer::<f32>::new(cap);
    let s = sh.clone();
    let stream = dev.build_output_stream(
        &cfg.config(),
        move |data: &mut [f32], _| {
            let g = s.flush_req.load(Acquire);
            if g != s.flush_ack.load(Relaxed) {
                while cons.pop().is_ok() {}
                s.flush_ack.store(g, Release);
            }
            let play = s.playing.load(Relaxed);
            let v = if s.muted.load(Relaxed) { 0.0 } else { s.vol() }; // 1.0 == bit-exact
            for frame in data.chunks_mut(ch) {
                let (l, r) = if play {
                    (cons.pop().unwrap_or(0.0), cons.pop().unwrap_or(0.0))
                } else {
                    (0.0, 0.0)
                };
                frame[0] = l * v;
                frame[1] = r * v;
                for x in frame.iter_mut().skip(2) {
                    *x = 0.0;
                }
            }
        },
        move |e| {
            eprintln!("haven: audio error: {e}");
            if matches!(e, cpal::StreamError::DeviceNotAvailable) {
                s_err.dev_lost.store(true, Relaxed); // the audio thread reopens on the default device
            }
        },
        None,
    )?;
    stream.play()?;
    Ok(Out {
        _stream: stream,
        prod,
        rate,
        cap,
        dev: dev_key,
    })
}

/// One peaking biquad (RBJ cookbook), stereo, f64 state.
#[derive(Clone, Copy, Default)]
struct Bq {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    z1: [f64; 2],
    z2: [f64; 2],
    on: bool,
}

impl Bq {
    fn set_peak(&mut self, fs: f64, f0: f64, gain_db: f64, q: f64) {
        if gain_db.abs() < 0.05 || f0 >= fs * 0.45 {
            self.on = false;
            self.z1 = [0.0; 2];
            self.z2 = [0.0; 2];
            return;
        }
        let a = 10f64.powf(gain_db / 40.0);
        let (s, c) = (std::f64::consts::TAU * f0 / fs).sin_cos();
        let al = s / (2.0 * q);
        let a0 = 1.0 + al / a;
        self.b0 = (1.0 + al * a) / a0;
        self.b1 = (-2.0 * c) / a0;
        self.b2 = (1.0 - al * a) / a0;
        self.a1 = (-2.0 * c) / a0;
        self.a2 = (1.0 - al / a) / a0;
        self.on = true;
    }
    #[inline]
    fn run(&mut self, x: f64, ch: usize) -> f64 {
        let y = self.b0 * x + self.z1[ch];
        self.z1[ch] = self.b1 * x - self.a1 * y + self.z2[ch];
        self.z2[ch] = self.b2 * x - self.a2 * y;
        y
    }
}

/// Ten-band equalizer applied to decoded blocks before they enter the ring.
/// When it is off (or every band is 0 dB) blocks are not touched at all, so
/// playback stays bit-exact.
struct EqDsp {
    bands: [Bq; 10],
    pre: f64,
    active: bool,
    gen: u64,
    rate: u32,
}

impl EqDsp {
    fn new() -> Self {
        Self {
            bands: [Bq::default(); 10],
            pre: 1.0,
            active: false,
            gen: u64::MAX,
            rate: 0,
        }
    }
    fn reset(&mut self) {
        for b in self.bands.iter_mut() {
            b.z1 = [0.0; 2];
            b.z2 = [0.0; 2];
        }
    }
    fn sync(&mut self, sh: &Shared, rate: u32) {
        let gen = sh.eq_gen.load(Acquire);
        if gen == self.gen && rate == self.rate {
            return;
        }
        self.gen = gen;
        self.rate = rate;
        let on = sh.eq_on.load(Relaxed);
        let (mut max_boost, mut any) = (0.0f64, false);
        for i in 0..10 {
            let g = f32::from_bits(sh.eq_gain[i].load(Relaxed)) as f64;
            let g = if on { g } else { 0.0 };
            self.bands[i].set_peak(rate as f64, EQ_FREQS[i] as f64, g, 1.41);
            if g.abs() >= 0.05 {
                any = true;
                max_boost = max_boost.max(g);
            }
        }
        let was = self.active;
        self.active = on && any;
        // Automatic preamp cut equal to the largest boost, so boosts cannot clip.
        self.pre = 10f64.powf(-max_boost / 20.0);
        if !was && self.active {
            self.reset();
        }
    }
    fn process(&mut self, buf: &mut [f32]) {
        if !self.active {
            return;
        }
        let pre = self.pre;
        for f in buf.chunks_exact_mut(2) {
            for ch in 0..2 {
                let mut x = f[ch] as f64 * pre;
                for b in self.bands.iter_mut() {
                    if b.on {
                        x = b.run(x, ch);
                    }
                }
                f[ch] = (x as f32).clamp(-1.0, 1.0);
            }
        }
    }
}

struct Core {
    sh: Arc<Shared>,
    queue: Vec<PathBuf>,
    idx: usize,
    reader: Option<Reader>,
    out: Option<Out>,
    pending: Vec<f32>,
    poff: usize,
    base: u64,   // frames (out rate) at which the current segment started
    pushed: u64, // samples pushed since `base`
    rng: u64,
    eq: EqDsp,
    /// End of the queue reached: waiting (without blocking) for the ring to empty.
    tail: Option<Instant>,
    dev_retry: Instant,
}

impl Core {
    fn new(sh: Arc<Shared>) -> Self {
        Self {
            sh,
            queue: vec![],
            idx: 0,
            reader: None,
            out: None,
            pending: vec![],
            poff: 0,
            base: 0,
            pushed: 0,
            rng: 0x9E37_79B9_7F4A_7C15,
            eq: EqDsp::new(),
            tail: None,
            dev_retry: Instant::now(),
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            loop {
                match rx.try_recv() {
                    Ok(c) => self.cmd(c),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return,
                }
            }
            self.follow_device();
            if let Some(t) = self.tail {
                let empty = self.out.as_ref().map_or(true, |o| o.prod.slots() >= o.cap);
                if empty || t.elapsed() > Duration::from_secs(2) {
                    self.tail = None;
                    self.sh.playing.store(false, Relaxed);
                    self.sh.queue_ended.store(true, Relaxed);
                }
            }
            let mut worked = false;
            for _ in 0..16 {
                if self.pump() {
                    worked = true
                } else {
                    break;
                }
            }
            self.update_pos();
            if !worked {
                thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Queue(p, s) => {
                if p.is_empty() {
                    return;
                }
                *self.sh.queue_info.lock().unwrap() = p.iter().map(|p| meta::read(p)).collect();
                self.queue = p;
                self.sh.queue_ended.store(false, Relaxed);
                self.sh.playing.store(true, Relaxed);
                self.start_from(s.min(self.queue.len() - 1), false);
            }
            Cmd::Append(p) => {
                let old = self.queue.len();
                self.sh
                    .queue_info
                    .lock()
                    .unwrap()
                    .extend(p.iter().map(|p| meta::read(p)));
                self.queue.extend(p);
                if self.reader.is_none() && self.queue.len() > old {
                    self.sh.queue_ended.store(false, Relaxed);
                    self.sh.playing.store(true, Relaxed);
                    self.start_from(old, false);
                }
            }
            Cmd::Select(i) => {
                if i < self.queue.len() {
                    self.sh.queue_ended.store(false, Relaxed);
                    self.sh.playing.store(true, Relaxed);
                    self.start_from(i, false);
                }
            }
            Cmd::Play(true) => {
                if self.reader.is_none() {
                    // Nothing is loaded in the decoder (the queue ended): start it again.
                    if self.queue.is_empty() {
                        self.sh.playing.store(false, Relaxed);
                    } else {
                        self.sh.queue_ended.store(false, Relaxed);
                        self.sh.playing.store(true, Relaxed);
                        self.start_from(0, false);
                    }
                } else {
                    self.sh.playing.store(true, Relaxed);
                }
            }
            Cmd::Play(false) => self.sh.playing.store(false, Relaxed),
            Cmd::Next => {
                if let Some(i) = self.next_idx(false) {
                    self.sh.queue_ended.store(false, Relaxed);
                    self.sh.playing.store(true, Relaxed);
                    self.start_from(i, false);
                }
            }
            Cmd::Prev => {
                if self.sh.pos_ms.load(Relaxed) > 3000 || self.queue.len() < 2 {
                    self.seek(0.0);
                } else if !self.queue.is_empty() {
                    let i = if self.idx == 0 {
                        self.queue.len() - 1
                    } else {
                        self.idx - 1
                    };
                    self.sh.playing.store(true, Relaxed);
                    self.sh.queue_ended.store(false, Relaxed);
                    self.start_from(i, false);
                }
            }
            Cmd::Seek(s) => self.seek(s),
            Cmd::Clear => {
                // Only clear the list: the track that is playing keeps playing.
                if self.reader.is_some() && self.idx < self.queue.len() {
                    let keep = self.queue[self.idx].clone();
                    let keep_info = self.sh.queue_info.lock().unwrap().get(self.idx).cloned();
                    self.queue = vec![keep];
                    self.idx = 0;
                    let mut qi = self.sh.queue_info.lock().unwrap();
                    qi.clear();
                    if let Some(i) = keep_info {
                        qi.push(i);
                    }
                    self.sh.current_idx.store(0, Relaxed);
                    return;
                }
                self.queue.clear();
                self.reader = None;
                self.tail = None;
                self.pending.clear();
                self.poff = 0;
                self.flush();
                self.sh.playing.store(false, Relaxed);
                self.sh.pos_ms.store(0, Relaxed);
                self.sh.dur_ms.store(0, Relaxed);
                self.sh.current_idx.store(0, Relaxed);
                *self.sh.info.lock().unwrap() = None;
                self.sh.queue_info.lock().unwrap().clear();
                self.sh.track_gen.fetch_add(1, Relaxed);
            }
        }
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn next_idx(&mut self, auto: bool) -> Option<usize> {
        let n = self.queue.len();
        if n == 0 {
            return None;
        }
        let rep = self.sh.repeat.load(Relaxed);
        if auto && rep == 2 {
            return Some(self.idx);
        }
        if self.sh.shuffle.load(Relaxed) && n > 1 {
            loop {
                let i = (self.rand() % n as u64) as usize;
                if i != self.idx {
                    return Some(i);
                }
            }
        }
        if self.idx + 1 < n {
            Some(self.idx + 1)
        } else if rep == 1 || !auto {
            Some(0)
        } else {
            None
        }
    }

    fn start_from(&mut self, mut idx: usize, gapless: bool) {
        self.tail = None;
        self.sh.queue_ended.store(false, Relaxed);
        for _ in 0..self.queue.len() {
            if self.start(idx, gapless) {
                self.idx = idx;
                self.sh.current_idx.store(idx as u64, Relaxed);
                return;
            }
            idx = (idx + 1) % self.queue.len();
        }
        self.reader = None;
        self.sh.playing.store(false, Relaxed);
    }

    fn start(&mut self, idx: usize, gapless: bool) -> bool {
        let path = self.queue[idx].clone();
        let mut r = match Reader::open(&path) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("haven: {}: {e}", path.display());
                return false;
            }
        };
        let same = self.out.as_ref().map_or(false, |o| o.rate == r.rate);
        if same {
            if !gapless {
                self.flush();
            }
        } else {
            if gapless {
                self.drain();
            }
            self.out = None;
            match open_out(r.rate, &self.sh) {
                Ok(o) => self.out = Some(o),
                Err(e) => {
                    eprintln!("haven: audio output: {e}");
                    return false;
                }
            }
        }
        r.set_out_rate(self.out.as_ref().unwrap().rate);
        let info = meta::read(&path);
        self.sh.dur_ms.store(info.duration_ms, Relaxed);
        *self.sh.info.lock().unwrap() = Some(info);
        self.sh.track_gen.fetch_add(1, Relaxed);
        self.reader = Some(r);
        if !gapless {
            self.eq.reset();
        }
        self.pending.clear();
        self.poff = 0;
        self.base = 0;
        self.pushed = 0;
        true
    }

    /// Decode one block and push as much as fits. Returns true if progress was made.
    fn pump(&mut self) -> bool {
        if self.out.is_none() || self.reader.is_none() {
            return false;
        }
        if self.poff >= self.pending.len() {
            let next = self
                .reader
                .as_mut()
                .unwrap()
                .next_chunk()
                .map(|c| c.to_vec());
            match next {
                Some(c) => {
                    self.pending = c;
                    self.poff = 0;
                    let rate = self.out.as_ref().map_or(44100, |o| o.rate);
                    self.eq.sync(&self.sh, rate);
                    self.eq.process(&mut self.pending);
                }
                None => {
                    self.finish_track();
                    return true;
                }
            }
        }
        let o = self.out.as_mut().unwrap();
        let n = o.prod.slots().min(self.pending.len() - self.poff) & !1;
        if n == 0 {
            return false;
        }
        for s in &self.pending[self.poff..self.poff + n] {
            let _ = o.prod.push(*s);
        }
        self.poff += n;
        self.pushed += n as u64;
        true
    }

    /// End of file: roll straight into the next track with the same ring buffer (gapless).
    fn finish_track(&mut self) {
        self.reader = None;
        match self.next_idx(true) {
            Some(i) => self.start_from(i, true),
            None => {
                // Let the last ~300 ms play out, but do not block this thread meanwhile:
                // clicks that arrive now are handled at once (see `tail` in `run`).
                self.sh.queue_ended.store(true, Relaxed);
                self.tail = Some(Instant::now());
            }
        }
    }

    /// Moves playback to the system's current default output device when it changes
    /// (or when the device in use disappears). Resumes from where it was audible.
    fn follow_device(&mut self) {
        let Some(o) = self.out.as_ref() else {
            return;
        };
        let lost = self.sh.dev_lost.load(Relaxed);
        let cur = self.sh.default_dev.load(Relaxed);
        if !lost && (cur == 0 || cur == o.dev) {
            return;
        }
        if Instant::now() < self.dev_retry {
            return;
        }
        self.dev_retry = Instant::now() + Duration::from_millis(250);
        let rate = o.rate;
        let pos = self.sh.pos_ms.load(Relaxed) as f64 / 1000.0;
        match open_out(rate, &self.sh) {
            Ok(new) => {
                let new_rate = new.rate;
                self.out = Some(new); // drops the old stream
                self.sh.dev_lost.store(false, Relaxed);
                if let Some(r) = self.reader.as_mut() {
                    r.set_out_rate(new_rate);
                }
                self.pending.clear();
                self.poff = 0;
                if self.reader.is_some() {
                    self.seek(pos); // the old ring's ~300 ms is gone: replay it on the new device
                }
            }
            Err(e) => eprintln!("haven: switching output device: {e}"),
        }
    }

    fn seek(&mut self, s: f64) {
        let Some(r) = self.reader.as_mut() else {
            return;
        };
        r.seek(s);
        self.eq.reset();
        self.flush();
        self.pending.clear();
        self.poff = 0;
        self.base = (s * self.out.as_ref().map_or(0.0, |o| o.rate as f64)) as u64;
        self.pushed = 0;
    }

    /// Ask the audio callback to drop buffered samples and wait for its ack.
    fn flush(&mut self) {
        if self.out.is_none() {
            return;
        }
        let g = self.sh.flush_req.fetch_add(1, AcqRel) + 1;
        let t = Instant::now();
        while self.sh.flush_ack.load(Acquire) != g && t.elapsed() < Duration::from_millis(300) {
            thread::sleep(Duration::from_millis(2));
        }
    }

    fn drain(&mut self) {
        if !self.sh.playing.load(Relaxed) {
            return;
        }
        if let Some(o) = &self.out {
            let t = Instant::now();
            while o.prod.slots() < o.cap && t.elapsed() < Duration::from_secs(2) {
                thread::sleep(Duration::from_millis(2));
            }
        }
    }

    fn update_pos(&self) {
        if let Some(o) = &self.out {
            let buffered = ((o.cap - o.prod.slots()) / 2) as u64;
            let f = (self.base + self.pushed / 2).saturating_sub(buffered);
            self.sh.pos_ms.store(f * 1000 / o.rate as u64, Relaxed);
        }
    }
}
