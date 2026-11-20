//! Decodes one file into interleaved stereo f32 (optionally linear-resampled).
use anyhow::{anyhow, Result};
use std::{fs::File, path::Path};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    errors::Error as SErr,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::MediaSourceStream,
    meta::MetadataOptions,
    probe::Hint,
    units::Time,
};

pub struct Reader {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    pub rate: u32,
    out_rate: u32,
    sb: Option<SampleBuffer<f32>>,
    stereo: Vec<f32>,
    buf: Vec<f32>,
    frac: f64,
    last: [f32; 2],
}

impl Reader {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(e) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(e);
        }
        let fmt_opts = FormatOptions {
            enable_gapless: true,
            ..Default::default()
        };
        let probed = symphonia::default::get_probe().format(
            &hint,
            mss,
            &fmt_opts,
            &MetadataOptions::default(),
        )?;
        let format = probed.format;
        let track = format
            .default_track()
            .ok_or_else(|| anyhow!("no audio track"))?;
        let rate = track
            .codec_params
            .sample_rate
            .ok_or_else(|| anyhow!("unknown sample rate"))?;
        let track_id = track.id;
        let decoder = symphonia::default::get_codecs()
            .make(&track.codec_params, &DecoderOptions::default())?;
        Ok(Self {
            format,
            decoder,
            track_id,
            rate,
            out_rate: rate,
            sb: None,
            stereo: vec![],
            buf: vec![],
            frac: 0.0,
            last: [0.0; 2],
        })
    }

    pub fn set_out_rate(&mut self, r: u32) {
        self.out_rate = r;
    }

    pub fn seek(&mut self, secs: f64) {
        let _ = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: Time::from(secs),
                track_id: Some(self.track_id),
            },
        );
        self.decoder.reset();
        self.frac = 0.0;
        self.last = [0.0; 2];
    }

    /// Next block of interleaved stereo f32 at `out_rate`; None at end of stream.
    pub fn next_chunk(&mut self) -> Option<&[f32]> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(_) => return None,
            };
            if packet.track_id() != self.track_id {
                continue;
            }
            match self.decoder.decode(&packet) {
                Ok(d) => {
                    let spec = *d.spec();
                    let ch = spec.channels.count().max(1);
                    let cap = d.capacity();
                    if self.sb.as_ref().map_or(true, |s| s.capacity() < cap * ch) {
                        self.sb = Some(SampleBuffer::new(cap as u64, spec));
                    }
                    let sb = self.sb.as_mut().unwrap();
                    sb.copy_interleaved_ref(d);
                    self.stereo.clear();
                    for f in sb.samples().chunks_exact(ch) {
                        self.stereo.push(f[0]);
                        self.stereo.push(if ch > 1 { f[1] } else { f[0] });
                    }
                }
                Err(SErr::DecodeError(_)) => continue,
                Err(_) => return None,
            }
            if self.out_rate == self.rate {
                return Some(&self.stereo);
            }
            self.resample();
            return Some(&self.buf);
        }
    }

    /// Fallback only: used when the device can't run at the file's native rate.
    fn resample(&mut self) {
        self.buf.clear();
        let n = (self.stereo.len() / 2) as isize;
        let step = self.rate as f64 / self.out_rate as f64;
        let mut pos = self.frac;
        let get = |i: isize, s: &Vec<f32>, last: &[f32; 2], c: usize| -> f32 {
            if i < 0 {
                last[c]
            } else {
                s[i as usize * 2 + c]
            }
        };
        while (pos.floor() as isize) + 1 <= n - 1 {
            let i = pos.floor() as isize;
            let t = (pos - i as f64) as f32;
            for c in 0..2 {
                let a = get(i, &self.stereo, &self.last, c);
                let b = get(i + 1, &self.stereo, &self.last, c);
                self.buf.push(a + (b - a) * t);
            }
            pos += step;
        }
        if n > 0 {
            self.last = [
                self.stereo[(n as usize - 1) * 2],
                self.stereo[(n as usize - 1) * 2 + 1],
            ];
        }
        self.frac = pos - n as f64;
    }
}
