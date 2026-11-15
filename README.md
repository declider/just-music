# Just Music

A tiny music player for Windows. Load a folder, hit play. Nothing else.

## Features

- Folder as playlist (recursive scan), title + artist from tags, cover of the current track
- FLAC, MP3, WAV, OGG/Vorbis, M4A/AAC
- Search by title or artist
- Shuffle, "play next" / "add to queue" via right-click
- Volume and seek sliders
- Tray icon: play/pause, previous, next, show, hide, exit
- Dark theme, compact window

## Build

```sh
cargo build --release
```

Output: `target/release/just_music.exe`.

Run `just_music.exe <folder>` to open a folder at startup (otherwise use "Open folder").

## Audio

Playback is rodio → WASAPI shared mode. No EQ, no filters, no normalisation, no
crossfade: decoded samples go straight to the output (volume slider is a plain
gain multiply, 1.0 = untouched). Windows' shared mixer still resamples to the
device mix format, as it does for every app that isn't in exclusive mode.

## Controls

- Click a track to play it
- Right-click a track: play next / add to queue
- "Hide" in the tray menu hides the window; music keeps playing
- Closing the window exits
