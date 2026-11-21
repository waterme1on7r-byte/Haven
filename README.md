# Haven

Haven is a compact, lossless desktop audio player built with Rust. Its frameless
interface combines gapless playback, live waveforms, drag-and-drop loading,
track metadata, and a hover card into one small native window.

## Features

- FLAC, MP4/M4A/AAC, MP3, Ogg, WAV, AIFF, and formats supported by Symphonia
- Gapless queue playback with live waveform rendering
- Waveform scrubbing and progress-ring seeking
- Hover metadata card for title, artist, codec, format, and elapsed playback
- Drag-to-scrub and drag-to-move behavior that avoids unwanted window drags
- Attached playlist with track selection and queue clearing
- Small, medium, and big window sizes
- Always-on-top, position lock, and persistent window settings
- Drag-and-drop files or folders; Shift+drop appends to the queue

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

Hover over the waveform while paused or playing to see the track metadata card.
The right-click menu controls the playlist, player size, minimize, exit,
always-on-top, and position lock.

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

1. Install the command-line tools with `xcode-select --install`.
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
release, such as `v0.1.2`.

macOS packages are unsigned until Developer ID signing and notarization
credentials are configured.

## Development

```sh
cargo fmt --check
cargo check --locked
cargo build --release
```
