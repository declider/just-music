//! Track scanning, tags, cover art and the single settings file.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::UNIX_EPOCH,
};

use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::Accessor,
};

pub const AUDIO_EXT: &[&str] = &["mp3", "flac", "wav", "ogg", "m4a", "aac", "mp4"];

#[derive(Clone)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub duration: f64,
    /// mtime + size fingerprint; cached entries are reused while it matches.
    pub stamp: u64,
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
        stamp: file_stamp(&path),
        path,
        title,
        artist,
        duration,
    }
}

/// Cheap change detector for a file, so unchanged tracks skip tag parsing.
fn file_stamp(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    (meta.len().wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ mtime) | 1
}

/// Recursively collects playable files. `count` lets the UI show progress
/// while a big folder is being indexed on another thread. Tracks whose stamp
/// still matches a cached entry are taken from `reuse` instead of being re-read.
pub fn scan(dir: &Path, out: &mut Vec<Track>, reuse: &mut HashMap<PathBuf, Track>, count: &AtomicUsize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, out, reuse, count);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && AUDIO_EXT.contains(&ext.to_ascii_lowercase().as_str())
        {
            let track = match reuse.get(&path) {
                Some(old) if old.stamp == file_stamp(&path) => old.clone(),
                _ => read_track(path),
            };
            out.push(track);
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
        app_dir().join("settings.ini")
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

fn app_dir() -> PathBuf {
    match std::env::var_os("APPDATA") {
        Some(base) => PathBuf::from(base).join("JustMusic"),
        None => PathBuf::from("."),
    }
}

// ------------------------------------------------------------- scan cache
//
// Tags for the last folder live in one text file next to settings.ini, so a
// restart shows the list immediately and the background rescan only reads
// files whose mtime/size changed.

const CACHE_VERSION: &str = "v1";

/// Cached track list for `folder`, empty when the cache belongs to another folder.
pub fn load_cache(folder: &Path) -> Vec<Track> {
    read_cache(&app_dir().join("library.cache"), folder)
}

pub fn save_cache(folder: &Path, tracks: &[Track]) {
    let dir = app_dir();
    let _ = std::fs::create_dir_all(&dir);
    write_cache(&dir.join("library.cache"), folder, tracks);
}

fn write_cache(path: &Path, folder: &Path, tracks: &[Track]) {
    let mut text = String::with_capacity(tracks.len() * 96);
    text.push_str(CACHE_VERSION);
    text.push('\t');
    text.push_str(&esc(&folder.to_string_lossy()));
    text.push('\n');
    for t in tracks {
        // Paths that cannot round-trip through UTF-8 are left out; the scan
        // re-reads them from disk anyway.
        let Some(p) = t.path.to_str() else {
            continue;
        };
        text.push_str(&format!(
            "{:x}\t{:.3}\t{}\t{}\t{}\n",
            t.stamp,
            t.duration,
            esc(p),
            esc(&t.title),
            esc(&t.artist)
        ));
    }
    let _ = std::fs::write(path, text);
}

fn read_cache(path: &Path, folder: &Path) -> Vec<Track> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines = text.lines();
    let Some((version, stored)) = lines.next().and_then(|l| l.split_once('\t')) else {
        return Vec::new();
    };
    if version != CACHE_VERSION || unesc(stored) != folder.to_string_lossy() {
        return Vec::new();
    }

    lines
        .filter_map(|line| {
            let mut f = line.split('\t');
            let stamp = u64::from_str_radix(f.next()?, 16).ok()?;
            let duration = f.next()?.parse().ok()?;
            let path = PathBuf::from(unesc(f.next()?));
            let title = unesc(f.next()?);
            let artist = unesc(f.next()?);
            Some(Track {
                path,
                title,
                artist,
                duration,
                stamp,
            })
        })
        .collect()
}

fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unesc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, title: &str, artist: &str) -> Track {
        Track {
            path: PathBuf::from(path),
            title: title.into(),
            artist: artist.into(),
            duration: 61.5,
            stamp: 0xdead_beef,
        }
    }

    #[test]
    fn cache_roundtrip_survives_tabs_and_newlines() {
        let file = std::env::temp_dir().join("just_music_cache_test.cache");
        let folder = Path::new("D:\\Music");
        let tracks = vec![
            track("D:\\Music\\a.mp3", "A\tB", "C\\D"),
            track("D:\\Music\\b.mp3", "line\nbreak", ""),
        ];
        write_cache(&file, folder, &tracks);
        let back = read_cache(&file, folder);
        let wrong_folder = read_cache(&file, Path::new("D:\\Other"));
        let _ = std::fs::remove_file(&file);

        assert_eq!(back.len(), 2);
        assert_eq!(back[0].path, tracks[0].path);
        assert_eq!(back[0].title, "A\tB");
        assert_eq!(back[0].artist, "C\\D");
        assert_eq!(back[1].title, "line\nbreak");
        assert_eq!(back[1].stamp, 0xdead_beef);
        assert!(wrong_folder.is_empty());
        assert!(read_cache(&file, folder).is_empty());
    }
}
