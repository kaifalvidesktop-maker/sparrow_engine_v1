//! Sparrow paint: layout items -> RGBA pixels (tiny-skia + built-in 5x7 bitmap font).

use crate::dro_css::Color;
use crate::dro_layout::{cells, is_virama, Layout, Paint};
use std::collections::HashMap;
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Rect, Transform};

/// Horizontal advance of one character cell. `dro_layout` uses this for
/// measuring, so it must stay in sync with how `text()` places glyphs.
pub fn advance(size: f32) -> f32 {
    (size * 0.6).max(4.0)
}

// 5x7 bitmap font, ASCII 32..=126 (column bytes, LSB = top row)
// NOTE: the last row used to be an arrow glyph; '~' is now a real tilde.
const FONT: [[u8; 5]; 95] = [
    [0, 0, 0, 0, 0], [0, 0, 0x5F, 0, 0], [0, 7, 0, 7, 0], [0x14, 0x7F, 0x14, 0x7F, 0x14],
    [0x24, 0x2A, 0x7F, 0x2A, 0x12], [0x23, 0x13, 8, 0x64, 0x62], [0x36, 0x49, 0x55, 0x22, 0x50], [0, 5, 3, 0, 0],
    [0, 0x1C, 0x22, 0x41, 0], [0, 0x41, 0x22, 0x1C, 0], [0x14, 8, 0x3E, 8, 0x14], [8, 8, 0x3E, 8, 8],
    [0, 0x50, 0x30, 0, 0], [8, 8, 8, 8, 8], [0, 0x60, 0x60, 0, 0], [0x20, 0x10, 8, 4, 2],
    [0x3E, 0x51, 0x49, 0x45, 0x3E], [0, 0x42, 0x7F, 0x40, 0], [0x42, 0x61, 0x51, 0x49, 0x46], [0x21, 0x41, 0x45, 0x4B, 0x31],
    [0x18, 0x14, 0x12, 0x7F, 0x10], [0x27, 0x45, 0x45, 0x45, 0x39], [0x3C, 0x4A, 0x49, 0x49, 0x30], [1, 0x71, 9, 5, 3],
    [0x36, 0x49, 0x49, 0x49, 0x36], [6, 0x49, 0x49, 0x29, 0x1E], [0, 0x36, 0x36, 0, 0], [0, 0x56, 0x36, 0, 0],
    [8, 0x14, 0x22, 0x41, 0], [0x14, 0x14, 0x14, 0x14, 0x14], [0, 0x41, 0x22, 0x14, 8], [2, 1, 0x51, 9, 6],
    [0x32, 0x49, 0x79, 0x41, 0x3E], [0x7E, 0x11, 0x11, 0x11, 0x7E], [0x7F, 0x49, 0x49, 0x49, 0x36], [0x3E, 0x41, 0x41, 0x41, 0x22],
    [0x7F, 0x41, 0x41, 0x22, 0x1C], [0x7F, 0x49, 0x49, 0x49, 0x41], [0x7F, 9, 9, 9, 1], [0x3E, 0x41, 0x49, 0x49, 0x7A],
    [0x7F, 8, 8, 8, 0x7F], [0, 0x41, 0x7F, 0x41, 0], [0x20, 0x40, 0x41, 0x3F, 1], [0x7F, 8, 0x14, 0x22, 0x41],
    [0x7F, 0x40, 0x40, 0x40, 0x40], [0x7F, 2, 0x0C, 2, 0x7F], [0x7F, 4, 8, 0x10, 0x7F], [0x3E, 0x41, 0x41, 0x41, 0x3E],
    [0x7F, 9, 9, 9, 6], [0x3E, 0x41, 0x51, 0x21, 0x5E], [0x7F, 9, 0x19, 0x29, 0x46], [0x46, 0x49, 0x49, 0x49, 0x31],
    [1, 1, 0x7F, 1, 1], [0x3F, 0x40, 0x40, 0x40, 0x3F], [0x1F, 0x20, 0x40, 0x20, 0x1F], [0x3F, 0x40, 0x38, 0x40, 0x3F],
    [0x63, 0x14, 8, 0x14, 0x63], [7, 8, 0x70, 8, 7], [0x61, 0x51, 0x49, 0x45, 0x43], [0, 0x7F, 0x41, 0x41, 0],
    [2, 4, 8, 0x10, 0x20], [0, 0x41, 0x41, 0x7F, 0], [4, 2, 1, 2, 4], [0x40, 0x40, 0x40, 0x40, 0x40],
    [0, 1, 2, 4, 0], [0x20, 0x54, 0x54, 0x54, 0x78], [0x7F, 0x48, 0x44, 0x44, 0x38], [0x38, 0x44, 0x44, 0x44, 0x20],
    [0x38, 0x44, 0x44, 0x48, 0x7F], [0x38, 0x54, 0x54, 0x54, 0x18], [8, 0x7E, 9, 1, 2], [8, 0x14, 0x54, 0x54, 0x3C],
    [0x7F, 8, 4, 4, 0x78], [0, 0x44, 0x7D, 0x40, 0], [0x20, 0x40, 0x44, 0x3D, 0], [0x7F, 0x10, 0x28, 0x44, 0],
    [0, 0x41, 0x7F, 0x40, 0], [0x7C, 4, 0x18, 4, 0x78], [0x7C, 8, 4, 4, 0x78], [0x38, 0x44, 0x44, 0x44, 0x38],
    [0x7C, 0x14, 0x14, 0x14, 8], [8, 0x14, 0x14, 0x18, 0x7C], [0x7C, 8, 4, 4, 8], [0x48, 0x54, 0x54, 0x54, 0x20],
    [4, 0x3F, 0x44, 0x40, 0x20], [0x3C, 0x40, 0x40, 0x20, 0x7C], [0x1C, 0x20, 0x40, 0x20, 0x1C], [0x3C, 0x40, 0x30, 0x40, 0x3C],
    [0x44, 0x28, 0x10, 0x28, 0x44], [0x0C, 0x50, 0x50, 0x50, 0x3C], [0x44, 0x64, 0x54, 0x4C, 0x44], [0, 8, 0x36, 0x41, 0],
    [0, 0, 0x7F, 0, 0], [0, 0x41, 0x36, 8, 0], [2, 1, 2, 4, 2],
];

/// Map common non-ASCII characters onto something the ASCII bitmap font can draw.
fn fold(ch: char) -> char {
    match ch {
        '\u{a0}' => ' ',
        '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{2032}' => '\'',
        '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{2033}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{2022}' | '\u{25cf}' | '\u{25aa}' => '*',
        '\u{b7}' | '\u{2026}' => '.',
        '\u{d7}' => 'x',
        'À'..='Å' => 'A',
        'Ç' => 'C',
        'È'..='Ë' => 'E',
        'Ì'..='Ï' => 'I',
        'Ñ' => 'N',
        'Ò'..='Ö' | 'Ø' => 'O',
        'Ù'..='Ü' => 'U',
        'Ý' => 'Y',
        'à'..='å' => 'a',
        'ç' => 'c',
        'è'..='ë' => 'e',
        'ì'..='ï' => 'i',
        'ñ' => 'n',
        'ò'..='ö' | 'ø' => 'o',
        'ù'..='ü' => 'u',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// Index into FONT; unsupported glyphs fall back to '?' (index 31).
fn glyph_index(ch: char) -> usize {
    let ch = fold(ch);
    if (' '..='~').contains(&ch) { ch as usize - 32 } else { 31 }
}

/// Decode PNG/JPEG/GIF into a premultiplied pixmap. Oversized images are rejected before decoding.
pub fn decode_image(bytes: &[u8]) -> Result<Pixmap, String> {
    const MAX_PX: u64 = 16_000_000;
    let reader = || image::io::Reader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string());
    let (w, h) = reader()?.into_dimensions().map_err(|e| e.to_string())?;
    if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_PX { return Err(format!("image too large: {w}x{h}")); }
    let mut data = image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8().into_raw();
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 255 {
            px[0] = ((px[0] as u32 * a + 127) / 255) as u8;
            px[1] = ((px[1] as u32 * a + 127) / 255) as u8;
            px[2] = ((px[2] as u32 * a + 127) / 255) as u8;
        }
    }
    let size = tiny_skia::IntSize::from_wh(w, h).ok_or("bad image size")?;
    Pixmap::from_vec(data, size).ok_or_else(|| "pixmap alloc failed".to_string())
}

fn rect(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: Color) {
    if ![x, y, w, h].iter().all(|v| v.is_finite()) { return; }
    // Snap to whole pixels: avoids tiny-skia's AA/hairline path (debug assert + slow).
    let (x0, y0) = (x.round(), y.round());
    let (w, h) = (((x + w).round() - x0).max(1.0), ((y + h).round() - y0).max(1.0));
    if let Some(r) = Rect::from_xywh(x0, y0, w, h) {
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(c.0, c.1, c.2, 255);
        p.anti_alias = false;
        pm.fill_rect(r, &p, Transform::identity(), None);
    }
}

fn text(pm: &mut Pixmap, t: &str, x: f32, y: f32, h: f32, size: f32, bold: bool, c: Color) {
    // Real shaped text (Bangla etc.) when system fonts exist; the bitmap font below is the fallback.
    if crate::dro_font::draw_text(pm, t, x, y, h, size, bold, (c.0, c.1, c.2)) { return; }
    let adv = advance(size);
    let s = adv / 6.0;
    let top = y + (h - 7.0 * s) / 2.0;
    let (pw, ph) = (pm.width() as f32, pm.height() as f32);
    if top > ph || top + 7.0 * s < 0.0 { return; }
    let stripe = if bold { s * 1.6 } else { s };

    let (mut cx, mut prev_v) = (x, false);
    for ch in t.chars() {
        // Same width model as the layout: marks are zero-width, conjuncts half, wide = 2 cells.
        let cw = cells(ch, prev_v) * adv;
        prev_v = is_virama(ch);
        if cw <= 0.0 { continue; } // combining marks: don't draw a bogus '?'
        let gx = cx;
        cx += cw;
        if gx > pw { break; }
        if gx + cw < 0.0 { continue; }

        let glyph = &FONT[glyph_index(ch)];
        for (col, &bits) in glyph.iter().enumerate() {
            let mut row = 0;
            while row < 7 {
                if (bits >> row) & 1 == 1 {
                    let start = row;
                    while row < 7 && (bits >> row) & 1 == 1 { row += 1; }
                    rect(pm, gx + col as f32 * s, top + start as f32 * s, stripe, (row - start) as f32 * s, c);
                } else {
                    row += 1;
                }
            }
        }
    }
}

pub fn paint(l: &Layout, w: u32, h: u32, scroll: f32, images: &HashMap<String, Pixmap>) -> Result<Vec<u8>, String> {
    let (w, h) = (w.max(1), h.max(1));
    let mut pm = Pixmap::new(w, h).ok_or("pixmap alloc failed")?;
    pm.fill(tiny_skia::Color::from_rgba8(l.bg.0, l.bg.1, l.bg.2, 255));
    let scroll = if scroll.is_finite() { scroll.max(0.0) } else { 0.0 };
    let (fw, fh) = (w as f32, h as f32);

    for it in &l.items {
        let y = it.y - scroll;
        // Cull vertically and horizontally.
        if y + it.h < 0.0 || y > fh || it.x > fw || it.x + it.w < 0.0 { continue; }
        match &it.paint {
            Paint::Box(c) => {
                // rect() enforces a 1px minimum for text strokes; empty boxes must stay invisible.
                if it.w > 0.0 && it.h > 0.0 { rect(&mut pm, it.x, y, it.w, it.h, *c); }
            }
            Paint::Image { src, .. } => {
                if it.w <= 0.0 || it.h <= 0.0 { continue; }
                match images.get(src) {
                    Some(img) if img.width() > 0 && img.height() > 0 => {
                        let (sx, sy) = (it.w / img.width() as f32, it.h / img.height() as f32);
                        let mut pp = PixmapPaint::default();
                        pp.quality = if (sx - 1.0).abs() < 1e-3 && (sy - 1.0).abs() < 1e-3 { FilterQuality::Nearest } else { FilterQuality::Bilinear };
                        pm.draw_pixmap(0, 0, img.as_ref(), &pp, Transform::from_row(sx, 0.0, 0.0, sy, it.x, y), None);
                    }
                    _ => {
                        // not loaded yet: dark box for media, light box for pictures
                        let c = if src.starts_with("media:") { Color(20, 20, 20) } else { Color(235, 235, 235) };
                        rect(&mut pm, it.x, y, it.w, it.h, c);
                    }
                }
            }
            Paint::Word { text: t, color, size, bold, link } => {
                text(&mut pm, t, it.x, y, it.h, *size, *bold, *color);
                if link.is_some() {
                    let th = (*size / 14.0).round().max(1.0);
                    rect(&mut pm, it.x, y + it.h - th - 1.0, it.w, th, *color);
                }
            }
        }
    }
    Ok(pm.take())
}
