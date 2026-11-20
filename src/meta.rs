//! Tags/stream info (lofty) and background waveform peak extraction.
use crate::reader::Reader;
use lofty::prelude::*;
use std::{
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc, RwLock},
};

// Fine enough that adjacent waveform columns retain the shape of the recording.
pub const BINS_PER_SEC: f64 = 100.0;

#[derive(Clone, Default)]
pub struct TrackInfo {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<u32>,
    pub track_no: Option<u32>,
    pub codec: String,
    pub sample_rate: u32,
    pub bit_depth: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    pub duration_ms: u64,
}

impl TrackInfo {
    pub fn tooltip(&self) -> String {
        let mut s = self.title.clone();
        if !self.artist.is_empty() {
            s += &format!(" — {}", self.artist);
        }
        if !self.album.is_empty() {
            s += &format!("\n{}", self.album);
        }
        s += &format!("\n{}", self.codec);
        if let Some(b) = self.bit_depth {
            s += &format!(" {b}-bit");
        }
        if self.sample_rate > 0 {
            s += &format!(" / {:.1} kHz", self.sample_rate as f32 / 1000.0);
        }
        if let Some(k) = self.bitrate_kbps {
            s += &format!(" · {k} kbps");
        }
        s
    }
}

pub fn read(path: &Path) -> TrackInfo {
    let mut i = TrackInfo {
        path: path.to_path_buf(),
        title: path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        codec: path
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Ok(t) = lofty::read_from_path(path) {
        let p = t.properties();
        i.sample_rate = p.sample_rate().unwrap_or(0);
        i.bit_depth = p.bit_depth();
        i.bitrate_kbps = p.audio_bitrate();
        i.duration_ms = p.duration().as_millis() as u64;
        if let Some(tag) = t.primary_tag().or_else(|| t.first_tag()) {
            if let Some(x) = tag.title() {
                i.title = x.into_owned();
            }
            i.artist = tag.artist().map(|c| c.into_owned()).unwrap_or_default();
            i.album = tag.album().map(|c| c.into_owned()).unwrap_or_default();
            i.year = tag.year();
            i.track_no = tag.track();
        }
    }
    i
}

pub struct Waveform {
    pub peaks: RwLock<Vec<f32>>,
    pub done: AtomicBool,
}

/// Decodes the file on a background thread; stops itself when the UI drops its Arc.
pub fn spawn_waveform(path: PathBuf) -> Arc<Waveform> {
    let wf = Arc::new(Waveform {
        peaks: RwLock::new(vec![]),
        done: AtomicBool::new(false),
    });
    let w2 = wf.clone();
    std::thread::spawn(move || {
        let Ok(mut r) = Reader::open(&path) else {
            return;
        };
        let bin = ((r.rate as f64 / BINS_PER_SEC) as usize).max(1);
        let (mut cnt, mut sum_sq, mut peak) = (0usize, 0.0f64, 0.0f32);
        while let Some(chunk) = r.next_chunk() {
            let mut local = Vec::new();
            for f in chunk.chunks_exact(2) {
                let frame_peak = f[0].abs().max(f[1].abs());
                peak = peak.max(frame_peak);
                sum_sq += (f[0] as f64 * f[0] as f64 + f[1] as f64 * f[1] as f64) * 0.5;
                cnt += 1;
                if cnt == bin {
                    let rms = (sum_sq / cnt as f64).sqrt() as f32;
                    // Preserve transients while keeping per-window loudness visible.
                    local.push((rms * 0.72 + peak * 0.28).clamp(0.0, 1.0));
                    cnt = 0;
                    sum_sq = 0.0;
                    peak = 0.0;
                }
            }
            if Arc::strong_count(&w2) == 1 {
                return;
            }
            if !local.is_empty() {
                w2.peaks.write().unwrap().extend(local);
            }
        }
        if cnt > 0 {
            let rms = (sum_sq / cnt as f64).sqrt() as f32;
            w2.peaks
                .write()
                .unwrap()
                .push((rms * 0.72 + peak * 0.28).clamp(0.0, 1.0));
        }
        w2.done.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    wf
}
