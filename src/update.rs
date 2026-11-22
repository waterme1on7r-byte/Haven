//! Update check + download over GitHub Releases.
//!
//! Haven never replaces its own binary: it finds the newest release, downloads the
//! installer for this platform (.exe on Windows, .dmg on macOS) and opens it. Files
//! fetched this way carry no quarantine flag, so macOS does not flag them as damaged.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// "owner/repository" on GitHub that publishes Haven's releases. CHANGE THIS.
pub const REPO: &str = "waterme1on7r-byte/Haven";
const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
}

#[derive(Clone, Debug)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, u64, u64), // done, total bytes (total 0 = unknown)
    Downloaded(PathBuf),
    Failed(String),
}

#[derive(Clone)]
pub struct Updater {
    state: Arc<Mutex<State>>,
}

impl Updater {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::Idle)),
        }
    }

    pub fn current() -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    pub fn snapshot(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    pub fn available(&self) -> bool {
        matches!(*self.state.lock().unwrap(), State::Available(_))
    }

    pub fn busy(&self) -> bool {
        matches!(
            *self.state.lock().unwrap(),
            State::Checking | State::Downloading(..)
        )
    }

    fn set(&self, s: State) {
        *self.state.lock().unwrap() = s;
    }

    /// Silent check shortly after launch, at most once every 24 hours.
    pub fn auto_check(&self) {
        if checked_recently() {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(6));
            me.run_check();
        });
    }

    /// Manual check (Settings -> Updates).
    pub fn check(&self) {
        if self.busy() {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || me.run_check());
    }

    fn run_check(&self) {
        self.set(State::Checking);
        match fetch_latest() {
            Ok(Some(rel)) => {
                stamp();
                self.set(State::Available(rel));
            }
            Ok(None) => {
                stamp();
                self.set(State::UpToDate);
            }
            Err(e) => self.set(State::Failed(e.to_string())),
        }
    }

    /// Downloads the installer for this platform and opens it.
    pub fn download(&self, rel: Release) {
        let (Some(url), Some(name)) = (rel.asset_url.clone(), rel.asset_name.clone()) else {
            open_path_or_url(&rel.page); // no matching file: show the release page
            return;
        };
        let me = self.clone();
        std::thread::spawn(move || match me.run_download(&rel, &url, &name) {
            Ok(p) => {
                open_path_or_url(&p.to_string_lossy());
                me.set(State::Downloaded(p));
            }
            Err(e) => me.set(State::Failed(e.to_string())),
        });
    }

    fn run_download(&self, rel: &Release, url: &str, name: &str) -> anyhow::Result<PathBuf> {
        let resp = agent().get(url).call()?;
        let total: u64 = resp
            .header("Content-Length")
            .and_then(|h| h.parse().ok())
            .unwrap_or(0);
        let file_name = Path::new(name)
            .file_name()
            .map(|n| n.to_os_string())
            .ok_or_else(|| anyhow::anyhow!("bad file name"))?;
        let dir = download_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(file_name);
        let tmp = dir.join(format!("{}.part", path.file_name().unwrap().to_string_lossy()));
        self.set(State::Downloading(rel.clone(), 0, total));
        let mut reader = resp.into_reader();
        let mut f = std::fs::File::create(&tmp)?;
        let mut buf = vec![0u8; 64 * 1024];
        let (mut done, mut last) = (0u64, Instant::now());
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            f.write_all(&buf[..n])?;
            done += n as u64;
            if last.elapsed() > Duration::from_millis(200) {
                self.set(State::Downloading(rel.clone(), done, total));
                last = Instant::now();
            }
        }
        drop(f);
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent(concat!("Haven/", env!("CARGO_PKG_VERSION"))) // GitHub requires a UA
        .build()
}

fn fetch_latest() -> anyhow::Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let v: serde_json::Value = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()?
        .into_json()?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("release has no tag"))?;
    if !newer(tag, Updater::current()) {
        return Ok(None);
    }
    let assets: Vec<serde_json::Value> = v["assets"].as_array().cloned().unwrap_or_default();
    let (asset_name, asset_url) = match pick_asset(&assets) {
        Some((n, u)) => (Some(n), Some(u)),
        None => (None, None),
    };
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_name,
        asset_url,
    }))
}

/// The installer for this OS, preferring the one that matches the CPU.
fn pick_asset(assets: &[serde_json::Value]) -> Option<(String, String)> {
    let ext = if cfg!(windows) {
        ".exe"
    } else if cfg!(target_os = "macos") {
        ".dmg"
    } else {
        return None;
    };
    let arch: &[&str] = if cfg!(target_arch = "aarch64") {
        &["aarch64", "arm64"]
    } else {
        &["x64", "x86_64", "amd64"]
    };
    let mut best: Option<(bool, String, String)> = None;
    for a in assets {
        if let (Some(name), Some(url)) = (a["name"].as_str(), a["browser_download_url"].as_str()) {
            let lower = name.to_lowercase();
            if !lower.ends_with(ext) {
                continue;
            }
            let arch_ok = arch.iter().any(|t| lower.contains(t));
            if best.as_ref().map_or(true, |b| arch_ok && !b.0) {
                best = Some((arch_ok, name.to_string(), url.to_string()));
            }
        }
    }
    best.map(|(_, n, u)| (n, u))
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|p| p.parse::<u64>().ok())
        .collect()
}

/// True when `remote` is a higher version than `local` (1.10.0 > 1.9.0).
pub fn newer(remote: &str, local: &str) -> bool {
    let (a, b) = (parse_version(remote), parse_version(local));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (a.get(i).copied().unwrap_or(0), b.get(i).copied().unwrap_or(0));
        if x != y {
            return x > y;
        }
    }
    false
}

fn download_dir() -> PathBuf {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    };
    home.map(|h| PathBuf::from(h).join("Downloads"))
        .unwrap_or_else(std::env::temp_dir)
}

fn stamp_path() -> Option<PathBuf> {
    crate::settings::config_dir().map(|d| d.join("last_update_check.txt"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn checked_recently() -> bool {
    stamp_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| t.trim().parse::<u64>().ok())
        .map_or(false, |t| now_secs().saturating_sub(t) < CHECK_EVERY_SECS)
}

fn stamp() {
    if let Some(p) = stamp_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, now_secs().to_string());
    }
}

/// Opens a file (installer / disk image) or a web page with the OS default handler.
pub fn open_path_or_url(target: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(target).spawn();
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = target;
    }
}

#[cfg(test)]
mod tests {
    use super::newer;
    #[test]
    fn compares_versions() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("1.10.0", "1.9.0"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("0.0.9", "0.1.0"));
    }
}
