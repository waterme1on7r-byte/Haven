# Haven

Haven is a compact, lossless desktop audio player built with Rust. It combines a custom
frameless interface, gapless playback, live waveforms, and persistent window settings in a
small native application.

## Features

- FLAC, MP4/M4A/AAC, MP3, Ogg, WAV, AIFF, and other formats supported by Symphonia
- Gapless queue playback with no per-track ring-buffer rebuild for compatible sample rates
- Waveform scrubbing and seeking, with hover details for title, artist, album, codec, and format
- Attached playlist, album-folder scanning, track metadata, and waveform overview
- Small, medium, and big player sizes
- Always-on-top and position-lock controls
- Drag-and-drop files or folders; Shift+drop appends to the existing queue
- Remembers window size, position, always-on-top state, and position lock

## Controls

| Action | Control |
| --- | --- |
| Play / pause | `Space` |
| Seek backward / forward | `←` / `→` |
| Previous / next track | `Shift+←` / `Shift+→` |
| Volume down / up | `↓` / `↑` |
| Mute | `M` |
| Scrub | Drag or click the waveform |
| Transport and playlist menu | Right-click the player |
| Move the player | Drag the frame or the top-edge grip |
| Move anywhere | Hold `Alt` on Windows or `Option` on macOS while dragging |

The command menu also controls player size, playlist visibility, minimizing, exiting,
always-on-top, and position lock.

## Install and run

### Windows

1. Install Rust from https://rustup.rs and choose the MSVC toolchain.
2. Install the Visual Studio Build Tools **Desktop development with C++** workload.
3. Run `cargo run --release`.
4. Open a file or folder from the command line:

   ```powershell
   cargo run --release -- "D:\Music\Album"
   ```

The release executable is `target\release\haven.exe`.

### macOS

1. Install the command-line tools with `xcode-select --install`.
2. Install Rust with `curl https://sh.rustup.rs -sSf | sh`.
3. Run `cargo run --release`, or pass a file or folder:

   ```sh
   cargo run --release -- ~/Music/Album
   ```

Haven can also be packaged as a native installer:

```sh
cargo install cargo-packager --locked
cargo packager --release
```

## Releases

The `Build Haven` GitHub Actions workflow builds Windows NSIS installers and macOS disk
images on their respective platforms. Pushing a `v*` tag publishes both packages to a
GitHub release (for example, `v0.1.1`).

macOS packages are unsigned until Developer ID signing and notarization credentials are
configured.

## Development

```sh
cargo fmt --check
cargo check --locked
cargo build --release
```

The app uses egui with glow for the UI, Symphonia for decoding and metadata, cpal for audio
output, and a lock-free ring buffer for streaming audio between the decoder and callback
threads.
