# Haven

Haven is a compact, frameless desktop audio player for Windows and macOS. It keeps playback controls, waveform seeking, queue management, and system window controls in a small floating player.

![Haven player with playlist](docs/screens/player-playlist.png)

## Features

- Plays FLAC, ALAC/M4A, WAV, AIFF, MP3, AAC, OGG/OGA, and supported audio in MP4 containers.
- Opens individual files or scans a folder into a queue.
- Accepts file and folder drag and drop. Hold `Shift` while dropping to append instead of replacing the queue.
- Renders a waveform from the media itself, with click-to-seek and drag-to-scrub controls.
- Shows elapsed and remaining time around the active playhead.
- Supports play/pause, previous, next, volume, mute, shuffle, repeat off, repeat all, and repeat one.
- Includes a playlist attached beneath the player, with current-track details, row selection, scrolling, and Clear/Close actions.
- Provides small, medium, and big player sizes. Big is the original size.
- Supports Always on top, lock position, minimize, and exit from the right-click menu.
- Dims to 50% opacity when the player loses focus and returns to full opacity when it regains focus.
- Keeps one Haven process running at a time.
- Uses the Signal app icon in the player window and packaged app assets.

## Screens

### Player and playlist

The playlist stays attached to the player and moves with it. It contains playback details, selectable queue rows, and Clear and Close controls.

![Player and playlist](docs/screens/player-playlist.png)

### Context menu

Right-click anywhere in the player or playlist to access transport, queue, volume, playback, size, window, and file actions. The player shifts upward when the menu would extend beyond the bottom of the screen.

![Context menu](docs/screens/context-menu.png)

### Waveform

The waveform uses grey for played audio and white for upcoming audio, with a red playhead. Click or drag the waveform to seek.

![Waveform](docs/screens/waveform.png)

## Controls

| Action | Control |
| --- | --- |
| Play / pause | `Space` or the center button |
| Seek | Click or drag the waveform |
| Seek 5 seconds | `Left` / `Right` |
| Previous / next track | `Shift` + `Left` / `Right` |
| Volume | `Up` / `Down`, or the context-menu slider |
| Mute | `M` or the context menu |
| Move the player | Drag empty player space |
| Add media | Drop files or folders; hold `Shift` to append |
| Open options | Right-click anywhere in Haven |

## Playlist

Open **Playlist** from the context menu. The panel appears 5 px below the player and includes:

- the playing track title, artist or format, and playback time;
- every queued track with its title, artist, and duration;
- an active-track highlight;
- scroll support for longer queues;
- **Clear**, which stops playback and empties the queue; and
- **Close**, which hides the playlist without changing the queue.

## Window and menu options

The context menu contains:

- Previous and Next
- Playlist / Hide playlist
- Volume and Mute
- Shuffle
- Repeat off, Repeat all, and Repeat one
- Small, Medium, and Big sizes
- Always on top
- Lock position
- Open files and Open folder
- Minimize and Exit

Haven saves the player size, position, Always on top state, and Lock position state in the platform application-data directory.

## Download app

### Check releases 

Windows packaging produces an NSIS installer. macOS packaging produces a DMG. A macOS package must be signed and notarized with an Apple Developer ID before public distribution.

## Architecture

```text
egui UI ── commands ──► decoder thread ── lock-free audio ring ──► cpal output callback
   ▲                         symphonia decoder                     pop + volume only
   └──── shared playback state, track metadata, and waveform ◄──────────────────────┘
```

Waveform extraction runs in the background and produces 100 amplitude samples per second. The player preserves the audio stream between consecutive tracks where possible; it rebuilds the stream only when the output sample rate changes.

## Current limits

- Audio playback uses shared-mode system audio through cpal.
- WavPack (`.wv`) and Monkey's Audio (`.ape`) are not decoded.
- Album artwork is read from metadata but is not displayed in the current player design.
