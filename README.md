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
4. Universal binary:
   ```
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
workflow artifacts. Pushing a version tag such as `v0.1.0` also publishes the installers as a
GitHub release. macOS packages are unsigned until a Developer ID certificate and notarization
are configured.

## Controls
Space play/pause · ←/→ seek 5 s · Shift+←/→ prev/next · ↑/↓ volume · M mute
Drag on waveform to scrub · click waveform to jump · right-click waveform for the transport menu
Progress ring (#ab4354) around the pill: click or drag it to seek, hover for elapsed / total time
Right-click menu also has Playlist, Always on top and Lock position (the last two are remembered between launches, along with the window position)
Window: drag the frame around the pill (or the gap inside the ring), or hold Alt/Option and drag anywhere. Size (small / medium / big) is in the right-click menu and is remembered.
The playlist opens as a panel attached below the player; it moves with the player and rows select tracks.
Drop files/folders to play · Shift+drop to append · hover waveform for track/format details

## Architecture
```
UI thread (egui)  ──Cmd (mpsc)──►  decoder thread  ──rtrb ring (~300 ms)──►  cpal callback
      ▲                              symphonia                               pop + volume only
      └──── Shared atomics (pos, dur, vol, flags) + TrackInfo ◄──────────────────┘
waveform thread: decodes file once → 10 peaks/s → RwLock<Vec<f32>> read by UI
```
Gapless: at EOF the decoder opens the next file and keeps writing into the same ring; the stream
is only rebuilt when the sample rate changes.

## Known gaps / next steps
- WASAPI *exclusive* needs a dedicated backend (`wasapi` crate); cpal is shared-mode only. CoreAudio
  hog-mode / device sample-rate switching: add via `coreaudio-rs`.
- WavPack (.wv) and Monkey's Audio (.ape) aren't supported by symphonia → add an optional `ffmpeg-next` decoder behind a feature flag.
- Frameless window has no native resize edges; add a grip using egui viewport commands, or use decorations on Windows.
- No queue/library view yet (the reference design has none); candidates: a "Queue" submenu in the right-click menu.
- Album art is read by lofty but not shown (design has no slot for it).
