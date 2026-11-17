# Just Music

A tiny music player for local files on Windows. Nothing extra.

**English** · [Русский](README.ru.md)

## Download

`just_music.exe` is in the [latest release](../../releases/latest). No installer, no runtime.

## Build

```sh
cargo build --release
```

Output: `target/release/just_music.exe` (~2.3 MB).

Run `just_music.exe <folder>` to open a folder at startup (otherwise use the folder button).

## Settings

One file: `%APPDATA%\JustMusic\settings.ini` — last folder, volume, shuffle.

## Author

Vibe-coded with DeepSeek v4.1 1017

## License

MIT — see [LICENSE](LICENSE).
