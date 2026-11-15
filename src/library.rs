//! Track scanning, tags, cover art and the single settings file.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::Accessor,
};

pub const AUDIO_EXT: &[&str] = &["mp3", "flac", "wav", "ogg", "m4a", "aac", "mp4"];

pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub duration: f64,
}

pub fn read_track(path: PathBuf) -> Track {
    let mut title = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut artist = String::new();
    let mut duration = 0.0;

    if let Ok(file) = lofty::read_from_path(&path) {
        duration = file.properties().duration().as_secs_f64();
        if let Some(tag) = file.primary_tag().or_else(|| file.first_tag()) {
            if let Some(t) = tag.title() {
                title = t.to_string();
            }
            if let Some(a) = tag.artist() {
                artist = a.to_string();
            }
        }
    }

    Track {
        path,
        title,
        artist,
        duration,
    }
}

/// Recursively collects playable files. `count` lets the UI show progress
/// while a big folder is being indexed on another thread.
pub fn scan(dir: &Path, out: &mut Vec<Track>, count: &AtomicUsize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, out, count);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && AUDIO_EXT.contains(&ext.to_ascii_lowercase().as_str())
        {
            out.push(read_track(path));
            count.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// RGBA thumbnail of the embedded cover, if any.
pub fn cover_rgba(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let file = lofty::read_from_path(path).ok()?;
    let pic = file
        .primary_tag()
        .or_else(|| file.first_tag())?
        .pictures()
        .first()?;
    let img = image::load_from_memory(pic.data())
        .ok()?
        .thumbnail(96, 96)
        .to_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

pub struct Settings {
    pub folder: Option<PathBuf>,
    pub volume: f32,
    pub shuffle: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folder: None,
            volume: 0.7,
            shuffle: false,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        match std::env::var_os("APPDATA") {
            Some(base) => PathBuf::from(base).join("JustMusic").join("settings.ini"),
            None => PathBuf::from("settings.ini"),
        }
    }

    pub fn load() -> Self {
        let mut s = Self::default();
        let Ok(text) = std::fs::read_to_string(Self::path()) else {
            return s;
        };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim();
            match key.trim() {
                "folder" if !value.is_empty() => s.folder = Some(PathBuf::from(value)),
                "volume" => s.volume = value.parse().unwrap_or(s.volume),
                "shuffle" => s.shuffle = value == "1" || value == "true",
                _ => {}
            }
        }
        s.volume = s.volume.clamp(0.0, 1.0);
        s
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = format!(
            "folder={}\nvolume={:.2}\nshuffle={}\n",
            self.folder
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            self.volume,
            u8::from(self.shuffle),
        );
        let _ = std::fs::write(path, text);
    }
}
