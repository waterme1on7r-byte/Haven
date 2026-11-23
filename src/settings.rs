//! Persisted player and appearance settings in the platform config folder. No dependencies.
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub struct Settings {
    pub on_top: bool,
    pub locked: bool,
    /// Window outer position in points (x, y).
    pub pos: Option<(f32, f32)>,
    /// 0 = small, 1 = medium, 2 = big, 3 = macOS Menu Bar layout.
    pub size: u8,
    /// Waveform style 0..=4.
    pub wave: u8,
    /// Last used volume 0..=1 (None = never changed).
    pub volume: Option<f32>,
    /// Equalizer on/off and the ten band gains in dB (-12..=12).
    pub eq_on: bool,
    pub eq: [f32; 10],
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            on_top: false,
            locked: false,
            pos: None,
            size: 2,
            wave: 0,
            volume: None,
            eq_on: false,
            eq: [0.0; 10],
        }
    }
}

/// The Haven settings folder.
pub fn config_dir() -> Option<PathBuf> {
    path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
}

fn path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    };
    Some(base?.join("Haven").join("settings.txt"))
}

impl Settings {
    /// Window inner size in points for the chosen size.
    pub fn dims(&self) -> [f32; 2] {
        Self::dims_for(self.size)
    }
    pub fn dims_for(n: u8) -> [f32; 2] {
        match n {
            0 => [378.0, 126.0],
            1 => [504.0, 168.0],
            #[cfg(target_os = "macos")]
            3 => [350.0, 300.0],
            _ => [630.0, 210.0],
        }
    }

    pub fn load() -> Self {
        let mut s = Self::default();
        let (mut x, mut y) = (None, None);
        if let Some(t) = path().and_then(|p| std::fs::read_to_string(p).ok()) {
            for l in t.lines() {
                let l = l.trim();
                match l {
                    "on_top=1" => s.on_top = true,
                    "locked=1" => s.locked = true,
                    "eq_on=1" => s.eq_on = true,
                    _ => {
                        if let Some(v) = l.strip_prefix("x=") {
                            x = v.parse::<f32>().ok();
                        }
                        if let Some(v) = l.strip_prefix("y=") {
                            y = v.parse::<f32>().ok();
                        }
                        if let Some(v) = l.strip_prefix("volume=") {
                            if let Ok(n) = v.parse::<f32>() {
                                s.volume = Some(n.clamp(0.0, 1.0));
                            }
                        }
                        if let Some(v) = l.strip_prefix("eq=") {
                            for (i, n) in v.split(',').take(10).enumerate() {
                                if let Ok(g) = n.trim().parse::<f32>() {
                                    s.eq[i] = g.clamp(-12.0, 12.0);
                                }
                            }
                        }
                        if let Some(v) = l.strip_prefix("wave=") {
                            if let Ok(n) = v.parse::<u8>() {
                                s.wave = n.min(4);
                            }
                        }
                        if let Some(v) = l.strip_prefix("size=") {
                            if let Ok(n) = v.parse::<u8>() {
                                s.size = n.min(if cfg!(target_os = "macos") { 3 } else { 2 });
                            }
                        }
                    }
                }
            }
        }
        // Ignore obviously bogus positions (e.g. a title area that would be off the top of the screen).
        if let (Some(x), Some(y)) = (x, y) {
            if x.abs() < 10000.0 && y > -20.0 && y < 10000.0 {
                s.pos = Some((x, y));
            }
        }
        s
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let mut t = format!(
                "on_top={}\nlocked={}\nsize={}\nwave={}\n",
                self.on_top as u8, self.locked as u8, self.size, self.wave
            );
            t += &format!("eq_on={}\n", self.eq_on as u8);
            t += &format!(
                "eq={}\n",
                self.eq
                    .iter()
                    .map(|g| format!("{g:.1}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            if let Some(v) = self.volume {
                t += &format!("volume={v:.3}\n");
            }
            if let Some((x, y)) = self.pos {
                t += &format!("x={x}\ny={y}\n");
            }
            let _ = std::fs::write(p, t);
        }
    }
}
