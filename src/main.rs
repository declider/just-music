#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod player;

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use eframe::egui::{
    self, Align2, Color32, FontId, Rect, RichText, Sense, ThemePreference, Vec2, pos2, vec2,
};
use lofty::{
    file::{AudioFile, TaggedFileExt},
    tag::Accessor,
};
use player::Audio;
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

const AUDIO_EXT: &[&str] = &["mp3", "flac", "wav", "ogg", "m4a", "aac", "mp4"];
const ROW_H: f32 = 26.0;
const ACCENT: Color32 = Color32::from_rgb(96, 165, 255);

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([360.0, 520.0])
            .with_min_inner_size([300.0, 360.0])
            .with_title("Just Music")
            .with_icon(Arc::new(app_icon_data())),
        ..Default::default()
    };
    eframe::run_native(
        "Just Music",
        options,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    )
}

// ---------------------------------------------------------------- library

struct Track {
    path: PathBuf,
    title: String,
    artist: String,
    duration: f64,
}

fn read_track(path: PathBuf) -> Track {
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

fn scan_dir(dir: &Path, out: &mut Vec<Track>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, out);
        } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_ascii_lowercase();
            if AUDIO_EXT.contains(&ext.as_str()) {
                out.push(read_track(path));
            }
        }
    }
}

// ---------------------------------------------------------------- playback order

fn sequential_next(len: usize, current: Option<usize>, wrap: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match current {
        None => Some(0),
        Some(c) if c + 1 < len => Some(c + 1),
        Some(_) if wrap => Some(0),
        Some(_) => None,
    }
}

fn shuffled_next(len: usize, current: usize, rand: u64) -> usize {
    if len < 2 {
        return 0;
    }
    let mut n = rand as usize % (len - 1);
    if n >= current {
        n += 1;
    }
    n
}

fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

// ---------------------------------------------------------------- app

#[derive(Clone, Copy, PartialEq)]
enum TrayAction {
    PlayPause,
    Next,
    Prev,
    Show,
    Hide,
    Exit,
}

struct App {
    audio: Option<Audio>,
    error: Option<String>,

    tracks: Vec<Track>,
    visible: Vec<usize>,
    search: String,

    current: Option<usize>,
    playing: bool,
    pos: f64,
    seeking: bool,
    volume: f32,
    shuffle: bool,
    queue: VecDeque<usize>,
    show_queue: bool,

    cover: Option<egui::TextureHandle>,
    rng: u64,

    tray: Option<Tray>,
    tray_label_playing: bool,
    tray_tooltip: String,
}

struct Tray {
    icon: TrayIcon,
    play_pause: MenuItem,
    next: MenuItem,
    prev: MenuItem,
    show: MenuItem,
    hide: MenuItem,
    exit: MenuItem,
}

impl Tray {
    fn new() -> Option<Self> {
        let play_pause = MenuItem::new("Play", true, None);
        let prev = MenuItem::new("Previous", true, None);
        let next = MenuItem::new("Next", true, None);
        let show = MenuItem::new("Show", true, None);
        let hide = MenuItem::new("Hide", true, None);
        let exit = MenuItem::new("Exit", true, None);

        let menu = Menu::new();
        let _ = menu.append_items(&[
            &play_pause,
            &prev,
            &next,
            &PredefinedMenuItem::separator(),
            &show,
            &hide,
            &PredefinedMenuItem::separator(),
            &exit,
        ]);

        let (rgba, w, h) = icon_pixels();
        let icon = TrayIconBuilder::new()
            .with_id("just-music")
            .with_tooltip("Just Music")
            .with_icon(tray_icon::Icon::from_rgba(rgba, w, h).ok()?)
            .with_menu(Box::new(menu))
            .build()
            .ok()?;

        Some(Self {
            icon,
            play_pause,
            next,
            prev,
            show,
            hide,
            exit,
        })
    }
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();
        ctx.set_theme(ThemePreference::Dark);
        ctx.all_styles_mut(|s| {
            s.spacing.item_spacing = vec2(6.0, 6.0);
            s.spacing.button_padding = vec2(8.0, 3.0);
        });

        // Keep `logic` running (seek bar, track advance, tray) even while the
        // window is hidden. A 200 ms tick is plenty and costs nothing.
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(200));
                ctx.request_repaint();
            }
        });

        let audio = match Audio::open() {
            Ok(a) => Some(a),
            Err(e) => {
                eprintln!("audio device: {e}");
                None
            }
        };

        let mut app = Self {
            audio,
            error: None,
            tracks: Vec::new(),
            visible: Vec::new(),
            search: String::new(),
            current: None,
            playing: false,
            pos: 0.0,
            seeking: false,
            volume: 0.7,
            shuffle: false,
            queue: VecDeque::new(),
            show_queue: false,
            cover: None,
            rng: seed(),
            tray: Tray::new(),
            tray_label_playing: false,
            tray_tooltip: String::new(),
        };

        // `just_music.exe <folder>` opens that folder right away.
        if let Some(dir) = std::env::args().nth(1) {
            let dir = PathBuf::from(dir);
            if dir.is_dir() {
                app.load_folder(dir);
            }
        }

        app
    }

    fn load_folder(&mut self, dir: PathBuf) {
        let mut tracks = Vec::new();
        scan_dir(&dir, &mut tracks);
        tracks.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        self.tracks = tracks;
        self.search.clear();
        self.refilter();
    }

    fn refilter(&mut self) {
        let q = self.search.trim().to_lowercase();
        self.visible = if q.is_empty() {
            (0..self.tracks.len()).collect()
        } else {
            self.tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| {
                    t.title.to_lowercase().contains(&q) || t.artist.to_lowercase().contains(&q)
                })
                .map(|(i, _)| i)
                .collect()
        };
    }

    fn rng_next(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn play_index(&mut self, idx: usize, ctx: &egui::Context) {
        let Some(track) = self.tracks.get(idx) else {
            return;
        };
        let path = track.path.clone();
        let title = track.title.clone();

        let Some(audio) = &self.audio else {
            self.error = Some("No audio output device".into());
            return;
        };
        match audio.play(&path, self.volume) {
            Ok(()) => {
                self.current = Some(idx);
                self.pos = 0.0;
                self.playing = true;
                self.error = None;
                self.load_cover(ctx);
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                    "{title} — Just Music"
                )));
            }
            Err(e) => {
                // Broken file: stop instead of looping over the whole folder.
                self.current = None;
                self.playing = false;
                self.error = Some(format!("{title}: {e}"));
            }
        }
    }

    fn pick_next(&mut self, wrap: bool) -> Option<usize> {
        if let Some(i) = self.queue.pop_front() {
            return Some(i);
        }
        let len = self.tracks.len();
        if self.shuffle
            && let Some(cur) = self.current
        {
            let rand = self.rng_next();
            return Some(shuffled_next(len, cur, rand));
        }
        sequential_next(len, self.current, wrap)
    }

    fn next_track(&mut self, wrap: bool, ctx: &egui::Context) {
        match self.pick_next(wrap) {
            Some(i) => self.play_index(i, ctx),
            None => {
                if let Some(a) = &self.audio {
                    a.stop();
                }
                self.playing = false;
                self.pos = 0.0;
            }
        }
    }

    fn prev_track(&mut self, ctx: &egui::Context) {
        // Same behaviour as every other player: restart first, then step back.
        if self.pos > 3.0 {
            self.seek(0.0);
            return;
        }
        let len = self.tracks.len();
        let Some(cur) = self.current else {
            if len > 0 {
                self.play_index(0, ctx);
            }
            return;
        };
        let idx = if cur > 0 { cur - 1 } else { len.saturating_sub(1) };
        if len > 0 {
            self.play_index(idx, ctx);
        }
    }

    fn seek(&mut self, secs: f64) {
        self.pos = secs;
        if let Some(a) = &self.audio {
            a.seek(Duration::from_secs_f64(secs));
        }
    }

    fn toggle_play(&mut self, ctx: &egui::Context) {
        if self.audio.is_none() {
            return;
        }
        let finished = self.audio.as_ref().is_some_and(|a| a.finished());
        // Nothing loaded, or the last track already ran to the end.
        if self.current.is_none() || finished {
            if !self.tracks.is_empty() {
                self.next_track(true, ctx);
            }
            return;
        }
        let audio = self.audio.as_ref().unwrap();
        if audio.is_paused() {
            audio.resume();
        } else {
            audio.pause();
        }
        self.playing = !audio.is_paused();
    }

    fn load_cover(&mut self, ctx: &egui::Context) {
        self.cover = None;
        let Some(track) = self.current.and_then(|i| self.tracks.get(i)) else {
            return;
        };
        let Ok(file) = lofty::read_from_path(&track.path) else {
            return;
        };
        let Some(pic) = file
            .primary_tag()
            .or_else(|| file.first_tag())
            .and_then(|t| t.pictures().first())
        else {
            return;
        };
        let Ok(img) = image::load_from_memory(pic.data()) else {
            return;
        };
        let img = img.thumbnail(96, 96).to_rgba8();
        let size = [img.width() as usize, img.height() as usize];
        let pixels = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
        self.cover = Some(ctx.load_texture("cover", pixels, egui::TextureOptions::LINEAR));
    }

    fn tick(&mut self, ctx: &egui::Context) {
        let finished = self
            .audio
            .as_ref()
            .is_some_and(|a| self.current.is_some() && a.finished());
        if finished {
            self.next_track(false, ctx);
            return;
        }
        if self.current.is_some()
            && !self.seeking
            && let Some(a) = &self.audio
        {
            self.pos = a.position().as_secs_f64();
        }
        if let Some(a) = &self.audio {
            self.playing = !a.is_paused();
        }
    }

    fn poll_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else {
            return;
        };

        let mut actions = Vec::new();
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            let id = &ev.id;
            let action = if id == tray.play_pause.id() {
                TrayAction::PlayPause
            } else if id == tray.next.id() {
                TrayAction::Next
            } else if id == tray.prev.id() {
                TrayAction::Prev
            } else if id == tray.show.id() {
                TrayAction::Show
            } else if id == tray.hide.id() {
                TrayAction::Hide
            } else if id == tray.exit.id() {
                TrayAction::Exit
            } else {
                continue;
            };
            actions.push(action);
        }

        for action in actions {
            match action {
                TrayAction::PlayPause => self.toggle_play(ctx),
                TrayAction::Next => self.next_track(true, ctx),
                TrayAction::Prev => self.prev_track(ctx),
                TrayAction::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayAction::Hide => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                TrayAction::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }

        // Keep the tray in sync with playback state / current track.
        if self.tray_label_playing != self.playing {
            self.tray_label_playing = self.playing;
            if let Some(tray) = &self.tray {
                tray.play_pause
                    .set_text(if self.playing { "Pause" } else { "Play" });
            }
        }
        let tooltip = match self.current.and_then(|i| self.tracks.get(i)) {
            Some(t) if t.artist.is_empty() => t.title.clone(),
            Some(t) => format!("{} — {}", t.artist, t.title),
            None => String::new(),
        };
        if tooltip != self.tray_tooltip {
            self.tray_tooltip = tooltip;
            if let Some(tray) = &self.tray {
                let tip = if self.tray_tooltip.is_empty() {
                    "Just Music".to_string()
                } else {
                    self.tray_tooltip.clone()
                };
                let _ = tray.icon.set_tooltip(Some(tip));
            }
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_tray(ctx);
        self.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                if ui.button("Open folder").clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    self.load_folder(dir);
                }
                if ui
                    .selectable_label(self.shuffle, "Shuffle")
                    .on_hover_text("Play tracks in random order")
                    .clicked()
                {
                    self.shuffle = !self.shuffle;
                }
                if !self.queue.is_empty()
                    && ui.button(format!("Queue {}", self.queue.len())).clicked()
                {
                    self.show_queue = true;
                }
            });
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search title or artist")
                    .desired_width(f32::INFINITY),
            );
            if search.changed() {
                self.refilter();
            }
            ui.add_space(2.0);
        });

        egui::Panel::bottom("player").show(ui, |ui| {
            ui.add_space(4.0);
            self.player_bar(ui);
            ui.add_space(4.0);
        });

        if let Some(err) = &self.error {
            egui::Panel::bottom("error").show(ui, |ui| {
                ui.label(RichText::new(err).color(Color32::LIGHT_RED).size(11.0));
            });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if self.tracks.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("Open a folder to start").weak().size(13.0));
                });
                return;
            }
            if self.visible.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label(RichText::new("No matches").weak().size(13.0));
                });
                return;
            }

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW_H, self.visible.len(), |ui, range| {
                    for row in range {
                        let idx = self.visible[row];
                        let is_current = self.current == Some(idx);
                        let resp = track_row(ui, &self.tracks[idx], is_current);
                        if resp.clicked() {
                            self.play_index(idx, &ctx);
                        }
                        resp.context_menu(|ui| {
                            if ui.button("Play next").clicked() {
                                self.queue.push_front(idx);
                                ui.close();
                            }
                            if ui.button("Add to queue").clicked() {
                                self.queue.push_back(idx);
                                ui.close();
                            }
                        });
                    }
                });
        });

        if self.show_queue {
            self.queue_window(&ctx);
        }
    }
}

impl App {
    fn player_bar(&mut self, ui: &mut egui::Ui) {
        let (title, artist, dur) = match self.current.and_then(|i| self.tracks.get(i)) {
            Some(t) => (t.title.clone(), t.artist.clone(), t.duration),
            None => ("Nothing playing".to_string(), String::new(), 0.0),
        };

        ui.horizontal(|ui| {
            match &self.cover {
                Some(tex) => {
                    let size = Vec2::splat(46.0);
                    ui.image(egui::load::SizedTexture::new(tex.id(), size));
                }
                None => {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::hover());
                    ui.painter().rect_filled(rect, 4.0, Color32::from_gray(38));
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "♪",
                        FontId::proportional(20.0),
                        Color32::from_gray(90),
                    );
                }
            }
            ui.vertical(|ui| {
                ui.add_space(5.0);
                ui.add(
                    egui::Label::new(RichText::new(title).strong().size(13.0))
                        .truncate()
                        .sense(Sense::hover()),
                );
                ui.add(egui::Label::new(RichText::new(artist).weak().size(11.0)).truncate());
            });
        });

        // Seek bar
        ui.horizontal(|ui| {
            ui.label(RichText::new(fmt_time(self.pos)).size(11.0).weak());
            let width = (ui.available_width() - 44.0).max(60.0);
            let mut value = self.pos as f32;
            let resp = ui.add_sized(
                [width, 16.0],
                egui::Slider::new(&mut value, 0.0..=dur.max(1.0) as f32)
                    .show_value(false)
                    .trailing_fill(true),
            );
            if resp.drag_started() {
                self.seeking = true;
            }
            if resp.dragged() {
                self.pos = value as f64;
            }
            if resp.drag_stopped() {
                self.seeking = false;
                self.seek(value as f64);
            } else if resp.clicked() {
                self.seek(value as f64);
            }
            ui.label(RichText::new(fmt_time(dur)).size(11.0).weak());
        });

        // Controls
        ui.horizontal(|ui| {
            let ctx = ui.ctx().clone();
            if ui.button("Prev").clicked() {
                self.prev_track(&ctx);
            }
            let label = if self.playing { "Pause" } else { "Play" };
            if ui.button(label).clicked() {
                self.toggle_play(&ctx);
            }
            if ui.button("Next").clicked() {
                self.next_track(true, &ctx);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().slider_width = 80.0;
                let resp = ui.add(
                    egui::Slider::new(&mut self.volume, 0.0..=1.0)
                        .show_value(false)
                        .trailing_fill(true),
                );
                if resp.changed()
                    && let Some(a) = &self.audio
                {
                    a.set_volume(self.volume);
                }
                ui.label("Vol");
            });
        });
    }

    fn queue_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_queue;
        egui::Window::new("Queue")
            .open(&mut open)
            .default_width(240.0)
            .resizable(false)
            .show(ctx, |ui| {
                if self.queue.is_empty() {
                    ui.label(RichText::new("Empty").weak());
                }
                let mut remove = None;
                for (slot, &idx) in self.queue.iter().enumerate() {
                    let name = self
                        .tracks
                        .get(idx)
                        .map(|t| t.title.clone())
                        .unwrap_or_default();
                    ui.horizontal(|ui| {
                        if ui.small_button("x").clicked() {
                            remove = Some(slot);
                        }
                        ui.add(egui::Label::new(RichText::new(name).size(12.0)).truncate());
                    });
                }
                if let Some(slot) = remove {
                    self.queue.remove(slot);
                }
                if !self.queue.is_empty() && ui.button("Clear").clicked() {
                    self.queue.clear();
                }
            });
        self.show_queue = open;
    }
}

fn track_row(ui: &mut egui::Ui, track: &Track, is_current: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    let painter = ui.painter().clone();
    let visuals = ui.visuals().clone();

    if is_current {
        painter.rect_filled(rect, 3.0, visuals.selection.bg_fill);
    } else if resp.hovered() {
        painter.rect_filled(rect, 3.0, visuals.widgets.hovered.bg_fill);
    }

    let text_color = if is_current {
        visuals.strong_text_color()
    } else {
        visuals.text_color()
    };
    let mut left = rect.left() + 6.0;
    if is_current {
        painter.text(
            pos2(left, rect.center().y),
            Align2::LEFT_CENTER,
            "▶",
            FontId::proportional(9.0),
            ACCENT,
        );
        left += 12.0;
    }

    let artist_w = (rect.width() * 0.38).min(130.0);
    let title_clip = Rect::from_min_max(
        pos2(left, rect.top()),
        pos2(rect.right() - artist_w, rect.bottom()),
    );
    painter
        .with_clip_rect(title_clip.intersect(painter.clip_rect()))
        .text(
            pos2(left, rect.center().y),
            Align2::LEFT_CENTER,
            &track.title,
            FontId::proportional(13.0),
            text_color,
        );

    if !track.artist.is_empty() {
        let clip = Rect::from_min_max(
            pos2(rect.right() - artist_w, rect.top()),
            pos2(rect.right() - 4.0, rect.bottom()),
        );
        painter
            .with_clip_rect(clip.intersect(painter.clip_rect()))
            .text(
                pos2(rect.right() - 6.0, rect.center().y),
                Align2::RIGHT_CENTER,
                &track.artist,
                FontId::proportional(11.0),
                visuals.weak_text_color(),
            );
    }

    resp
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e3779b97f4a7c15)
        | 1
}

// ---------------------------------------------------------------- icons

fn app_icon_data() -> egui::IconData {
    let (rgba, w, h) = icon_pixels();
    egui::IconData {
        rgba,
        width: w,
        height: h,
    }
}

/// 32x32 rounded dark tile with a white play triangle.
fn icon_pixels() -> (Vec<u8>, u32, u32) {
    const S: u32 = 32;
    let mut rgba = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let r = 8.0;
            let in_x = px.clamp(r, S as f32 - r);
            let in_y = py.clamp(r, S as f32 - r);
            let d = ((px - in_x).powi(2) + (py - in_y).powi(2)).sqrt();
            if d > r {
                continue; // transparent corner
            }
            let tri = x as f32 >= 11.0
                && x as f32 <= 23.0
                && (y as f32 - 16.0).abs() <= 7.0 - (x as f32 - 11.0) * 0.29;
            let (r_, g_, b_) = if tri { (245, 245, 245) } else { (28, 30, 36) };
            let i = ((y * S + x) * 4) as usize;
            rgba[i] = r_;
            rgba[i + 1] = g_;
            rgba[i + 2] = b_;
            rgba[i + 3] = 255;
        }
    }
    (rgba, S, S)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_wraps_only_when_asked() {
        assert_eq!(sequential_next(0, None, true), None);
        assert_eq!(sequential_next(3, None, false), Some(0));
        assert_eq!(sequential_next(3, Some(1), false), Some(2));
        assert_eq!(sequential_next(3, Some(2), false), None);
        assert_eq!(sequential_next(3, Some(2), true), Some(0));
    }

    #[test]
    fn shuffle_never_picks_the_current_track() {
        for len in 2..12 {
            for cur in 0..len {
                for rand in [0u64, 7, u64::MAX, 12_345] {
                    let n = shuffled_next(len, cur, rand);
                    assert!(n < len);
                    assert_ne!(n, cur);
                }
            }
        }
        assert_eq!(shuffled_next(1, 0, 99), 0);
    }

    #[test]
    fn time_formatting() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(65.9), "1:05");
        assert_eq!(fmt_time(-3.0), "0:00");
    }
}
