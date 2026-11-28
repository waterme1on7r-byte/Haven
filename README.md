# Haven

Minimal lossless desktop player. Rust · egui (glow) · symphonia · cpal.

## Build & run

### Windows 10/11
1. Install Rust: https://rustup.rs  (choose the MSVC toolchain)
2. Install "Build Tools for Visual Studio" with the *Desktop development with C++* workload.
3. `cargo run --release`  — or `cargo run --release -- "D:\Music\Album"`
4. Binary: `target\release\haven.exe`

### macOS 12+
1. `xcode-select --install`
2. Install Rust: `curl https://sh.rustup.rs -sSf | sh`
3. `cargo run --release`  — or `cargo run --release -- ~/Music/Album`
4. Universal 2 binary (Intel and Apple silicon, targeting macOS 12.0 and later):
   ```
   export MACOSX_DEPLOYMENT_TARGET=12.0
   rustup target add aarch64-apple-darwin x86_64-apple-darwin
   cargo build --release --target aarch64-apple-darwin
   cargo build --release --target x86_64-apple-darwin
   lipo -create -output haven target/aarch64-apple-darwin/release/haven target/x86_64-apple-darwin/release/haven
   ```
5. `.app` bundle + signing: `cargo install cargo-packager` (or cargo-bundle), then codesign + notarize.

## Installers and GitHub builds

Installers can be built locally with `cargo install cargo-packager --locked` followed by
`cargo packager --release`. Windows produces an NSIS setup `.exe`; macOS produces a `.dmg`.
The GitHub Actions workflow builds both formats on their native runners and uploads them as
workflow artifacts. Its macOS DMG contains a Universal 2 app for Intel and Apple silicon, with a
macOS 12.0 minimum deployment target; this includes macOS Ventura 13. Pushing a version tag such
as `v0.1.13` also publishes the installers as a GitHub release. macOS packages are ad-hoc signed
but not notarized until a Developer ID certificate and notarization are configured.
On macOS, Haven disables the native window shadow because the transparent player draws its own
shadow and the system shadow can leave ghosting artifacts during waveform animation. Settings →
Size → Menu Bar switches Haven to a real status item beside the system icons, hides the player
window and Dock icon, and provides playback controls plus Show Haven and Quit Haven menu items.

## macOS notes (hover card, menu bar, icon, DMG)
- The window keeps a permanent transparent strip below the player for the hover metadata card, so showing or hiding the card (and therefore every click and drag) never resizes or moves the window; resizing a transparent window made the player flash on macOS. The strip is click-through, and the card always appears below the player on macOS.
- The Menu Bar status item is retained by Haven (AppKit does not keep it alive) and uses the `music.note` SF Symbol, falling back to a ♫ title.
- CI builds `Haven.icns` with `iconutil` from `assets/haven.iconset`, removes any icon the packager generated and sets `CFBundleIconFile` to `Haven`.
- The DMG contains `Haven.app` and an `Applications` shortcut for drag-to-install.

## Controls
Space play/pause · ←/→ seek 5 s · Shift+←/→ prev/next · ↑/↓ volume · M mute · E equalizer on/off
Drag on waveform to scrub · click waveform to jump · right-click waveform for the transport menu
Progress ring (#ab4354) around the pill: click or drag it to seek, hover for elapsed / total time
Right-click menu ("Mini Mixer"): prev / play-pause / next, a vertical volume fader (tap its speaker to mute), Shuffle / Repeat (cycles off → all → one) / Playlist chips, and an icon footer for Settings, Open files, Open folder, Minimize, Exit. Settings… opens a drawer under the player (same slot as the playlist) with Always on top, Lock position, Size (X-Small / Small / Med / Big; macOS also has Menu Bar) and Wave style (5 live thumbnails); these are remembered between launches, along with the window position
Window: drag the top-edge grip, the frame around the pill (or the gap inside the ring), or hold Alt/Option and drag anywhere. The right-click menu opens at the pointer, downward or — when the screen has too little room below — upward; the player itself never moves.
The playlist opens as a panel attached below the player; it moves with the player and rows select tracks.
Drop files/folders (or Open…) to add them to the playlist — what is playing keeps playing · Playlist → Clear empties the list but keeps the current track playing
Hover anywhere over the player: a metadata card pops up above it (below if there is no room); once the mouse moves it fades after 3 s and returns after the pointer leaves and re-enters

## Equalizer
Settings → Equalizer opens a ten-band panel in the same slot (close returns to Settings). Bands sit one octave apart from 32 Hz to 16 kHz, ±12 dB in half-dB steps. Drag a fader, scroll over it to nudge, double-click to reset it. Presets: Flat, Bass, Vocal, Treble; Custom lights up when the faders match none. The switch in the panel header (or the E key) turns it on/off; on/off and gains are remembered between launches.
The filters (peaking biquads, Q 1.41) run on the decoder thread before audio enters the ring, so the audio callback is untouched. When the EQ is off, or every band is 0 dB, audio is not touched at all and playback stays bit-exact. An automatic preamp cut equal to the largest boost prevents clipping. Changes are heard after about 0.3 s (the ring buffer).

## Architecture
```
UI thread (egui)  ──Cmd (mpsc)──►  decoder thread  ──rtrb ring (~300 ms)──►  cpal callback
      ▲                              symphonia                               pop + volume only
      └──── Shared atomics (pos, dur, vol, flags) + TrackInfo ◄──────────────────┘
waveform thread: decodes file once → 100 amplitude samples/s → RwLock<Vec<f32>> read by UI
```
Gapless: at EOF the decoder opens the next file and keeps writing into the same ring; the stream
is only rebuilt when the sample rate changes.

## Known gaps / next steps
- WASAPI *exclusive* needs a dedicated backend (`wasapi` crate); cpal is shared-mode only. CoreAudio
  hog-mode / device sample-rate switching: add via `coreaudio-rs`.
- WavPack (.wv) and Monkey's Audio (.ape) aren't supported by symphonia → add an optional `ffmpeg-next` decoder behind a feature flag.
- Frameless window has no native resize edges; add a grip using egui viewport commands, or use decorations on Windows.
- Album art is read by lofty but not shown (design has no slot for it).

## Icon
`assets/haven.svg` is the single source. The `.ico`, `.icns`, `haven.iconset/*.png` and `haven-128.rgba` (window icon, embedded in the binary) are rasterised from it with fully transparent corners. Re-export all of them if the SVG changes. The macOS artwork (`haven.iconset/*`, `haven.icns`, `haven-256-mac.rgba`) is the same design scaled to 824/1024 and centred with a soft shadow, matching Apple's icon grid so it is not oversized next to other apps; the Windows `.ico` and `haven-128.rgba` stay full-bleed.

## macOS: "haven is damaged and can't be opened"
CI ad-hoc signs and verifies the app (no Apple Developer account, so no notarization). An app with no valid signature is reported as "damaged" on Apple Silicon; Gatekeeper may still flag browser downloads. Drag Haven.app to Applications, then run once in Terminal:

    xattr -cr /Applications/Haven.app

Open the app itself (Haven.app), not the `haven` file inside `Contents/MacOS`. If it still refuses, sign it locally: `codesign --force --deep --sign - /Applications/Haven.app`.

## Behaviour notes
- Opening a file with Haven while it is already running (e.g. it is your default player and you double-click a song) sends the file to the running window: it replaces the queue and plays. Files that arrive together from a multi-select are added in order.
- Right-clicking the waveform only opens the menu; it never scrubs or seeks (a seek flushes the audio buffer and is audible). Left-click/drag still seeks.
- Mouse wheel over the waveform changes volume (nowhere else); a short read-out shows the level.
- The play button and Space are disabled while nothing is loaded. After the last track ends, one press of play restarts the queue (the window keeps repainting briefly after input so the button never shows a stale state).
- At the end of the queue, the play button switches to Play as soon as the decoder reaches EOF, while the audio callback drains its final buffer. One click restarts the queue.
- Playlist, Settings, and Equalizer panels close after the Haven window stays unfocused for 10 seconds. The timer resets when Haven regains focus.
- Controls ignore the activation click when Haven was unfocused; hover feedback remains visible.
- The hover card goes above the player, or above the Playlist/Settings panel when that panel is docked upward.
- The context menu closes immediately when the window loses focus, and Minimize waits until the window has shrunk back so it restores in the right place.
- Skyline bars stand on a baseline with a short, faded reflection underneath. Wave is a single smooth sine whose height follows loudness. No bar animates on its own: heights come only from the audio.
- Output device: Haven follows the system's default output device. Switch to headphones (or unplug / change the output in the OS) while playing and Haven reopens on the new default within a few hundred milliseconds, resuming from the same point. If the device in use disappears, Haven retries the new default.
- Play/pause sends an explicit target state, not a flip, so extra clicks while the engine is busy cannot cancel each other. At the end of the queue the last ~300 ms plays out without blocking, so a click right after the end restarts the queue immediately.
- Settings shows a small "WTMLJR" signature at the bottom.
- Playlist and Settings dock below the player, or above it when the screen has too little room below (decided when they open; the player never moves).

## Updates (GitHub Releases)
1. `REPO` in `src/update.rs` is `waterme1on7r-byte/Haven`.
2. Bump `version` in `Cargo.toml` (currently 0.1.13), commit, then push a tag that matches it, e.g. `v0.1.14` for the next release. CI refuses to build a tag that differs from the Cargo version and attaches the Windows installer (.exe) and macOS disk image (.dmg) to the release.
3. Running copies check shortly after launch (at most once a day) and via Settings → Updates → Check. When a newer release exists, the Settings icon in the context menu gets a red dot and the Updates row offers **Update**. One click does everything: it downloads the installer (with progress), verifies the size, quits Haven, installs silently and starts the new version.
   - Windows: the NSIS installer is per-user (no administrator prompt) and is run with `/S`. A helper script waits for Haven to exit first, then relaunches Haven.
   - macOS: the helper copies the new Haven.app out of the disk image over the running bundle and reopens it. If that is not possible (no permission, or not running from an .app) it reopens the old app and shows the disk image so you can drag it over yourself.
   - If the automatic install cannot start, the installer is opened as before ("Couldn't install by itself — installer opened").
   - Playback stops while Haven restarts; the queue is not restored.
   - Downloads go to a temporary folder and are deleted after a successful install.
   - A second click on Update, or a background check, can no longer interfere with a download in progress.

The repository must be public (or the release assets reachable without a token) for the check to work.

## Volume
The last used volume is saved (600 ms after the last change) and restored on launch. Scrolling over the waveform or over the context menu changes it; the menu fader shows the level as a number.

## macOS: opening files with Haven
The CI build adds `CFBundleDocumentTypes` (mp3, flac, wav, aif/aiff, m4a, aac, ogg/oga) to Haven.app's `Info.plist`, so Haven shows up under *Open With* and can be set as the default player (Get Info → Open with → Change All). Finder hands files over as an Apple Event, which `src/macos_open.rs` receives and forwards to the running player. The handler is attached before the window exists, so it also works when the file launches Haven.
