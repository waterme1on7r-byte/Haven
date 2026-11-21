# Haven

Haven is a compact, lossless desktop audio player built with Rust. It uses a
custom frameless interface, gapless playback, live waveforms, a playlist, and
persisted window settings.

## Features

- FLAC, MP4/M4A/AAC, MP3, Ogg, WAV, AIFF, and formats supported by Symphonia
- Gapless queue playback with live waveform rendering
- Five selectable waveform styles
- Waveform and progress-ring scrubbing
- Hover metadata card with title, artist, codec, and format details
- Attached playlist with track selection, shuffle, Open, and Clear controls
- Drag-and-drop files or folders; dropped media is appended without interrupting playback
- Small, medium, and big window sizes
- Always-on-top and position-lock controls
- Persistent window size, position, playback, and display preferences

## Controls

| Action | Control |
| --- | --- |
| Play / pause | `Space` |
| Seek backward / forward | `←` / `→` |
| Previous / next track | `Shift+←` / `Shift+→` |
| Volume down / up | `↓` / `↑` |
| Mute | `M` |
| Scrub | Drag or click the waveform or progress ring |
| Transport and playlist menu | Right-click the player |
| Move the player | Drag the frame or top-edge grip |
| Move anywhere | Hold `Alt` on Windows or `Option` on macOS while dragging |

Open adds files or folders to the playlist without replacing the current track.
Clearing the playlist keeps the current track playing.

## Install and run

### Windows

1. Install Rust from https://rustup.rs and select the MSVC toolchain.
2. Install Visual Studio Build Tools with the **Desktop development with C++** workload.
3. Run `cargo run --release`.
4. Optionally open a file or folder:

   ```powershell
   cargo run --release -- "D:\Music\Album"
   ```

The release executable is `target\release\haven.exe`.

### macOS

1. Install command-line tools with `xcode-select --install`.
2. Install Rust with `curl https://sh.rustup.rs -sSf | sh`.
3. Run `cargo run --release`, optionally followed by a file or folder.

Package a native installer with:

```sh
cargo install cargo-packager --locked
cargo packager --release
```

## Releases

The `Build Haven` GitHub Actions workflow builds Windows NSIS installers and
macOS disk images. Pushing a `v*` tag publishes both packages to a GitHub
release, such as `v0.1.3`.

macOS packages are unsigned until Developer ID signing and notarization
credentials are configured.

## Development

```sh
cargo fmt --check
cargo check --locked
cargo build --release
```

The icon source is `assets/haven.svg`. Rasterized `.ico`, `.icns`, iconset,
and `haven-128.rgba` assets should be regenerated whenever the SVG changes.
