# Just Music

A tiny music player for Windows. Pick a folder, hit play. Nothing else.

**English** · [Русский](README.ru.md)

## Features

- Folder as playlist (recursive scan on a worker thread), title + artist from tags, cover of the current track
- FLAC, MP3, WAV, OGG/Vorbis, M4A/AAC
- Search by title or artist in the title bar; the empty box shows track count, scan progress or errors as a faded hint
- Shuffle, "play next" / "add to queue" via right-click
- Volume and seek sliders (volume shows its %); the scrollbar thumb highlights while dragged
- Borderless window with a custom title bar: folder, search, minimise / maximise / close
- Player bar: cover, title/artist, transport + shuffle, seek, volume
- Tray icon: play/pause, previous, next, show, hide, exit
- Dark theme, compact layout
- Native Win32/GDI, no runtime, no telemetry

## Download

Grab `just_music.exe` from the [latest release](../../releases/latest) and run it. No installer, no runtime.

## Build

```sh
cargo build --release
```

Output: `target/release/just_music.exe` (~2.3 MB).

Run `just_music.exe <folder>` to open a folder at startup (otherwise use the folder button).

### App icon

Drop an `icon.ico` next to `just_music.exe` and it is used for the window, taskbar and
tray (restart the app after replacing it). Without it, a built-in 32×32 icon is drawn.
Explorer's file icon is taken from the executable's embedded resources, so changing that
one needs an icon resource in the build, not just the file.

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

## License

MIT — see [LICENSE](LICENSE).
