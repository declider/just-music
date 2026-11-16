#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]
#![allow(clippy::field_reassign_with_default)]

//! Just Music — a small native Win32 music player.

mod library;
mod player;

use std::{
    collections::VecDeque,
    ffi::c_void,
    path::PathBuf,
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    time::Duration,
};

use library::{Settings, Track};
use player::Audio;

use windows::{
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, CLEARTYPE_QUALITY,
            CLIP_DEFAULT_PRECIS, CreateBitmap, CreateCompatibleBitmap, CreateCompatibleDC,
            CreateDIBSection, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
            DIB_RGB_COLORS, DRAW_TEXT_FORMAT, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT, DT_NOPREFIX,
            DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, Ellipse,
            EndPaint, FillRect, GetStockObject, GetTextExtentPoint32W, HALFTONE, HBRUSH, HBITMAP,
            HDC, HFONT, HGDIOBJ, IntersectClipRect, InvalidateRect, NULL_PEN, OUT_TT_PRECIS,
            PAINTSTRUCT, RestoreDC, RoundRect, SRCCOPY, SaveDC, ScreenToClient, SelectObject,
            SetBkColor, SetBkMode, SetBrushOrgEx, SetStretchBltMode, SetTextColor, StretchBlt,
            TRANSPARENT, UpdateWindow,
        },
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{GetCurrentProcess, SetProcessWorkingSetSize},
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, GetSystemMetricsForDpi,
                SetProcessDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{
                GetFocus, ReleaseCapture, SetCapture, SetFocus, VK_DOWN, VK_LEFT, VK_RIGHT, VK_SPACE,
                VK_UP,
            },
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
                NOTIFYICONDATAW, NOTIFYICON_VERSION_4, Shell_NotifyIconW,
            },
            WindowsAndMessaging::*,
        },
    },
    core::PCWSTR,
};

// ---------------------------------------------------------------- drawing bits

const BG: COLORREF = COLORREF(0x0018_1414); // #141418
const PANEL: COLORREF = COLORREF(0x0021_1c1c); // #1c1c21
const TEXT: COLORREF = COLORREF(0x00ec_ece8); // #e8ecec
const DIM: COLORREF = COLORREF(0x00a0_9a96); // #969aa0
const ACCENT: COLORREF = COLORREF(0x00ff_a05a); // #5aa0ff  (BGR)
const HOVER: COLORREF = COLORREF(0x0030_2a26); // #262a30
const SEL: COLORREF = COLORREF(0x004a_3226); // #26324a
const TRACK_BG: COLORREF = COLORREF(0x0044_3a3a); // #3a3a44

const ERROR_RED: COLORREF = COLORREF(0x0068_7cff);
const WM_TRAY: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;

const CMD_PLAY: usize = 100;
const CMD_PREV: usize = 101;
const CMD_NEXT: usize = 102;
const CMD_SHOW: usize = 103;
const CMD_HIDE: usize = 104;
const CMD_EXIT: usize = 105;

const CMD_QUEUE_NEXT: usize = 200;
const CMD_QUEUE_ADD: usize = 201;
const CMD_QUEUE_CLEAR: usize = 202;

const EM_SETCUEBANNER: u32 = 0x1501;
const EM_SETLIMITTEXT: u32 = 0x00C5;
const EM_SETRECT: u32 = 0x00B3;
const EN_CHANGE_CODE: u32 = 0x0300;

struct Fonts {
    ui: HFONT,
    bold: HFONT,
    small: HFONT,
    icons: HFONT,
}

impl Fonts {
    unsafe fn new(dpi: i32) -> Self {
        // Pixel heights (not points) so text lands on whole pixels at 100% and scales cleanly.
        let h = |px: i32| -(px * dpi / 96).max(9);
        let make = |px: i32, weight: i32, face: PCWSTR| unsafe {
            CreateFontW(
                h(px),
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_TT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                DEFAULT_PITCH.0 as u32,
                face,
            )
        };
        Self {
            ui: make(14, 400, w_ptr("Segoe UI")),
            bold: make(14, 600, w_ptr("Segoe UI")),
            small: make(11, 400, w_ptr("Segoe UI")),
            icons: make(18, 400, w_ptr("Segoe MDL2 Assets")),
        }
    }
}

fn w_ptr(s: &str) -> PCWSTR {
    // Small static-ish strings; leaked so the PCWSTR stays valid.
    let wide: Vec<u16> = s.encode_utf16().chain(std::iter::once(0)).collect();
    PCWSTR(Box::leak(wide.into_boxed_slice()).as_ptr())
}

unsafe fn fill(hdc: HDC, r: RECT, color: COLORREF) {
    let brush = CreateSolidBrush(color);
    FillRect(hdc, &r, brush);
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

unsafe fn round_fill(hdc: HDC, r: RECT, color: COLORREF, radius: i32) {
    let brush = CreateSolidBrush(color);
    let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
    let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
    let _ = RoundRect(hdc, r.left, r.top, r.right, r.bottom, radius * 2, radius * 2);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

unsafe fn draw_text(hdc: HDC, r: RECT, text: &str, font: HFONT, color: COLORREF, flags: DRAW_TEXT_FORMAT) {
    if text.is_empty() {
        return;
    }
    let mut buf: Vec<u16> = text.encode_utf16().collect();
    let old_font = SelectObject(hdc, HGDIOBJ(font.0));
    SetTextColor(hdc, color);
    SetBkMode(hdc, TRANSPARENT);
    let mut rc = r;
    DrawTextW(hdc, &mut buf, &mut rc, flags | DT_NOPREFIX);
    SelectObject(hdc, old_font);
}

unsafe fn draw_glyph(hdc: HDC, r: RECT, glyph: &str, font: HFONT, color: COLORREF) {
    draw_text(hdc, r, glyph, font, color, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
}

/// Folder silhouette for the "open folder" button (no MDL2 glyph needed).
unsafe fn draw_folder(hdc: HDC, r: RECT, hot: bool, s: f32) {
    let color = if hot { TEXT } else { DIM };
    let w = (16.0 * s) as i32;
    let h = (13.0 * s) as i32;
    let left = (r.left + r.right - w) / 2;
    let top = (r.top + r.bottom - h) / 2;
    fill(hdc, rect(left, top, left + w / 2, top + (4.0 * s) as i32), color);
    round_fill(hdc, rect(left, top + (3.0 * s) as i32, left + w, top + h), color, (2.0 * s) as i32);
}

unsafe fn draw_bar(hdc: HDC, r: RECT, frac: f32, hot: bool, scale: f32) {
    let center = (r.top + r.bottom) / 2;
    let h = ((if hot { 5.0 } else { 3.0 }) * scale).round() as i32;
    let track = RECT {
        left: r.left,
        top: center - h / 2,
        right: r.right,
        bottom: center - h / 2 + h,
    };
    round_fill(hdc, track, TRACK_BG, (h / 2).max(1));

    let frac = frac.clamp(0.0, 1.0);
    let knob = ((if hot { 5.0 } else { 4.0 }) * scale).round() as i32;
    let cx = (r.left + ((r.right - r.left) as f32 * frac) as i32)
        .clamp(r.left + knob, (r.right - knob).max(r.left + knob));
    if cx > r.left {
        let done = RECT {
            right: cx,
            ..track
        };
        round_fill(hdc, done, ACCENT, (h / 2).max(1));
    }

    let brush = CreateSolidBrush(TEXT);
    let old_brush = SelectObject(hdc, HGDIOBJ(brush.0));
    let old_pen = SelectObject(hdc, GetStockObject(NULL_PEN));
    let _ = Ellipse(hdc, cx - knob, center - knob, cx + knob, center + knob);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(HGDIOBJ(brush.0));
}

/// Draws text clipped to `r`; long strings scroll with a pause at both ends.
unsafe fn draw_marquee(hdc: HDC, r: RECT, text: &str, font: HFONT, color: COLORREF, scale: f32, tick: u32) {
    if text.is_empty() {
        return;
    }
    let buf: Vec<u16> = text.encode_utf16().collect();
    let mut size = SIZE::default();
    let old = SelectObject(hdc, HGDIOBJ(font.0));
    let _ = GetTextExtentPoint32W(hdc, &buf, &mut size);
    SelectObject(hdc, old);
    let avail = (r.right - r.left).max(1);
    if size.cx <= avail {
        draw_text(
            hdc,
            r,
            text,
            font,
            color,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        return;
    }

    // 6px per 200ms tick, ~1.2s pause before and after.
    let gap = (24.0 * scale) as i32;
    let travel = size.cx - avail + gap;
    let scroll_ticks = (travel as f32 / (6.0 * scale)).ceil() as u32;
    let pause = 6;
    let t = tick % (pause + scroll_ticks + pause);
    let offset = if t < pause {
        0
    } else if t < pause + scroll_ticks {
        ((t - pause) as f32 * 6.0 * scale) as i32
    } else {
        travel
    };

    let saved = SaveDC(hdc);
    IntersectClipRect(hdc, r.left, r.top, r.right, r.bottom);
    let shifted = rect(r.left - offset, r.top, r.left - offset + size.cx + gap, r.bottom);
    draw_text(
        hdc,
        shifted,
        text,
        font,
        color,
        DT_LEFT | DT_SINGLELINE | DT_VCENTER,
    );
    let _ = RestoreDC(hdc, saved);
}

// ---------------------------------------------------------------- layout

#[derive(Clone, Copy)]
struct Layout {
    header: RECT,
    open: RECT,
    search: RECT,
    min_btn: RECT,
    max_btn: RECT,
    close_btn: RECT,
    list: RECT,
    bar: RECT,
    cover: RECT,
    title: RECT,
    artist: RECT,
    shuffle: RECT,
    prev: RECT,
    play: RECT,
    next: RECT,
    cur_time: RECT,
    seek: RECT,
    tot_time: RECT,
    vol_bar: RECT,
    vol_pct: RECT,
    status: RECT,
    row_h: i32,
    scrollbar: i32,
}

fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

impl Layout {
    fn edit_rect(&self, s: f32) -> RECT {
        rect(
            self.search.left + (22.0 * s) as i32,
            self.search.top,
            self.search.right - (6.0 * s) as i32,
            self.search.bottom,
        )
    }

    fn compute(w: i32, h: i32, scale: f32) -> Self {
        let s = |v: f32| (v * scale).round() as i32;
        let pad = s(6.0);
        let header_h = s(36.0);
        let open = s(24.0);
        let cap_w = s(34.0);
        let search_h = s(24.0);
        let status_h = s(15.0);
        let bar_h = s(80.0);
        let cover_sz = s(68.0);
        let ctrl = s(28.0);
        let seek_h = s(18.0);
        let row_h = s(26.0);

        let header = rect(0, 0, w, header_h);
        let open_rc = rect(pad, (header_h - open) / 2, pad + open, (header_h + open) / 2);
        let close_btn = rect(w - cap_w, 0, w, header_h);
        let max_btn = rect(w - cap_w * 2, 0, w - cap_w, header_h);
        let min_btn = rect(w - cap_w * 3, 0, w - cap_w * 2, header_h);
        let search = rect(
            open_rc.right + s(6.0),
            (header_h - search_h) / 2,
            min_btn.left - s(6.0),
            (header_h + search_h) / 2,
        );

        let bar_top = (h - status_h - bar_h).max(header_h + row_h);
        let bar = rect(0, bar_top, w, bar_top + bar_h);
        let list = rect(pad, header_h + s(4.0), w - pad, bar_top);

        let cover = rect(
            pad,
            bar_top + s(6.0),
            pad + cover_sz,
            bar_top + s(6.0) + cover_sz,
        );
        let content_left = cover.right + s(10.0);
        let content_right = w - pad;

        // Track / transport row.
        let next_rc = rect(content_right - ctrl, bar_top + s(14.0), content_right, bar_top + s(14.0) + ctrl);
        let play = rect(next_rc.left - s(6.0) - ctrl, next_rc.top, next_rc.left - s(6.0), next_rc.bottom);
        let prev = rect(play.left - s(6.0) - ctrl, next_rc.top, play.left - s(6.0), next_rc.bottom);
        let shuffle = rect(prev.left - s(10.0) - ctrl, next_rc.top, prev.left - s(10.0), next_rc.bottom);

        let title = rect(
            content_left,
            bar_top + s(10.0),
            (shuffle.left - s(10.0)).max(content_left + s(20.0)),
            bar_top + s(29.0),
        );
        let artist = rect(
            content_left,
            bar_top + s(30.0),
            title.right,
            bar_top + s(47.0),
        );

        // Seek / volume row, aligned with the bottom of the cover.
        let seek_y = bar_top + s(56.0);
        let vol_pct = rect(content_right - s(34.0), seek_y, content_right, seek_y + seek_h);
        let vol_bar = rect(vol_pct.left - s(6.0) - s(64.0), seek_y, vol_pct.left - s(6.0), seek_y + seek_h);
        let cur_time = rect(content_left, seek_y, content_left + s(34.0), seek_y + seek_h);
        let tot_time = rect(vol_bar.left - s(6.0) - s(34.0), seek_y, vol_bar.left - s(6.0), seek_y + seek_h);
        let seek = rect(
            cur_time.right + s(6.0),
            seek_y,
            (tot_time.left - s(6.0)).max(cur_time.right + s(20.0)),
            seek_y + seek_h,
        );

        Self {
            header,
            open: open_rc,
            search,
            min_btn,
            max_btn,
            close_btn,
            list,
            bar,
            cover,
            title,
            artist,
            shuffle,
            prev,
            play,
            next: next_rc,
            cur_time,
            seek,
            tot_time,
            vol_bar,
            vol_pct,
            status: rect(pad, h - status_h, w - pad, h - s(2.0)),
            row_h,
            scrollbar: s(8.0),
        }
    }
}

fn inside(r: RECT, x: i32, y: i32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn expand(r: RECT, d: i32) -> RECT {
    rect(r.left - d, r.top - d, r.right + d, r.bottom + d)
}

/// Bars are only grabbable near their visible 4-6px line, not the whole row.
fn bar_hit(r: RECT, x: i32, y: i32, s: f32) -> bool {
    x >= r.left - (3.0 * s) as i32
        && x <= r.right + (3.0 * s) as i32
        && (y - (r.top + r.bottom) / 2).abs() <= (4.0 * s) as i32 + 1
}

// ---------------------------------------------------------------- app state

#[derive(Clone, Copy, PartialEq)]
enum Hit {
    Open,
    Shuffle,
    Prev,
    Play,
    Next,
    Min,
    Max,
    Close,
    Seek,
    Volume,
    Scroll,
    Row(usize),
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    Seek,
    Volume,
    Scroll,
}

struct Scan {
    rx: Receiver<Vec<Track>>,
    count: Arc<AtomicUsize>,
}

struct Cover {
    bmp: windows::Win32::Graphics::Gdi::HBITMAP,
    w: i32,
    h: i32,
}

struct App {
    hwnd: HWND,
    edit: HWND,
    icon: HICON,
    fonts: Fonts,
    brush_input: HBRUSH,

    audio: Option<Audio>,
    error: Option<String>,

    tracks: Vec<Track>,
    visible: Vec<usize>,
    search: String,
    folder: Option<PathBuf>,

    current: Option<usize>,
    playing: bool,
    pos: f64,
    volume: f32,
    shuffle: bool,
    queue: VecDeque<usize>,
    scroll: usize,
    drag: Drag,

    hover: Option<Hit>,
    scan: Option<Scan>,
    scan_count: usize,
    anim: u32,
    marquee: u32,

    cover: Option<Cover>,
    rng: u64,
    dpi_scale: f32,

    buf_dc: HDC,
    buf_bmp: HBITMAP,
    buf_old: HGDIOBJ,
    buf_w: i32,
    buf_h: i32,

    tray: bool,
    tray_tip: String,
}

impl App {
    unsafe fn new(hwnd: HWND) -> Self {
        let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0;
        let settings = Settings::load();

        let mut app = Self {
            hwnd,
            edit: HWND::default(),
            icon: HICON::default(),
            fonts: unsafe { Fonts::new((96.0 * dpi) as i32) },
            brush_input: unsafe { CreateSolidBrush(BG) },
            audio: Audio::open().ok(),
            error: None,
            tracks: Vec::new(),
            visible: Vec::new(),
            search: String::new(),
            folder: None,
            current: None,
            playing: false,
            pos: 0.0,
            volume: settings.volume,
            shuffle: settings.shuffle,
            queue: VecDeque::new(),
            scroll: 0,
            drag: Drag::None,
            hover: None,
            scan: None,
            scan_count: 0,
            anim: 0,
            marquee: 0,
            cover: None,
            rng: seed(),
            dpi_scale: dpi,
            buf_dc: HDC::default(),
            buf_bmp: HBITMAP::default(),
            buf_old: HGDIOBJ::default(),
            buf_w: 0,
            buf_h: 0,
            tray: false,
            tray_tip: String::new(),
        };

        if let Some(audio) = &app.audio {
            audio.set_volume(app.volume);
        }

        unsafe {
            app.icon = make_icon();
            if !app.icon.is_invalid() {
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_SMALL as usize)),
                    Some(LPARAM(app.icon.0 as isize)),
                );
                SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(ICON_BIG as usize)),
                    Some(LPARAM(app.icon.0 as isize)),
                );
            }
            app.create_edit(hwnd);
            app.init_tray(hwnd);
        }

        let startup = std::env::args()
            .nth(1)
            .map(PathBuf::from)
            .or(settings.folder);
        if let Some(dir) = startup {
            if dir.is_dir() {
                app.load_folder(dir);
            }
        } else {
            app.save_settings();
        }

        app
    }

    unsafe fn create_edit(&mut self, hwnd: HWND) {
        let style = WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32);
        let Ok(edit) = (unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w_ptr("EDIT"),
                w_ptr(""),
                style,
                0,
                0,
                10,
                10,
                Some(hwnd),
                None,
                None,
                None,
            )
        }) else {
            return;
        };
        self.edit = edit;
        unsafe {
            SendMessageW(edit, WM_SETFONT, Some(WPARAM(self.fonts.ui.0 as isize as usize)), Some(LPARAM(1)));
            SendMessageW(
                edit,
                EM_SETCUEBANNER,
                Some(WPARAM(0)),
                Some(LPARAM(w_ptr("Search title or artist").0 as isize)),
            );
            SendMessageW(edit, EM_SETLIMITTEXT, Some(WPARAM(256)), Some(LPARAM(0)));
        }
    }

    unsafe fn init_tray(&mut self, hwnd: HWND) {
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = self.icon;
        set_tip(&mut nid.szTip, "Just Music");
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                self.tray = true;
                nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
            }
        }
    }

    unsafe fn remove_tray(&mut self) {
        if !self.tray {
            return;
        }
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
        self.tray = false;
    }

    fn scale(&self) -> f32 {
        self.dpi_scale
    }

    fn layout(&self) -> Layout {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        Layout::compute(rc.right, rc.bottom, self.scale())
    }

    fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn save_settings(&self) {
        Settings {
            folder: self.folder.clone(),
            volume: self.volume,
            shuffle: self.shuffle,
        }
        .save();
    }

    // ------------------------------------------------ library / playback

    fn load_folder(&mut self, dir: PathBuf) {
        self.folder = Some(dir.clone());
        self.tracks.clear();
        self.visible.clear();
        self.scroll = 0;
        self.current = None;
        self.playing = false;
        self.pos = 0.0;
        self.queue.clear();
        if let Some(audio) = &self.audio {
            audio.stop();
        }

        let (tx, rx) = mpsc::channel();
        let count = Arc::new(AtomicUsize::new(0));
        let thread_count = count.clone();
        std::thread::spawn(move || {
            let mut tracks = Vec::new();
            library::scan(&dir, &mut tracks, &thread_count);
            tracks.sort_by(|a, b| {
                a.title
                    .to_lowercase()
                    .cmp(&b.title.to_lowercase())
                    .then_with(|| a.artist.to_lowercase().cmp(&b.artist.to_lowercase()))
            });
            let _ = tx.send(tracks);
        });
        self.scan = Some(Scan { rx, count });
        self.scan_count = 0;
        self.save_settings();
        self.invalidate();
    }

    fn poll_scan(&mut self) {
        let Some(scan) = &self.scan else {
            return;
        };
        match scan.rx.try_recv() {
            Ok(tracks) => {
                self.tracks = tracks;
                self.scan = None;
                self.scan_count = self.tracks.len();
                self.refilter();
                self.invalidate();
                trim_working_set();
            }
            Err(TryRecvError::Empty) => {
                self.scan_count = scan.count.load(Ordering::Relaxed);
            }
            Err(TryRecvError::Disconnected) => {
                self.scan = None;
            }
        }
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
        self.scroll = 0;
    }

    fn play_index(&mut self, idx: usize) {
        let Some(track) = self.tracks.get(idx) else {
            return;
        };
        let path = track.path.clone();
        let title = track.title.clone();
        let artist = track.artist.clone();

        let Some(audio) = &self.audio else {
            self.error = Some("No audio output device".into());
            return;
        };
        match audio.play(&path, self.volume) {
            Ok(()) => {
                self.current = Some(idx);
                self.pos = 0.0;
                self.playing = true;
                self.marquee = 0;
                self.error = None;
                unsafe {
                    self.load_cover();
                }
                self.scroll_to_current();
                unsafe {
                    let text = if artist.is_empty() {
                        format!("{title} — Just Music")
                    } else {
                        format!("{title} — {artist} — Just Music")
                    };
                    let _ = SetWindowTextW(self.hwnd, &windows::core::HSTRING::from(text));
                }
            }
            Err(e) => {
                self.current = None;
                self.playing = false;
                self.error = Some(format!("{title}: {e}"));
            }
        }
        self.invalidate();
    }

    fn next_track(&mut self, wrap: bool) {
        let idx = if let Some(i) = self.queue.pop_front() {
            Some(i)
        } else if self.shuffle && self.current.is_some() {
            Some(shuffled_next(
                self.tracks.len(),
                self.current.unwrap_or(0),
                self.rng_next(),
            ))
        } else {
            sequential_next(self.tracks.len(), self.current, wrap)
        };
        match idx {
            Some(i) => self.play_index(i),
            None => {
                if let Some(audio) = &self.audio {
                    audio.stop();
                }
                self.playing = false;
                self.pos = 0.0;
                self.invalidate();
            }
        }
    }

    fn prev_track(&mut self) {
        if self.pos > 3.0 {
            self.seek_to(0.0);
            return;
        }
        let len = self.tracks.len();
        if len == 0 {
            return;
        }
        let idx = match self.current {
            Some(0) | None => len - 1,
            Some(c) => c - 1,
        };
        self.play_index(idx);
    }

    fn toggle_play(&mut self) {
        let Some(audio) = &self.audio else {
            return;
        };
        let finished = audio.finished();
        if self.current.is_none() || finished {
            if !self.tracks.is_empty() {
                self.next_track(true);
            }
            return;
        }
        if audio.is_paused() {
            audio.resume();
        } else {
            audio.pause();
        }
        self.playing = !audio.is_paused();
        self.invalidate();
    }

    fn seek_to(&mut self, secs: f64) {
        self.pos = secs;
        if let Some(audio) = &self.audio {
            audio.seek(Duration::from_secs_f64(secs.max(0.0)));
        }
        self.invalidate();
    }

    fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(audio) = &self.audio {
            audio.set_volume(self.volume);
        }
        self.invalidate();
    }

    fn current_duration(&self) -> f64 {
        self.current
            .and_then(|i| self.tracks.get(i))
            .map(|t| t.duration)
            .unwrap_or(0.0)
    }

    fn scroll_to_current(&mut self) {
        let Some(cur) = self.current else {
            return;
        };
        let Some(pos) = self.visible.iter().position(|&i| i == cur) else {
            return;
        };
        let l = self.layout();
        let rows = ((l.list.bottom - l.list.top) / l.row_h).max(1) as usize;
        if pos < self.scroll {
            self.scroll = pos;
        } else if pos >= self.scroll + rows {
            self.scroll = pos + 1 - rows;
        }
    }

    fn rng_next(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    unsafe fn load_cover(&mut self) {
        if let Some(old) = self.cover.take() {
            unsafe {
                let _ = DeleteObject(HGDIOBJ(old.bmp.0));
            }
        }
        let Some(track) = self.current.and_then(|i| self.tracks.get(i)) else {
            return;
        };
        let Some((rgba, w, h)) = library::cover_rgba(&track.path) else {
            return;
        };

        unsafe {
            let mut bmi = BITMAPINFO::default();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w as i32;
            bmi.bmiHeader.biHeight = -(h as i32);
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB.0;

            let mut bits: *mut c_void = null_mut();
            let Ok(bmp) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
                return;
            };
            std::ptr::copy_nonoverlapping(rgba.as_ptr(), bits as *mut u8, rgba.len());
            self.cover = Some(Cover {
                bmp,
                w: w as i32,
                h: h as i32,
            });
        }
    }

    fn tick(&mut self) {
        self.anim = self.anim.wrapping_add(1);
        self.marquee = self.marquee.wrapping_add(1);
        self.poll_scan();

        let finished = self.current.is_some() && self.audio.as_ref().is_some_and(|a| a.finished());
        if finished {
            self.next_track(false);
            return;
        }
        if let Some(audio) = &self.audio {
            if self.current.is_some() && self.drag != Drag::Seek {
                self.pos = audio.position().as_secs_f64();
            }
            self.playing = self.current.is_some() && !audio.is_paused();
        }

        let tip = match self.current.and_then(|i| self.tracks.get(i)) {
            Some(t) if t.artist.is_empty() => t.title.clone(),
            Some(t) => format!("{} — {}", t.artist, t.title),
            None => String::new(),
        };
        if tip != self.tray_tip {
            self.tray_tip = tip;
            unsafe {
                self.update_tray_tip();
            }
        }

        unsafe {
            if IsWindowVisible(self.hwnd).as_bool() && !IsIconic(self.hwnd).as_bool() {
                let _ = InvalidateRect(Some(self.hwnd), None, false);
            }
        }
    }

    unsafe fn update_tray_tip(&self) {
        if !self.tray {
            return;
        }
        let mut nid = NOTIFYICONDATAW::default();
        nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = 1;
        nid.uFlags = NIF_TIP;
        let tip = if self.tray_tip.is_empty() {
            "Just Music".to_string()
        } else {
            self.tray_tip.clone()
        };
        set_tip(&mut nid.szTip, &tip);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    // ------------------------------------------------ input

    /// Non-client hit test for the borderless window: resize edges + draggable header.
    fn nchit(&self, x: i32, y: i32) -> u32 {
        let l = self.layout();
        let b = (5.0 * self.scale()) as i32;
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut rc);
        }
        if !unsafe { IsZoomed(self.hwnd) }.as_bool() {
            let left = x < b;
            let right = x >= rc.right - b;
            let top = y < b;
            let bottom = y >= rc.bottom - b;
            match (top, bottom, left, right) {
                (true, _, true, _) => return HTTOPLEFT,
                (true, _, _, true) => return HTTOPRIGHT,
                (_, true, true, _) => return HTBOTTOMLEFT,
                (_, true, _, true) => return HTBOTTOMRIGHT,
                (true, _, _, _) => return HTTOP,
                (_, true, _, _) => return HTBOTTOM,
                (_, _, true, _) => return HTLEFT,
                (_, _, _, true) => return HTRIGHT,
                _ => {}
            }
        }
        if y < l.header.bottom {
            if !inside(l.search, x, y) && self.hit_test(x, y).is_none() {
                return HTCAPTION;
            }
        } else if y >= l.bar.top && self.hit_test(x, y).is_none() {
            // The compact header is hard to grab, so the player bar drags the window too.
            return HTCAPTION;
        }
        HTCLIENT
    }

    fn hit_test(&self, x: i32, y: i32) -> Option<Hit> {
        let l = self.layout();
        if inside(l.close_btn, x, y) {
            return Some(Hit::Close);
        }
        if inside(l.max_btn, x, y) {
            return Some(Hit::Max);
        }
        if inside(l.min_btn, x, y) {
            return Some(Hit::Min);
        }
        if inside(l.open, x, y) {
            return Some(Hit::Open);
        }
        if inside(l.shuffle, x, y) {
            return Some(Hit::Shuffle);
        }
        if inside(l.prev, x, y) {
            return Some(Hit::Prev);
        }
        if inside(l.play, x, y) {
            return Some(Hit::Play);
        }
        if inside(l.next, x, y) {
            return Some(Hit::Next);
        }
        if bar_hit(l.seek, x, y, self.scale()) {
            return Some(Hit::Seek);
        }
        if bar_hit(l.vol_bar, x, y, self.scale()) {
            return Some(Hit::Volume);
        }
        if inside(l.list, x, y) {
            if x >= l.list.right - l.scrollbar {
                let rows = ((l.list.bottom - l.list.top) / l.row_h).max(1) as usize;
                return (self.visible.len() > rows).then_some(Hit::Scroll);
            }
            let row = self.scroll + ((y - l.list.top) / l.row_h) as usize;
            if row < self.visible.len() {
                return Some(Hit::Row(row));
            }
        }
        None
    }

    fn seek_from_x(&mut self, x: i32) {
        let l = self.layout();
        let width = (l.seek.right - l.seek.left).max(1);
        let frac = ((x - l.seek.left) as f32 / width as f32).clamp(0.0, 1.0);
        let dur = self.current_duration();
        self.pos = (dur * frac as f64).clamp(0.0, dur);
        self.invalidate();
    }

    fn volume_from_x(&mut self, x: i32) {
        let l = self.layout();
        let width = (l.vol_bar.right - l.vol_bar.left).max(1);
        let frac = ((x - l.vol_bar.left) as f32 / width as f32).clamp(0.0, 1.0);
        self.set_volume(frac);
    }

    fn scroll_from_y(&mut self, y: i32) {
        let l = self.layout();
        let rows = ((l.list.bottom - l.list.top) / l.row_h).max(1) as usize;
        let total = self.visible.len();
        let max_scroll = total.saturating_sub(rows);
        if max_scroll == 0 {
            return;
        }
        let height = (l.list.bottom - l.list.top).max(1);
        let thumb_h = (((rows as f32 / total as f32) * height as f32).max(20.0) as i32).min(height);
        let span = (height - thumb_h).max(1);
        // Centre the thumb under the cursor so grabbing it doesn't jump.
        let frac = ((y - l.list.top - thumb_h / 2) as f32 / span as f32).clamp(0.0, 1.0);
        self.scroll = (frac * max_scroll as f32).round() as usize;
        self.invalidate();
    }

    fn on_mouse_move(&mut self, x: i32, y: i32) {
        match self.drag {
            Drag::Seek => {
                self.seek_from_x(x);
                return;
            }
            Drag::Volume => {
                self.volume_from_x(x);
                return;
            }
            Drag::Scroll => {
                self.scroll_from_y(y);
                return;
            }
            Drag::None => {}
        }
        let hit = self.hit_test(x, y);
        if hit != self.hover {
            self.hover = hit;
            self.invalidate();
        }
    }

    fn on_mouse_down(&mut self, x: i32, y: i32) {
        match self.hit_test(x, y) {
            Some(Hit::Open) => {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.load_folder(dir);
                }
            }
            Some(Hit::Shuffle) => {
                self.shuffle = !self.shuffle;
                self.save_settings();
                self.invalidate();
            }
            Some(Hit::Prev) => self.prev_track(),
            Some(Hit::Play) => self.toggle_play(),
            Some(Hit::Next) => self.next_track(true),
            Some(Hit::Min) => unsafe {
                let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
            },
            Some(Hit::Max) => unsafe {
                if IsZoomed(self.hwnd).as_bool() {
                    let _ = ShowWindow(self.hwnd, SW_RESTORE);
                } else {
                    let _ = ShowWindow(self.hwnd, SW_MAXIMIZE);
                }
            },
            Some(Hit::Close) => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Some(Hit::Seek) => {
                self.drag = Drag::Seek;
                unsafe {
                    SetCapture(self.hwnd);
                }
                self.seek_from_x(x);
            }
            Some(Hit::Volume) => {
                self.drag = Drag::Volume;
                unsafe {
                    SetCapture(self.hwnd);
                }
                self.volume_from_x(x);
            }
            Some(Hit::Scroll) => {
                self.drag = Drag::Scroll;
                unsafe {
                    SetCapture(self.hwnd);
                }
                self.scroll_from_y(y);
            }
            // Single click only hovers; playing on double click avoids accidental track changes.
            Some(Hit::Row(_)) => {}
            None => {}
        }
    }

    fn on_double_click(&mut self, x: i32, y: i32) {
        if let Some(Hit::Row(row)) = self.hit_test(x, y)
            && let Some(&idx) = self.visible.get(row)
        {
            self.play_index(idx);
        }
    }

    fn on_mouse_up(&mut self) {
        match self.drag {
            Drag::Seek => self.seek_to(self.pos),
            Drag::Volume => self.save_settings(),
            Drag::None | Drag::Scroll => {}
        }
        if self.drag != Drag::None {
            unsafe {
                let _ = ReleaseCapture();
            }
        }
        self.drag = Drag::None;
    }

    fn on_wheel(&mut self, delta: i32) {
        let rows = ((self.layout().list.bottom - self.layout().list.top) / self.layout().row_h).max(1);
        let step = if delta > 0 { -3 } else { 3 };
        let max_scroll = self
            .visible
            .len()
            .saturating_sub(rows as usize);
        let next = self.scroll as i32 + step;
        self.scroll = next.clamp(0, max_scroll as i32) as usize;
        self.invalidate();
    }

    fn on_key(&mut self, key: u32) {
        match key {
            k if k == VK_SPACE.0 as u32 => self.toggle_play(),
            k if k == VK_LEFT.0 as u32 => self.seek_to((self.pos - 5.0).max(0.0)),
            k if k == VK_RIGHT.0 as u32 => {
                let dur = self.current_duration();
                if dur > 0.0 {
                    self.seek_to((self.pos + 5.0).min(dur));
                }
            }
            k if k == VK_UP.0 as u32 => self.set_volume(self.volume + 0.05),
            k if k == VK_DOWN.0 as u32 => self.set_volume(self.volume - 0.05),
            _ => {}
        }
    }

    fn on_search_changed(&mut self) {
        let mut buf = [0u16; 256];
        let len = unsafe { GetWindowTextW(self.edit, &mut buf) } as usize;
        self.search = String::from_utf16_lossy(&buf[..len]);
        self.refilter();
        self.invalidate();
    }

    fn context_menu_row(&mut self, row: usize) {
        let Some(&idx) = self.visible.get(row) else {
            return;
        };
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUEUE_NEXT, w_ptr("Play next"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUEUE_ADD, w_ptr("Add to queue"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_QUEUE_CLEAR, w_ptr("Clear queue"));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(self.hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                self.hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));

            match cmd.0 as usize {
                CMD_QUEUE_NEXT => self.queue.push_front(idx),
                CMD_QUEUE_ADD => self.queue.push_back(idx),
                CMD_QUEUE_CLEAR => self.queue.clear(),
                _ => {}
            }
            self.invalidate();
        }
    }

    unsafe fn tray_menu(&mut self, hwnd: HWND) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let play = if self.playing { w_ptr("Pause") } else { w_ptr("Play") };
            let _ = AppendMenuW(menu, MF_STRING, CMD_PLAY, play);
            let _ = AppendMenuW(menu, MF_STRING, CMD_PREV, w_ptr("Previous"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_NEXT, w_ptr("Next"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_SHOW, w_ptr("Show"));
            let _ = AppendMenuW(menu, MF_STRING, CMD_HIDE, w_ptr("Hide"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            let _ = AppendMenuW(menu, MF_STRING, CMD_EXIT, w_ptr("Exit"));

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let _ = SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenu(
                menu,
                TPM_RETURNCMD | TPM_RIGHTBUTTON,
                pt.x,
                pt.y,
                None,
                hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));

            match cmd.0 as usize {
                CMD_PLAY => self.toggle_play(),
                CMD_PREV => self.prev_track(),
                CMD_NEXT => self.next_track(true),
                CMD_SHOW => {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = ShowWindow(hwnd, SW_RESTORE);
                    let _ = SetForegroundWindow(hwnd);
                }
                CMD_HIDE => {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                CMD_EXIT => {
                    let _ = DestroyWindow(hwnd);
                }
                _ => {}
            }
        }
    }

    // ------------------------------------------------ painting

    unsafe fn paint(&mut self, hwnd: HWND) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        if rc.right <= 0 || rc.bottom <= 0 {
            unsafe {
                let _ = EndPaint(hwnd, &ps);
            }
            return;
        }

        self.ensure_buffer(hdc, rc.right, rc.bottom);

        unsafe {
            self.draw(self.buf_dc, rc);
            let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, Some(self.buf_dc), 0, 0, SRCCOPY);
            let _ = EndPaint(hwnd, &ps);
        }
    }

    /// Back buffer survives repaints; only a resize recreates it.
    fn ensure_buffer(&mut self, hdc: HDC, w: i32, h: i32) {
        if self.buf_w == w && self.buf_h == h && !self.buf_dc.0.is_null() {
            return;
        }
        unsafe {
            if !self.buf_dc.0.is_null() {
                if !self.buf_old.0.is_null() {
                    SelectObject(self.buf_dc, self.buf_old);
                }
                let _ = DeleteObject(HGDIOBJ(self.buf_bmp.0));
                let _ = DeleteDC(self.buf_dc);
            }
            let dc = CreateCompatibleDC(Some(hdc));
            let bmp = CreateCompatibleBitmap(hdc, w, h);
            let old = SelectObject(dc, HGDIOBJ(bmp.0));
            self.buf_dc = dc;
            self.buf_bmp = bmp;
            self.buf_old = old;
            self.buf_w = w;
            self.buf_h = h;
        }
    }

    unsafe fn draw(&mut self, hdc: HDC, rc: RECT) {
        let l = self.layout();
        let s = self.scale();
        let hot = |h: Option<Hit>, want: Hit| h == Some(want);

        unsafe {
            fill(hdc, rc, BG);

            // --- title bar: open + search + window buttons
            fill(hdc, l.header, PANEL);

            round_fill(
                hdc,
                l.open,
                if hot(self.hover, Hit::Open) { HOVER } else { PANEL },
                (4.0 * s) as i32,
            );
            draw_folder(hdc, l.open, hot(self.hover, Hit::Open), s);

            let focused = GetFocus() == self.edit;
            fill(hdc, expand(l.search, (1.0 * s) as i32), if focused { ACCENT } else { TRACK_BG });
            fill(hdc, l.search, BG);
            draw_glyph(
                hdc,
                rect(l.search.left, l.search.top, l.search.left + (22.0 * s) as i32, l.search.bottom),
                "\u{E721}",
                self.fonts.icons,
                DIM,
            );

            // Window buttons.
            for (r, hovered) in [
                (l.min_btn, hot(self.hover, Hit::Min)),
                (l.max_btn, hot(self.hover, Hit::Max)),
                (l.close_btn, hot(self.hover, Hit::Close)),
            ] {
                if hovered {
                    fill(hdc, r, if r == l.close_btn { COLORREF(0x001c_2bc4) } else { HOVER });
                }
            }
            draw_glyph(hdc, l.min_btn, "\u{E921}", self.fonts.icons, TEXT);
            draw_glyph(
                hdc,
                l.max_btn,
                if IsZoomed(self.hwnd).as_bool() { "\u{E923}" } else { "\u{E922}" },
                self.fonts.icons,
                TEXT,
            );
            draw_glyph(hdc, l.close_btn, "\u{E8BB}", self.fonts.icons, TEXT);

            fill(
                hdc,
                rect(0, l.header.bottom - 1, rc.right, l.header.bottom),
                COLORREF(0x0030_2a26),
            );

            // --- playlist
            self.draw_list(hdc, &l, s);

            // --- player bar
            fill(hdc, l.bar, PANEL);
            fill(hdc, rect(0, l.bar.top, rc.right, l.bar.top + 1), COLORREF(0x0030_2a26));

            if let Some(cover) = &self.cover {
                let mem = CreateCompatibleDC(Some(hdc));
                let old = SelectObject(mem, HGDIOBJ(cover.bmp.0));
                let _ = SetStretchBltMode(hdc, HALFTONE);
                let _ = SetBrushOrgEx(hdc, 0, 0, None);
                let _ = StretchBlt(
                    hdc,
                    l.cover.left,
                    l.cover.top,
                    l.cover.right - l.cover.left,
                    l.cover.bottom - l.cover.top,
                    Some(mem),
                    0,
                    0,
                    cover.w,
                    cover.h,
                    SRCCOPY,
                );
                SelectObject(mem, old);
                let _ = DeleteDC(mem);
            } else {
                fill(hdc, l.cover, BG);
                draw_glyph(hdc, l.cover, "\u{E8D6}", self.fonts.icons, DIM);
            }

            let (title, artist) = match self.current.and_then(|i| self.tracks.get(i)) {
                Some(t) => (t.title.clone(), t.artist.clone()),
                None => ("Nothing playing".to_string(), String::new()),
            };
            draw_marquee(
                hdc,
                l.title,
                &title,
                self.fonts.bold,
                if self.current.is_some() { TEXT } else { DIM },
                s,
                self.marquee,
            );
            draw_text(
                hdc,
                l.artist,
                &artist,
                self.fonts.small,
                DIM,
                DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS | DT_VCENTER,
            );

            // Transport.
            round_fill(
                hdc,
                l.shuffle,
                if self.shuffle {
                    SEL
                } else if hot(self.hover, Hit::Shuffle) {
                    HOVER
                } else {
                    PANEL
                },
                (4.0 * s) as i32,
            );
            draw_glyph(
                hdc,
                l.shuffle,
                "\u{E8B1}",
                self.fonts.icons,
                if self.shuffle { ACCENT } else { DIM },
            );

            round_fill(
                hdc,
                l.prev,
                if hot(self.hover, Hit::Prev) { HOVER } else { PANEL },
                (4.0 * s) as i32,
            );
            draw_glyph(hdc, l.prev, "\u{E892}", self.fonts.icons, TEXT);

            round_fill(hdc, l.play, COLORREF(0x0032_2418), (4.0 * s) as i32);
            draw_glyph(
                hdc,
                l.play,
                if self.playing { "\u{E769}" } else { "\u{E768}" },
                self.fonts.icons,
                ACCENT,
            );

            round_fill(
                hdc,
                l.next,
                if hot(self.hover, Hit::Next) { HOVER } else { PANEL },
                (4.0 * s) as i32,
            );
            draw_glyph(hdc, l.next, "\u{E893}", self.fonts.icons, TEXT);

            // Seek + volume.
            let dur = self.current_duration();
            draw_text(
                hdc,
                l.cur_time,
                &fmt_time(self.pos),
                self.fonts.small,
                DIM,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );
            draw_text(
                hdc,
                l.tot_time,
                &fmt_time(dur),
                self.fonts.small,
                DIM,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
            );
            let frac = if dur > 0.0 { (self.pos / dur) as f32 } else { 0.0 };
            draw_bar(
                hdc,
                l.seek,
                frac,
                hot(self.hover, Hit::Seek) || self.drag == Drag::Seek,
                s,
            );

            draw_bar(
                hdc,
                l.vol_bar,
                self.volume,
                hot(self.hover, Hit::Volume) || self.drag == Drag::Volume,
                s,
            );
            draw_text(
                hdc,
                l.vol_pct,
                &format!("{}%", (self.volume * 100.0).round() as i32),
                self.fonts.small,
                DIM,
                DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
            );

            // --- status line
            let status = if let Some(err) = &self.error {
                (err.clone(), ERROR_RED)
            } else if self.scan.is_some() {
                (format!("Scanning… {} files", self.scan_count), DIM)
            } else if self.tracks.is_empty() {
                ("Open a folder to start".to_string(), DIM)
            } else {
                let mut text = format!("{} tracks", self.tracks.len());
                if !self.queue.is_empty() {
                    text.push_str(&format!(" · queue {}", self.queue.len()));
                }
                (text, DIM)
            };
            draw_text(
                hdc,
                l.status,
                &status.0,
                self.fonts.small,
                status.1,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );

            if self.scan.is_some() {
                let bar = rect(
                    l.status.left,
                    l.status.bottom - (2.0 * s).max(2.0) as i32,
                    l.status.right,
                    l.status.bottom,
                );
                fill(hdc, bar, PANEL);
                let width = (bar.right - bar.left) as f32;
                let seg = width * 0.3;
                let phase = (self.anim % 60) as f32 / 60.0;
                let x = bar.left + ((width + seg) * phase) as i32 - seg as i32;
                let seg_rect = rect(
                    x.max(bar.left),
                    bar.top,
                    (x + seg as i32).min(bar.right),
                    bar.bottom,
                );
                fill(hdc, seg_rect, ACCENT);
            }
        }
    }

    unsafe fn draw_list(&self, hdc: HDC, l: &Layout, s: f32) {
        let rows = ((l.list.bottom - l.list.top) / l.row_h).max(1) as usize;
        let total = self.visible.len();
        let visible_rows = rows.min(total.saturating_sub(self.scroll));

        unsafe {
            for i in 0..visible_rows {
                let vi = self.scroll + i;
                let idx = self.visible[vi];
                let y = l.list.top + i as i32 * l.row_h;
                let mut row = rect(l.list.left, y, l.list.right - l.scrollbar - 2, y + l.row_h);
                row.bottom -= 1;

                let is_current = self.current == Some(idx);
                let hovered = self.hover == Some(Hit::Row(vi));

                if is_current {
                    round_fill(hdc, row, SEL, (3.0 * s) as i32);
                } else if hovered {
                    round_fill(hdc, row, HOVER, (3.0 * s) as i32);
                }

                let track = &self.tracks[idx];
                let left = row.left + (7.0 * s) as i32;

                let artist_w = (((row.right - row.left) as f32) * 0.38)
                    .min(150.0 * s)
                    .max(60.0 * s) as i32;
                let title_rc = rect(left, row.top, row.right - artist_w, row.bottom);
                draw_text(
                    hdc,
                    title_rc,
                    &track.title,
                    self.fonts.ui,
                    TEXT,
                    DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                );

                if !track.artist.is_empty() {
                    let artist_rc = rect(
                        row.right - artist_w,
                        row.top,
                        row.right - (6.0 * s) as i32,
                        row.bottom,
                    );
                    draw_text(
                        hdc,
                        artist_rc,
                        &track.artist,
                        self.fonts.small,
                        DIM,
                        DT_RIGHT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
                    );
                }
            }

            if total == 0 {
                let text = if self.scan.is_some() {
                    format!("Scanning… {} files", self.scan_count)
                } else {
                    "No matches".to_string()
                };
                draw_text(
                    hdc,
                    l.list,
                    &text,
                    self.fonts.ui,
                    DIM,
                    DT_CENTER | DT_SINGLELINE | DT_VCENTER,
                );
            }

            // scrollbar
            let max_scroll = total.saturating_sub(rows);
            if max_scroll > 0 {
                let track = rect(l.list.right - l.scrollbar, l.list.top, l.list.right, l.list.bottom);
                let height = (track.bottom - track.top).max(1);
                let thumb_h = ((rows as f32 / total as f32) * height as f32).max(20.0) as i32;
                let offset = ((self.scroll as f32 / max_scroll as f32)
                    * (height - thumb_h) as f32) as i32;
                let thumb = rect(track.left, track.top + offset, track.right, track.top + offset + thumb_h);
                round_fill(hdc, thumb, TRACK_BG, 3);
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            if !self.buf_dc.0.is_null() {
                if !self.buf_old.0.is_null() {
                    SelectObject(self.buf_dc, self.buf_old);
                }
                let _ = DeleteObject(HGDIOBJ(self.buf_bmp.0));
                let _ = DeleteDC(self.buf_dc);
            }
            if let Some(cover) = self.cover.take() {
                let _ = DeleteObject(HGDIOBJ(cover.bmp.0));
            }
            let _ = DeleteObject(HGDIOBJ(self.fonts.ui.0));
            let _ = DeleteObject(HGDIOBJ(self.fonts.bold.0));
            let _ = DeleteObject(HGDIOBJ(self.fonts.small.0));
            let _ = DeleteObject(HGDIOBJ(self.fonts.icons.0));
            let _ = DeleteObject(HGDIOBJ(self.brush_input.0));
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

// ---------------------------------------------------------------- helpers

fn trim_working_set() {
    // Ask Windows to page out what the scan/decoding touched; task manager then shows the real footprint.
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e3779b97f4a7c15)
        | 1
}

fn fmt_time(secs: f64) -> String {
    let secs = secs.max(0.0) as u64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

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

fn set_tip(dst: &mut [u16; 128], text: &str) {
    let wide: Vec<u16> = text.encode_utf16().take(126).collect();
    dst[..wide.len()].copy_from_slice(&wide);
    dst[wide.len()] = 0;
}

unsafe fn make_icon() -> HICON {
    let (rgba, w, h) = icon_rgba();
    unsafe {
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32);
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut bits: *mut c_void = null_mut();
        let Ok(color) = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) else {
            return HICON::default();
        };
        std::ptr::copy_nonoverlapping(rgba.as_ptr(), bits as *mut u8, rgba.len());

        let mask = CreateBitmap(w as i32, h as i32, 1, 1, None);
        let info = ICONINFO {
            fIcon: windows::core::BOOL::from(true),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}

/// 32x32 rounded dark tile with a play triangle.
fn icon_rgba() -> (Vec<u8>, u32, u32) {
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
                continue;
            }
            let tri = x as f32 >= 11.0
                && x as f32 <= 23.0
                && (y as f32 - 16.0).abs() <= 7.0 - (x as f32 - 11.0) * 0.29;
            let (r_, g_, b_) = if tri { (235, 240, 250) } else { (26, 26, 31) };
            let i = ((y * S + x) * 4) as usize;
            rgba[i] = r_;
            rgba[i + 1] = g_;
            rgba[i + 2] = b_;
            rgba[i + 3] = 255;
        }
    }
    (rgba, S, S)
}

// ---------------------------------------------------------------- window

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_CREATE => {
                let app = Box::new(App::new(hwnd));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
                SetTimer(Some(hwnd), TIMER_ID, 200, None);
                trim_working_set();
                LRESULT(0)
            }
            WM_DESTROY => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
                if !ptr.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let mut app = Box::from_raw(ptr);
                    app.save_settings();
                    app.remove_tray();
                }
                let _ = KillTimer(Some(hwnd), TIMER_ID);
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
                let Some(app) = ptr.as_mut() else {
                    return DefWindowProcW(hwnd, msg, wp, lp);
                };

                match msg {
                    WM_PAINT => {
                        app.paint(hwnd);
                        LRESULT(0)
                    }
                    WM_ERASEBKGND => LRESULT(1),
                    WM_TIMER => {
                        app.tick();
                        // Idle UI is static; only repaint while something is moving.
                        if (app.playing || app.scan.is_some()) && IsWindowVisible(hwnd).as_bool() {
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                        LRESULT(0)
                    }
                    WM_SIZE => {
                        if wp.0 as u32 == SIZE_MINIMIZED {
                            trim_working_set();
                        }
                        let l = app.layout();
                        let edit = l.edit_rect(app.scale());
                        let w = (edit.right - edit.left).max(10);
                        let h = (edit.bottom - edit.top).max(10);
                        let _ = MoveWindow(app.edit, edit.left, edit.top, w, h, true);
                        // Vertically centre the single-line edit text.
                        let mut fmt = rect(0, ((24.0 - 14.0) * app.scale() / 2.0) as i32, w, h);
                        SendMessageW(
                            app.edit,
                            EM_SETRECT,
                            Some(WPARAM(0)),
                            Some(LPARAM(&mut fmt as *mut RECT as isize)),
                        );
                        let _ = InvalidateRect(Some(hwnd), None, false);
                        LRESULT(0)
                    }
                    WM_NCCALCSIZE => {
                        // Client area covers the whole window; we draw our own title bar.
                        if wp.0 != 0 && IsZoomed(hwnd).as_bool() {
                            let params = &mut *(lp.0 as *mut NCCALCSIZE_PARAMS);
                            let dpi = GetDpiForWindow(hwnd);
                            let bx = GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi)
                                + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                            let by = GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi)
                                + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi);
                            params.rgrc[0].left += bx;
                            params.rgrc[0].right -= bx;
                            params.rgrc[0].top += by;
                            params.rgrc[0].bottom -= by;
                        }
                        LRESULT(0)
                    }
                    WM_NCHITTEST => {
                        let mut pt = POINT {
                            x: (lp.0 & 0xffff) as i16 as i32,
                            y: ((lp.0 >> 16) & 0xffff) as i16 as i32,
                        };
                        let _ = ScreenToClient(hwnd, &mut pt);
                        LRESULT(app.nchit(pt.x, pt.y) as isize)
                    }
                    WM_MOUSEMOVE => {
                        let x = (lp.0 & 0xffff) as i16 as i32;
                        let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
                        app.on_mouse_move(x, y);
                        LRESULT(0)
                    }
                    WM_LBUTTONDOWN => {
                        let x = (lp.0 & 0xffff) as i16 as i32;
                        let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
                        app.on_mouse_down(x, y);
                        LRESULT(0)
                    }
                    WM_LBUTTONDBLCLK => {
                        let x = (lp.0 & 0xffff) as i16 as i32;
                        let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
                        app.on_double_click(x, y);
                        LRESULT(0)
                    }
                    WM_LBUTTONUP => {
                        app.on_mouse_up();
                        LRESULT(0)
                    }
                    WM_RBUTTONUP => {
                        let x = (lp.0 & 0xffff) as i16 as i32;
                        let y = ((lp.0 >> 16) & 0xffff) as i16 as i32;
                        if let Some(Hit::Row(row)) = app.hit_test(x, y) {
                            app.context_menu_row(row);
                        }
                        LRESULT(0)
                    }
                    WM_MOUSEWHEEL => {
                        let delta = ((wp.0 >> 16) & 0xffff) as i16 as i32;
                        app.on_wheel(delta);
                        LRESULT(0)
                    }
                    WM_KEYDOWN => {
                        app.on_key(wp.0 as u32);
                        LRESULT(0)
                    }
                    WM_COMMAND => {
                        let code = ((wp.0 >> 16) & 0xffff) as u32;
                        if code == EN_CHANGE_CODE && HWND(lp.0 as *mut c_void) == app.edit {
                            app.on_search_changed();
                        }
                        LRESULT(0)
                    }
                    WM_CTLCOLOREDIT => {
                        if HWND(lp.0 as *mut c_void) == app.edit {
                            let hdc = HDC(wp.0 as *mut c_void);
                            SetTextColor(hdc, TEXT);
                            SetBkColor(hdc, BG);
                            return LRESULT(app.brush_input.0 as isize);
                        }
                        DefWindowProcW(hwnd, msg, wp, lp)
                    }
                    WM_TRAY => {
                        let event = (lp.0 & 0xffff) as u32;
                        match event {
                            WM_RBUTTONUP | WM_CONTEXTMENU => app.tray_menu(hwnd),
                            WM_LBUTTONUP | WM_LBUTTONDBLCLK => {
                                let _ = ShowWindow(hwnd, SW_SHOW);
                                let _ = ShowWindow(hwnd, SW_RESTORE);
                                let _ = SetForegroundWindow(hwnd);
                            }
                            _ => {}
                        }
                        LRESULT(0)
                    }
                    WM_CLOSE => {
                        // Closing hides to tray; exit lives in the tray menu.
                        let _ = ShowWindow(hwnd, SW_HIDE);
                        trim_working_set();
                        LRESULT(0)
                    }
                    WM_GETMINMAXINFO => {
                        if lp.0 != 0 {
                            let info = &mut *(lp.0 as *mut MINMAXINFO);
                            info.ptMinTrackSize.x = (400.0 * app.scale()) as i32;
                            info.ptMinTrackSize.y = (320.0 * app.scale()) as i32;
                        }
                        LRESULT(0)
                    }
                    WM_SETCURSOR => {
                        let h = app.hover;
                        if matches!(
                            h,
                            Some(
                                Hit::Open
                                    | Hit::Shuffle
                                    | Hit::Prev
                                    | Hit::Play
                                    | Hit::Next
                                    | Hit::Min
                                    | Hit::Max
                                    | Hit::Close
                            )
                        ) {
                            if let Ok(cursor) = LoadCursorW(None, IDC_HAND) {
                                SetCursor(Some(cursor));
                            }
                            return LRESULT(1);
                        }
                        DefWindowProcW(hwnd, msg, wp, lp)
                    }
                    _ => DefWindowProcW(hwnd, msg, wp, lp),
                }
            }
        }
    }
}

fn main() -> windows::core::Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let instance = GetModuleHandleW(None)?;
        let class_name = w_ptr("JustMusicWindow");

        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            return Err(windows::core::Error::from_thread());
        }

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w_ptr("Just Music"),
            WS_POPUP | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            420,
            610,
            None,
            None,
            Some(HINSTANCE(instance.0)),
            None,
        )?;

        // Popup windows ignore CW_USEDEFAULT; centre inside the work area instead.
        let mut work = RECT::default();
        let _ = SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut work as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
        let _ = SetWindowPos(
            hwnd,
            None,
            work.left + (work.right - work.left - 420) / 2,
            work.top + (work.bottom - work.top - 610) / 2,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        // Keep keyboard focus on the window, not the search box.
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
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
    fn bars_only_respond_near_the_line() {
        let bar = rect(10, 50, 110, 68); // 18px row, 4-6px visible bar
        assert!(bar_hit(bar, 50, 59, 1.0));
        assert!(bar_hit(bar, 50, 56, 1.0));
        assert!(!bar_hit(bar, 50, 65, 1.0));
        assert!(!bar_hit(bar, 50, 51, 1.0));
        assert!(!bar_hit(bar, 130, 59, 1.0));
    }

    #[test]
    fn time_formatting() {
        assert_eq!(fmt_time(0.0), "0:00");
        assert_eq!(fmt_time(65.9), "1:05");
    }
}
