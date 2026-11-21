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
workflow artifacts. Pushing a version tag such as `v0.1.5` also publishes the installers as a
GitHub release. macOS packages are unsigned until a Developer ID certificate and notarization
are configured.

## Controls
Space play/pause · ←/→ seek 5 s · Shift+←/→ prev/next · ↑/↓ volume · M mute
Drag on waveform to scrub · click waveform to jump · right-click waveform for the transport menu
Progress ring (#ab4354) around the pill: click or drag it to seek, hover for elapsed / total time
Right-click menu also has Playlist, Size (S / M / B), Wave style (1–5), Minimize, Exit, Always on top and Lock position (these settings are remembered between launches, along with the window position)
Window: drag the top-edge grip, the frame around the pill (or the gap inside the ring), or hold Alt/Option and drag anywhere. The right-click menu opens at the pointer, downward or — when the screen has too little room below — upward; the player itself never moves.
The playlist opens as a panel attached below the player; it moves with the player and rows select tracks.
Drop files/folders (or Open…) to add them to the playlist — what is playing keeps playing · Playlist → Clear empties the list but keeps the current track playing
Hover anywhere over the player: a metadata card pops up above it (below if there is no room); once the mouse moves it fades after 3 s and returns after the pointer leaves and re-enters

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
`assets/haven.svg` is the single source. The `.ico`, `.icns`, `haven.iconset/*.png` and `haven-128.rgba` (window icon, embedded in the binary) are rasterised from it with fully transparent corners. Re-export all of them if the SVG changes.

## macOS: "haven is damaged and can't be opened"
CI ad-hoc signs and verifies the app (no Apple Developer account, so no notarization). An app with no valid signature is reported as "damaged" on Apple Silicon; Gatekeeper may still flag browser downloads. Drag Haven.app to Applications, then run once in Terminal:

    xattr -cr /Applications/Haven.app

Open the app itself (Haven.app), not the `haven` file inside `Contents/MacOS`. If it still refuses, sign it locally: `codesign --force --deep --sign - /Applications/Haven.app`.
