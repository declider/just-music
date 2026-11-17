# Just Music

Крошечный музыкальный плеер для локальных файлов на Windows. Без излишков.

[English](README.md) · **Русский**

## Скачать

`just_music.exe` лежит в [последнем релизе](../../releases/latest). Без установщика и без рантайма.

## Сборка

```sh
cargo build --release
```

Результат: `target/release/just_music.exe` (~2,3 МБ).

Запуск `just_music.exe <папка>` сразу открывает папку (иначе — кнопка с папкой в заголовке).

## Настройки

Один файл: `%APPDATA%\JustMusic\settings.ini` — последняя папка, громкость, shuffle.

## Автор

Навайбкожено с помощью DeepSeek v4.1 1017

## Лицензия

MIT — см. [LICENSE](LICENSE).
