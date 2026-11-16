# Just Music

A tiny music player for Windows. Load a folder, hit play. Nothing else.

## Features

- Folder as playlist (recursive scan, on a worker thread), title + artist from tags, cover of the current track
- FLAC, MP3, WAV, OGG/Vorbis, M4A/AAC
- Search by title or artist in the title bar
- Shuffle, "play next" / "add to queue" via right-click
- Volume and seek sliders (volume shows its %)
- Borderless window with custom title bar: folder, search, minimise / maximise / close
- Player bar: cover, title/artist, transport + shuffle, seek, volume
- Tray icon: play/pause, previous, next, show, hide, exit
- Dark theme, compact layout

## Build

```sh
cargo build --release
```

Output: `target/release/just_music.exe` (~2.3 MB). Native Win32/GDI UI, no runtime.

Run `just_music.exe <folder>` to open a folder at startup (otherwise use the folder button).

## Settings

One file: `%APPDATA%\JustMusic\settings.ini` — last folder, volume, shuffle.

## Controls

- Double-click a track to play it; single click just hovers
- Right-click a track: play next / add to queue
- Drag the window by its title bar or the player area
- Long titles scroll horizontally in the player bar
- Minimize button / tray "Hide": window goes to the taskbar, music keeps playing
- Window X: hides the window (and its taskbar entry), music keeps playing
- Tray icon: click to show; right-click for play/pause, previous, next, show, hide, exit
- Exit is only from the tray menu ("Exit")
- Space play/pause, arrows seek ±5 s, up/down volume

## Audio

Playback is rodio → WASAPI shared mode. No EQ, no filters, no normalisation, no
crossfade: decoded samples go straight to the output (volume slider is a plain
gain multiply, 1.0 = untouched). Windows' shared mixer still resamples to the
device mix format, as it does for every app that isn't in exclusive mode.
