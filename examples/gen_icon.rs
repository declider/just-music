//! Regenerates `assets/icon.ico`, the icon embedded into the executable.
//!
//! `cargo run --release --example gen_icon`

use std::io::Cursor;

use image::{ImageFormat, Rgba, RgbaImage};

/// Sizes Windows asks for: tray 16, taskbar 32, alt-tab 48, Explorer up to 256.
const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];
/// Supersampling factor before downscaling each frame.
const SS: u32 = 4;

const TILE_TOP: [f32; 3] = [32.0, 34.0, 42.0]; // #20222a
const TILE_BOTTOM: [f32; 3] = [15.0, 16.0, 20.0]; // #0f1014
const TRIANGLE: [f32; 3] = [90.0, 160.0, 255.0]; // #5aa0ff, same as the UI accent

fn render(size: u32) -> RgbaImage {
    let n = size * SS;
    let s = n as f32;
    // Small sizes get a squarer tile and a bolder triangle so the shape survives
    // at 16-24 px in the tray.
    let small = size <= 24;
    let radius = if small { 0.15 } else { 0.215 } * s;
    let (left, half, apex) = if small {
        (0.300, 0.285, 0.760)
    } else {
        (0.360, 0.232, 0.706)
    };
    let tri = [
        (left * s, (0.5 - half) * s),
        (left * s, (0.5 + half) * s),
        (apex * s, 0.500 * s),
    ];

    let mut img = RgbaImage::new(n, n);
    for y in 0..n {
        for x in 0..n {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            // Vertical gradient, also used as the colour of transparent pixels
            // so downscaling cannot pull dark fringes into the edges.
            let t = py / s;
            let tile = [
                TILE_TOP[0] + (TILE_BOTTOM[0] - TILE_TOP[0]) * t,
                TILE_TOP[1] + (TILE_BOTTOM[1] - TILE_TOP[1]) * t,
                TILE_TOP[2] + (TILE_BOTTOM[2] - TILE_TOP[2]) * t,
            ];

            let cx = px.clamp(radius, s - radius);
            let cy = py.clamp(radius, s - radius);
            let inside_tile = (px - cx).powi(2) + (py - cy).powi(2) <= radius * radius;
            if !inside_tile {
                img.put_pixel(x, y, Rgba([tile[0] as u8, tile[1] as u8, tile[2] as u8, 0]));
                continue;
            }

            let color = if in_triangle(px, py, tri) {
                TRIANGLE
            } else {
                tile
            };
            img.put_pixel(
                x,
                y,
                Rgba([color[0] as u8, color[1] as u8, color[2] as u8, 255]),
            );
        }
    }

    image::imageops::resize(&img, size, size, image::imageops::FilterType::Triangle)
}

fn in_triangle(px: f32, py: f32, tri: [(f32, f32); 3]) -> bool {
    let sign = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (a.0 - c.0) * (b.1 - c.1) - (b.0 - c.0) * (a.1 - c.1)
    };
    let p = (px, py);
    let d1 = sign(p, tri[0], tri[1]);
    let d2 = sign(p, tri[1], tri[2]);
    let d3 = sign(p, tri[2], tri[0]);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(has_neg && has_pos)
}

fn main() {
    let mut frames = Vec::new();
    for size in SIZES {
        let img = render(size);
        let mut png = Cursor::new(Vec::new());
        img.write_to(&mut png, ImageFormat::Png)
            .expect("encode icon frame");
        frames.push((size, png.into_inner()));
    }

    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type: icon
    ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());

    let mut offset = 6 + 16 * frames.len() as u32;
    for (size, data) in &frames {
        let dim = if *size >= 256 { 0 } else { *size as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &frames {
        ico.extend_from_slice(data);
    }

    std::fs::write("assets/icon.ico", &ico).expect("write assets/icon.ico");
    println!("assets/icon.ico: {} bytes, {} frames", ico.len(), frames.len());
}
