//! Tiny persisted settings (two flags) in the platform config folder. No dependencies.
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub struct Settings {
    pub on_top: bool,
    pub locked: bool,
    /// Window outer position in points (x, y).
    pub pos: Option<(f32, f32)>,
    /// 0 = small, 1 = medium, 2 = big (default, the original size).
    pub size: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            on_top: false,
            locked: false,
            pos: None,
            size: 2,
        }
    }
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
                    _ => {
                        if let Some(v) = l.strip_prefix("x=") {
                            x = v.parse::<f32>().ok();
                        }
                        if let Some(v) = l.strip_prefix("y=") {
                            y = v.parse::<f32>().ok();
                        }
                        if let Some(v) = l.strip_prefix("size=") {
                            if let Ok(n) = v.parse::<u8>() {
                                s.size = n.min(2);
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
                "on_top={}\nlocked={}\nsize={}\n",
                self.on_top as u8, self.locked as u8, self.size
            );
            if let Some((x, y)) = self.pos {
                t += &format!("x={x}\ny={y}\n");
            }
            let _ = std::fs::write(p, t);
        }
    }
}
