//! Update check + one-click install over GitHub Releases.
//!
//! Flow: find the newest release -> download the installer for this platform (.exe on
//! Windows, .dmg on macOS) -> launch a small helper script and quit. The helper waits for
//! Haven to exit, installs silently (Windows: the NSIS installer with `/S`; macOS: copies
//! the app out of the disk image over the running bundle), then starts the new Haven.
//! If the automatic install is not possible the installer is simply opened, as before.
//! Files fetched this way carry no quarantine flag, so macOS does not flag them as damaged.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// "owner/repository" on GitHub that publishes Haven's releases.
pub const REPO: &str = "waterme1on7r-byte/Haven";
const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub page: String,
    pub asset_name: Option<String>,
    pub asset_url: Option<String>,
    pub asset_size: u64,
}

#[derive(Clone, Debug)]
pub enum State {
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading(Release, u64, u64), // done, total bytes (total 0 = unknown)
    /// Installer handed to the helper script; Haven is about to quit and restart.
    Installing,
    /// Automatic install was not possible; the installer was opened for the user.
    Downloaded(PathBuf, String),
    /// Short reason; `Some(release)` when it was the download/install that failed.
    Failed(String, Option<Release>),
}

fn is_busy(s: &State) -> bool {
    matches!(
        s,
        State::Checking | State::Downloading(..) | State::Installing
    )
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
        is_busy(&self.state.lock().unwrap())
    }

    /// True once the helper script is running and Haven should close its window.
    pub fn installing(&self) -> bool {
        matches!(*self.state.lock().unwrap(), State::Installing)
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
        let me = self.clone();
        std::thread::spawn(move || me.run_check());
    }

    fn run_check(&self) {
        {
            // Never replace a running check/download/install with a new check.
            let mut s = self.state.lock().unwrap();
            if is_busy(&s) {
                return;
            }
            *s = State::Checking;
        }
        match fetch_latest() {
            Ok(Some(rel)) => {
                stamp();
                self.set(State::Available(rel));
            }
            Ok(None) => {
                stamp();
                self.set(State::UpToDate);
            }
            Err(e) => self.set(State::Failed(e.to_string(), None)),
        }
    }

    /// Downloads the installer for this platform, then installs it and restarts Haven.
    pub fn download(&self, rel: Release) {
        let (Some(url), Some(name)) = (rel.asset_url.clone(), rel.asset_name.clone()) else {
            open_path_or_url(&rel.page); // no matching file: show the release page
            return;
        };
        {
            // The state flips to "Downloading" here, before the thread starts, so a
            // second click can never start a second download over the same file.
            let mut s = self.state.lock().unwrap();
            if is_busy(&s) {
                return;
            }
            *s = State::Downloading(rel.clone(), 0, rel.asset_size);
        }
        let me = self.clone();
        std::thread::spawn(move || match me.run_download(&rel, &url, &name) {
            Ok(path) => me.install(path),
            Err(e) => me.set(State::Failed(e.to_string(), Some(rel))),
        });
    }

    fn run_download(&self, rel: &Release, url: &str, name: &str) -> anyhow::Result<PathBuf> {
        let resp = agent().get(url).call().map_err(net_err)?;
        let total: u64 = resp
            .header("Content-Length")
            .and_then(|h| h.parse().ok())
            .unwrap_or(rel.asset_size);
        let file_name = Path::new(name)
            .file_name()
            .map(|n| n.to_os_string())
            .ok_or_else(|| anyhow::anyhow!("bad file name"))?;
        let dir = update_dir();
        let _ = std::fs::remove_dir_all(&dir); // stale installers / .part files
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&file_name);
        let tmp = dir.join(format!("{}.part", file_name.to_string_lossy()));
        self.set(State::Downloading(rel.clone(), 0, total));

        let result = (|| -> anyhow::Result<u64> {
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
            f.flush()?;
            f.sync_all()?;
            Ok(done)
        })();
        let done = match result {
            Ok(d) => d,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e);
            }
        };
        if (total > 0 && done != total) || done < 100_000 {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!("download incomplete");
        }
        self.set(State::Downloading(rel.clone(), done, done));
        std::fs::rename(&tmp, &path)?;
        Ok(path)
    }

    /// Hands the downloaded installer to the helper script and quits. When that is not
    /// possible, falls back to opening the installer for the user.
    fn install(&self, path: PathBuf) {
        match apply_update(&path) {
            Ok(()) => {
                self.set(State::Installing); // the UI closes the window
                                             // If the window has not ended the process in a few seconds, end it here
                                             // so the helper (which waits for Haven to exit) can go ahead.
                std::thread::sleep(Duration::from_secs(5));
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("haven: automatic install unavailable: {e}");
                open_path_or_url(&path.to_string_lossy());
                self.set(State::Downloaded(path, e.to_string()));
            }
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(30))
        .user_agent(concat!("Haven/", env!("CARGO_PKG_VERSION"))) // GitHub requires a UA
        .build()
}

/// Short, readable reason for the Settings row.
fn net_err(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(404, _) => anyhow::anyhow!("release not found (404)"),
        ureq::Error::Status(403, _) | ureq::Error::Status(429, _) => {
            anyhow::anyhow!("GitHub rate limit, try later")
        }
        ureq::Error::Status(c, _) => anyhow::anyhow!("server error {c}"),
        ureq::Error::Transport(_) => anyhow::anyhow!("no connection"),
    }
}

fn update_dir() -> PathBuf {
    std::env::temp_dir().join("Haven-update")
}

fn fetch_latest() -> anyhow::Result<Option<Release>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let v: serde_json::Value = agent()
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(net_err)?
        .into_json()?;
    let tag = v["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("release has no tag"))?;
    if !newer(tag, Updater::current()) {
        return Ok(None);
    }
    let assets: Vec<serde_json::Value> = v["assets"].as_array().cloned().unwrap_or_default();
    let (asset_name, asset_url, asset_size) = match pick_asset(&assets) {
        Some((n, u, z)) => (Some(n), Some(u), z),
        None => (None, None, 0),
    };
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        page: v["html_url"].as_str().unwrap_or("").to_string(),
        asset_name,
        asset_url,
        asset_size,
    }))
}

/// The installer for this OS, preferring the one that matches the CPU.
fn pick_asset(assets: &[serde_json::Value]) -> Option<(String, String, u64)> {
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
    let mut best: Option<(bool, String, String, u64)> = None;
    for a in assets {
        if let (Some(name), Some(url)) = (a["name"].as_str(), a["browser_download_url"].as_str()) {
            let lower = name.to_lowercase();
            if !lower.ends_with(ext) {
                continue;
            }
            let arch_ok = arch.iter().any(|t| lower.contains(t));
            if best.as_ref().map_or(true, |b| arch_ok && !b.0) {
                best = Some((
                    arch_ok,
                    name.to_string(),
                    url.to_string(),
                    a["size"].as_u64().unwrap_or(0),
                ));
            }
        }
    }
    best.map(|(_, n, u, z)| (n, u, z))
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
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
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

/// Runs a helper that waits for this process to exit, installs `installer` silently
/// and starts the new Haven. All paths reach the helper through environment variables
/// (not through the script text), so non-ASCII user names and spaces are safe.
#[cfg(windows)]
fn apply_update(installer: &Path) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    // The NSIS installer is per-user (no administrator prompt) and `/S` makes it silent.
    const SCRIPT: &str = r#"@echo off
setlocal
set /a N=0
:wait
tasklist /FI "PID eq %HAVEN_PID%" /NH 2>nul | find "%HAVEN_PID%" >nul
if errorlevel 1 goto go
set /a N+=1
if %N% geq 40 goto go
ping -n 2 127.0.0.1 >nul
goto wait
:go
ping -n 2 127.0.0.1 >nul
start "" /wait "%HAVEN_INSTALLER%" /S
set "NEW=%LOCALAPPDATA%\Haven\haven.exe"
if /I "%HAVEN_APP%"=="%NEW%" goto launch_app
if exist "%NEW%" goto launch_new
:launch_app
start "" "%HAVEN_APP%"
goto done
:launch_new
start "" "%NEW%"
:done
del /f /q "%HAVEN_INSTALLER%" >nul 2>&1
(goto) 2>nul & del /f /q "%~f0"
"#;
    let app = std::env::current_exe()?;
    let script = std::env::temp_dir().join("haven-update.cmd");
    std::fs::write(&script, SCRIPT.replace('\n', "\r\n"))?;
    Command::new("cmd")
        .arg("/D")
        .arg("/C")
        .raw_arg(format!("\"\"{}\"\"", script.display()))
        .env("HAVEN_PID", std::process::id().to_string())
        .env("HAVEN_INSTALLER", installer)
        .env("HAVEN_APP", &app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000 | 0x0000_0200) // CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        .spawn()?;
    Ok(())
}

/// macOS: waits for Haven to quit, swaps the app out of the disk image into the place
/// the running Haven.app lives, and reopens it. If the bundle cannot be replaced (no
/// permission, not running from an .app) the old app is reopened and the disk image shown.
#[cfg(target_os = "macos")]
fn apply_update(dmg: &Path) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    const SCRIPT: &str = r#"#!/bin/sh
# Haven update helper (macOS). Haven is only ad-hoc signed (no Apple Developer ID), so this
# script makes the new bundle launchable instead of relying on Gatekeeper's blessing:
#  - clears quarantine/extended attributes from the disk image and the new app,
#  - re-signs the new app ad hoc so the signature is valid on this Mac,
#  - swaps it in place (asking for an administrator password only if it must),
#  - refreshes Launch Services so the icon / registration are current.
i=0
while kill -0 "$HAVEN_PID" 2>/dev/null && [ "$i" -lt 60 ]; do sleep 0.5; i=$((i+1)); done
OK=0
WORK=$(mktemp -d /tmp/haven-update.XXXXXX)
MNT="$WORK/mnt"
STAGE="$WORK/Haven.app"
mkdir -p "$MNT"
xattr -dr com.apple.quarantine "$HAVEN_DMG" >/dev/null 2>&1
if hdiutil attach "$HAVEN_DMG" -nobrowse -noverify -noautoopen -quiet -mountpoint "$MNT" >/dev/null 2>&1; then
  SRC=$(find "$MNT" -maxdepth 1 -name '*.app' | head -n 1)
  if [ -z "$SRC" ] && [ -d "$MNT/Contents/MacOS" ]; then SRC="$MNT"; fi
  if [ -n "$SRC" ] && ditto "$SRC" "$STAGE" 2>/dev/null &&
      xattr -cr "$STAGE" >/dev/null 2>&1 &&
      codesign --force --deep --sign - "$STAGE" >/dev/null 2>&1 &&
      codesign --verify --deep --strict "$STAGE" >/dev/null 2>&1; then
    rm -rf "$HAVEN_APP.old"
    if mv "$HAVEN_APP" "$HAVEN_APP.old" 2>/dev/null; then
      if ditto "$STAGE" "$HAVEN_APP" 2>/dev/null; then
        OK=1
        rm -rf "$HAVEN_APP.old"
      else
        rm -rf "$HAVEN_APP"
        mv "$HAVEN_APP.old" "$HAVEN_APP"
      fi
    fi
    if [ "$OK" != 1 ]; then
      # Not allowed to modify the folder (e.g. installed by an admin): retry once with a
      # password prompt. Paths containing a quote are not passed to the shell.
      case "$HAVEN_APP$STAGE" in
        *"'"*) ;;
        *)
          PS="$WORK/priv.sh"
          cat > "$PS" <<EOS
#!/bin/sh
rm -rf '$HAVEN_APP.old'
mv '$HAVEN_APP' '$HAVEN_APP.old' || exit 1
if ditto '$STAGE' '$HAVEN_APP'; then
  rm -rf '$HAVEN_APP.old'
  chown -R '$(id -un)' '$HAVEN_APP'
  xattr -cr '$HAVEN_APP'
  exit 0
fi
rm -rf '$HAVEN_APP'
mv '$HAVEN_APP.old' '$HAVEN_APP'
exit 1
EOS
          if osascript -e "do shell script \"/bin/sh '$PS'\" with administrator privileges" >/dev/null 2>&1; then
            OK=1
          fi
          ;;
      esac
    fi
  fi
  hdiutil detach "$MNT" -quiet -force >/dev/null 2>&1
fi
if [ "$OK" = 1 ]; then
  xattr -cr "$HAVEN_APP" >/dev/null 2>&1
  /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$HAVEN_APP" >/dev/null 2>&1
  touch "$HAVEN_APP" >/dev/null 2>&1
fi
rm -rf "$WORK"
if [ "$OK" = 1 ]; then
  rm -f "$HAVEN_DMG"
  open "$HAVEN_APP"
else
  open "$HAVEN_APP"
  open "$HAVEN_DMG"
fi
rm -f "$0"
"#;
    let exe = std::env::current_exe()?;
    let app = exe
        .ancestors()
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("app"))
        .ok_or_else(|| anyhow::anyhow!("not running from an .app bundle"))?
        .to_path_buf();
    let script = std::env::temp_dir().join("haven-update.sh");
    std::fs::write(&script, SCRIPT)?;
    Command::new("/bin/sh")
        .arg(&script)
        .env("HAVEN_PID", std::process::id().to_string())
        .env("HAVEN_DMG", dmg)
        .env("HAVEN_APP", &app)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
}

#[cfg(not(any(windows, target_os = "macos")))]
fn apply_update(_installer: &Path) -> anyhow::Result<()> {
    anyhow::bail!("automatic install is not supported on this platform")
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
