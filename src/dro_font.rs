//! dro_font v2 — real text for Sparrow: system fonts (fontdb), OpenType shaping (rustybuzz, Bangla conjuncts),
//! bidi (unicode-bidi), CSS font-family lists, weights 100-900, italic (real or synthetic), web fonts (TTF/OTF),
//! glyph-mask cache with 1/4-pixel horizontal positioning, caret/cluster positions (for text input),
//! letter-spacing, underline/strike, line-break opportunities, RAM-light lazy face loading.
//! Cargo: unicode-bidi = "0.3", unicode-linebreak = "0.1" (plus existing fontdb, rustybuzz, tiny-skia).
//! Old API kept: Fonts::{new,count,has_fonts,shape,measure,metrics,draw}, with/available/measure/draw_text.
//! NOT yet: WOFF/WOFF2 (decompress first), colour emoji, vertical text, hinting, LCD subpixel AA.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Rect, Transform};

const WEB_MAX_BYTES: usize = 32 * 1024 * 1024;
const GLYPH_CACHE_MAX: usize = 6000;
const GLYPH_CACHE_BYTES: usize = 12 * 1024 * 1024;
const RASTER_MAX_SIZE: f32 = 160.0; // bigger text is filled directly (no mask cache)
const SUBPIXEL: u8 = 4;
const SANS: &[&str] = &["Segoe UI", "Arial", "Noto Sans", "DejaVu Sans", "Liberation Sans", "FreeSans", "Helvetica"];
const SERIF: &[&str] = &["Times New Roman", "Georgia", "Noto Serif", "DejaVu Serif", "Liberation Serif", "FreeSerif"];
const MONO: &[&str] = &["Consolas", "Cascadia Mono", "Courier New", "Noto Sans Mono", "DejaVu Sans Mono", "Liberation Mono", "FreeMono"];
const BENGALI: &[&str] = &["Nirmala UI", "Vrinda", "Shonar Bangla", "Noto Sans Bengali", "Siyam Rupali", "Lohit Bengali", "FreeSans", "Noto Serif Bengali"];
const OTHER: &[&str] = &["Segoe UI Symbol", "Noto Sans Symbols", "Microsoft YaHei", "Noto Sans CJK SC", "Yu Gothic", "Malgun Gothic", "Segoe UI Emoji", "Arial Unicode MS", "Noto Sans Devanagari", "Nirmala UI", "Segoe UI Historic", "DejaVu Sans", "Unifont"];

// ---------------------------------------------------------------- public types
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle { pub size: f32, pub weight: u16, pub italic: bool, pub families: Vec<String>, pub letter_spacing: f32 }
impl TextStyle {
    pub fn new(size: f32) -> Self { TextStyle { size: size.max(1.0).min(2000.0), weight: 400, italic: false, families: vec![], letter_spacing: 0.0 } }
    pub fn bold(mut self, b: bool) -> Self { self.weight = if b { 700 } else { 400 }; self }
    pub fn weight(mut self, w: u16) -> Self { self.weight = w.clamp(100, 900); self }
    pub fn italic(mut self, i: bool) -> Self { self.italic = i; self }
    pub fn families(mut self, css: &str) -> Self { self.families = parse_families(css); self }
    pub fn spacing(mut self, px: f32) -> Self { self.letter_spacing = if px.is_finite() { px.clamp(-20.0, 100.0) } else { 0.0 }; self }
}

/// Parse a CSS `font-family` value: `Roboto, "Helvetica Neue", sans-serif`.
pub fn parse_families(css: &str) -> Vec<String> {
    let (mut out, mut cur, mut q) = (Vec::<String>::new(), String::new(), None::<char>);
    for c in css.chars() {
        match (q, c) {
            (None, '"') | (None, '\'') => q = Some(c),
            (Some(x), y) if x == y => q = None,
            (None, ',') => { let t = cur.trim().to_string(); if !t.is_empty() { out.push(t); } cur.clear(); }
            _ => cur.push(c),
        }
    }
    let t = cur.trim().to_string();
    if !t.is_empty() { out.push(t); }
    out.truncate(12);
    out
}

#[derive(Clone, Copy, Debug)]
pub struct G { pub face: usize, pub id: u16, pub x: f32, pub y: f32, pub cluster: usize, pub adv: f32, pub rtl: bool }

#[derive(Clone)]
pub struct Shaped { pub glyphs: Vec<G>, pub width: f32, pub size: f32, pub weight: u16, pub italic: bool }

#[derive(Clone, Copy, Debug)]
pub struct Metrics { pub ascent: f32, pub descent: f32, pub line_gap: f32, pub x_height: f32, pub underline_pos: f32, pub underline_thick: f32 }

// ---------------------------------------------------------------- internals
struct FaceData {
    hb: rustybuzz::Face<'static>, // must be declared BEFORE `_data` (dropped first)
    _data: Box<[u8]>,
    upem: f32,
    weight: u16,
    italic: bool,
}
fn make_face(data: Vec<u8>, index: u32, weight: u16, italic: bool) -> Option<FaceData> {
    let data = data.into_boxed_slice();
    // SAFETY: the Box's heap buffer is never mutated or moved out, and `_data` is dropped after `hb`.
    let slice: &'static [u8] = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(&data[..]) };
    let hb = rustybuzz::Face::from_slice(slice, index)?;
    let upem = (hb.units_per_em() as f32).max(1.0);
    Some(FaceData { hb, _data: data, upem, weight, italic })
}

struct GlyphMask { w: u32, h: u32, left: i32, top: i32, a: Vec<u8> }
#[derive(Clone, PartialEq, Eq, Hash)] struct MaskKey { face: usize, gid: u16, size_bits: u32, sub: u8, skew: bool }
#[derive(Clone, PartialEq, Eq, Hash)] struct ChainKey { fam: String, w: u16, italic: bool }
#[derive(Clone, PartialEq, Eq, Hash)] struct ShapeKey { text: String, size_bits: u32, w: u16, italic: bool, fam: String, ls: i32 }
#[derive(Clone, Copy)] struct WebFace { id: fontdb::ID, weight: u16, italic: bool }

struct Pb(PathBuilder);
impl rustybuzz::ttf_parser::OutlineBuilder for Pb {
    fn move_to(&mut self, x: f32, y: f32) { self.0.move_to(x, y) }
    fn line_to(&mut self, x: f32, y: f32) { self.0.line_to(x, y) }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) { self.0.quad_to(x1, y1, x, y) }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) { self.0.cubic_to(x1, y1, x2, y2, x, y) }
    fn close(&mut self) { self.0.close() }
}

fn joiner(c: char) -> bool { matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}') }
fn has_rtl(t: &str) -> bool { t.chars().any(|c| matches!(c as u32, 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)) }

/// Visual-order runs `(byte range, is_rtl)`. Pure LTR text takes the fast path.
pub fn bidi_runs(text: &str) -> Vec<(Range<usize>, bool)> {
    if !has_rtl(text) { return vec![(0..text.len(), false)]; }
    let info = unicode_bidi::BidiInfo::new(text, None);
    let mut out = vec![];
    for para in &info.paragraphs {
        let (levels, runs) = info.visual_runs(para, para.range.clone());
        for r in runs { let rtl = levels[r.start].is_rtl(); out.push((r, rtl)); }
    }
    if out.is_empty() { out.push((0..text.len(), false)); }
    out
}

/// Byte offsets where a line may break (`true` = forced break). UAX #14.
pub fn line_breaks(text: &str) -> Vec<(usize, bool)> {
    unicode_linebreak::linebreaks(text).map(|(i, o)| (i, matches!(o, unicode_linebreak::BreakOpportunity::Mandatory))).collect()
}

/// Nearest caret stop (byte index) to pixel `x`.
pub fn index_at_x(stops: &[(usize, f32)], x: f32) -> usize {
    stops.iter().min_by(|a, b| (a.1 - x).abs().partial_cmp(&(b.1 - x).abs()).unwrap_or(std::cmp::Ordering::Equal)).map_or(0, |s| s.0)
}

fn family_candidates(name: &str) -> Option<&'static [&'static str]> {
    match name.to_ascii_lowercase().as_str() {
        "sans-serif" | "system-ui" | "ui-sans-serif" | "-apple-system" | "blinkmacsystemfont" | "cursive" | "fantasy" | "emoji" | "math" | "fangsong" => Some(SANS),
        "serif" | "ui-serif" => Some(SERIF),
        "monospace" | "ui-monospace" => Some(MONO),
        _ => None,
    }
}
fn push_unique(v: &mut Vec<fontdb::ID>, id: fontdb::ID) { if !v.contains(&id) { v.push(id); } }

fn raster_glyph(path: &Path, sc: f32, skew: bool, sub: f32) -> Option<GlyphMask> {
    let kx = if skew { 0.2 * sc } else { 0.0 };
    let p = path.clone().transform(Transform::from_row(sc, 0.0, kx, -sc, 0.0, 0.0))?;
    let b = p.bounds();
    let left = b.left().floor() as i32 - 1;
    let top = b.top().floor() as i32 - 1;
    let w = (b.right() - left as f32 + 1.0).ceil() as i32 + 1;
    let h = (b.bottom() - top as f32).ceil() as i32 + 1;
    if w <= 0 || h <= 0 || w > 512 || h > 512 { return None; }
    let mut pm = Pixmap::new(w as u32, h as u32)?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    pm.fill_path(&p, &paint, FillRule::Winding, Transform::from_translate(-left as f32 + sub, -top as f32), None);
    let a: Vec<u8> = pm.data().chunks_exact(4).map(|c| c[3]).collect();
    Some(GlyphMask { w: w as u32, h: h as u32, left, top, a })
}

/// Source-over blend of a coverage mask onto premultiplied RGBA8; clipped to the pixmap.
fn blit(pm: &mut Pixmap, m: &GlyphMask, ox: i32, oy: i32, rgb: (u8, u8, u8)) {
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let (r, g, b) = (rgb.0 as u32, rgb.1 as u32, rgb.2 as u32);
    for my in 0..m.h as i32 {
        let y = oy + m.top + my;
        if y < 0 || y >= ph { continue; }
        for mx in 0..m.w as i32 {
            let a = m.a[(my as u32 * m.w + mx as u32) as usize] as u32;
            if a == 0 { continue; }
            let x = ox + m.left + mx;
            if x < 0 || x >= pw { continue; }
            let i = ((y * pw + x) * 4) as usize;
            let inv = 255 - a;
            data[i] = ((r * a + data[i] as u32 * inv + 127) / 255) as u8;
            data[i + 1] = ((g * a + data[i + 1] as u32 * inv + 127) / 255) as u8;
            data[i + 2] = ((b * a + data[i + 2] as u32 * inv + 127) / 255) as u8;
            data[i + 3] = ((255 * a + data[i + 3] as u32 * inv + 127) / 255) as u8;
        }
    }
}

// ---------------------------------------------------------------- Fonts
pub struct Fonts {
    db: fontdb::Database,
    faces: Vec<Option<FaceData>>,
    by_id: HashMap<fontdb::ID, usize>,
    chains: HashMap<ChainKey, Vec<fontdb::ID>>,
    web: HashMap<String, Vec<WebFace>>,
    web_bytes: usize,
    paths: HashMap<(usize, u16), Option<Path>>,
    shaped: HashMap<ShapeKey, Shaped>,
    masks: HashMap<MaskKey, Option<Arc<GlyphMask>>>,
    mask_bytes: usize,
}

impl Fonts {
    pub fn new() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Fonts { db, faces: vec![], by_id: HashMap::new(), chains: HashMap::new(), web: HashMap::new(), web_bytes: 0, paths: HashMap::new(), shaped: HashMap::new(), masks: HashMap::new(), mask_bytes: 0 }
    }

    /// Number of fonts found on this system (plus registered web fonts).
    pub fn count(&self) -> usize { self.db.len() }
    /// Faces actually loaded into RAM (lazy: only fonts that were needed).
    pub fn loaded_faces(&self) -> usize { self.faces.iter().filter(|f| f.is_some()).count() }
    fn face(&self, i: usize) -> Option<&FaceData> { self.faces.get(i)?.as_ref() }

    fn load(&mut self, id: fontdb::ID) -> Option<usize> {
        if let Some(&i) = self.by_id.get(&id) { return Some(i); }
        let (data, index) = self.db.with_face_data(id, |d, i| (d.to_vec(), i))?;
        let sys = self.db.face(id).map(|f| (f.weight.0, !matches!(f.style, fontdb::Style::Normal))).unwrap_or((400, false));
        let ov = self.web.values().flat_map(|v| v.iter()).find(|w| w.id == id).map(|w| (w.weight, w.italic));
        let (weight, italic) = ov.unwrap_or(sys);
        let fd = make_face(data, index, weight, italic)?;
        self.faces.push(Some(fd));
        self.by_id.insert(id, self.faces.len() - 1);
        Some(self.faces.len() - 1)
    }

    fn find_family(&self, name: &str, weight: u16, italic: bool) -> Option<fontdb::ID> {
        if let Some(v) = self.web.get(&name.trim().to_ascii_lowercase()) {
            return v.iter().min_by_key(|w| (w.italic != italic, (w.weight as i32 - weight as i32).abs())).map(|w| w.id);
        }
        let q = fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight: fontdb::Weight(weight),
            stretch: fontdb::Stretch::Normal,
            style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal },
        };
        let id = self.db.query(&q)?;
        // query() falls back to any face when the family is missing: require the same family name
        let ok = self.db.face(id).map_or(false, |f| f.families.iter().any(|(fam, _)| fam.eq_ignore_ascii_case(name)));
        ok.then_some(id)
    }

    /// Ordered fallback chain of font ids (not loaded until needed).
    fn chain_for(&mut self, st: &TextStyle) -> Vec<fontdb::ID> {
        let key = ChainKey { fam: st.families.join("\u{1}").to_ascii_lowercase(), w: (st.weight / 100 * 100).clamp(100, 900), italic: st.italic };
        if let Some(c) = self.chains.get(&key) { return c.clone(); }
        let w = key.w;
        let mut out: Vec<fontdb::ID> = vec![];
        for fam in &st.families {
            if let Some(cands) = family_candidates(fam) {
                for n in cands { if let Some(id) = self.find_family(n, w, st.italic) { push_unique(&mut out, id); break; } }
            } else if let Some(id) = self.find_family(fam, w, st.italic) {
                push_unique(&mut out, id);
            }
        }
        if out.is_empty() {
            for n in SANS { if let Some(id) = self.find_family(n, w, st.italic) { push_unique(&mut out, id); break; } }
        }
        for group in [BENGALI, OTHER] {
            let mut taken = 0;
            for n in group {
                if let Some(id) = self.find_family(n, 400, false) { if !out.contains(&id) { out.push(id); taken += 1; } }
                if taken >= 3 { break; }
            }
        }
        if out.is_empty() { out.extend(self.db.faces().take(8).map(|f| f.id)); } // last resort
        self.chains.insert(key, out.clone());
        out
    }

    pub fn has_fonts(&mut self) -> bool { !self.chain_for(&TextStyle::new(16.0)).is_empty() }

    fn covers(&self, face: usize, c: char) -> bool { self.face(face).map_or(false, |f| f.hb.glyph_index(c).is_some()) }

    /// First face (preferring `prefer`) that has a glyph for `c`; loads faces lazily, in chain order.
    fn face_for(&mut self, chain: &[fontdb::ID], c: char, prefer: Option<usize>) -> Option<usize> {
        if let Some(p) = prefer { if self.covers(p, c) { return Some(p); } }
        for &id in chain {
            if let Some(i) = self.load(id) { if self.covers(i, c) { return Some(i); } }
        }
        None
    }

    // ------------------------------------------------------------ shaping
    pub fn shape(&mut self, text: &str, size: f32, bold: bool) -> Shaped { self.shape_style(text, &TextStyle::new(size).bold(bold)) }

    pub fn shape_style(&mut self, text: &str, st: &TextStyle) -> Shaped {
        let cacheable = text.len() <= 512;
        let key = ShapeKey { text: if cacheable { text.to_string() } else { String::new() }, size_bits: st.size.to_bits(), w: st.weight, italic: st.italic, fam: st.families.join(","), ls: (st.letter_spacing * 100.0) as i32 };
        if cacheable { if let Some(s) = self.shaped.get(&key) { return s.clone(); } }
        let chain = self.chain_for(st);
        let mut out = Shaped { glyphs: vec![], width: 0.0, size: st.size, weight: st.weight, italic: st.italic };
        let Some(first) = chain.first().and_then(|&id| self.load(id)) else {
            out.width = text.chars().count() as f32 * st.size * 0.6;
            return out;
        };
        let mut pen = 0.0f32;
        for (range, rtl) in bidi_runs(text) {
            // logical sub-runs by font coverage (stay on the current face when possible)
            let mut subs: Vec<(usize, usize, usize)> = vec![];
            let mut cur: Option<usize> = None;
            for (bi, ch) in text[range.clone()].char_indices() {
                let abs = range.start + bi;
                let f = match cur {
                    Some(c) if joiner(ch) => c,
                    _ => self.face_for(&chain, ch, cur).or(cur).unwrap_or(first),
                };
                match subs.last_mut() {
                    Some(l) if l.0 == f => l.2 = abs + ch.len_utf8(),
                    _ => subs.push((f, abs, abs + ch.len_utf8())),
                }
                cur = Some(f);
            }
            if rtl { subs.reverse(); } // visual order
            for (fi, s0, e0) in subs {
                let Some(fd) = self.face(fi) else { continue };
                let sc = st.size / fd.upem;
                let mut buf = rustybuzz::UnicodeBuffer::new();
                buf.push_str(&text[s0..e0]);
                buf.guess_segment_properties();
                buf.set_direction(if rtl { rustybuzz::Direction::RightToLeft } else { rustybuzz::Direction::LeftToRight });
                let gb = rustybuzz::shape(&fd.hb, &[], buf);
                for (info, pos) in gb.glyph_infos().iter().zip(gb.glyph_positions()) {
                    let adv = pos.x_advance as f32 * sc + if rtl { 0.0 } else { st.letter_spacing };
                    out.glyphs.push(G { face: fi, id: info.glyph_id as u16, x: pen + pos.x_offset as f32 * sc, y: -(pos.y_offset as f32 * sc), cluster: s0 + info.cluster as usize, adv, rtl });
                    pen += adv;
                }
            }
        }
        out.width = pen;
        if cacheable {
            if self.shaped.len() > 4000 { self.shaped.clear(); }
            self.shaped.insert(key, out.clone());
        }
        out
    }

    pub fn measure(&mut self, text: &str, size: f32, bold: bool) -> f32 { self.shape(text, size, bold).width }
    pub fn measure_style(&mut self, text: &str, st: &TextStyle) -> f32 { self.shape_style(text, st).width }

    /// Caret stops `(byte index, x)` in logical order, always starting at 0 and ending at `text.len()`.
    /// Ligatures / Bangla conjuncts are one stop (the caret never lands inside a cluster).
    pub fn caret_stops(&mut self, text: &str, st: &TextStyle) -> Vec<(usize, f32)> {
        let sh = self.shape_style(text, st);
        let mut m: BTreeMap<usize, (f32, f32, bool)> = BTreeMap::new();
        for g in &sh.glyphs {
            let e = m.entry(g.cluster).or_insert((g.x, g.x + g.adv, g.rtl));
            e.0 = e.0.min(g.x);
            e.1 = e.1.max(g.x + g.adv);
        }
        let mut out: Vec<(usize, f32)> = m.iter().map(|(&c, &(l, r, rtl))| (c, if rtl { r } else { l })).collect();
        if out.first().map_or(true, |s| s.0 != 0) {
            out.insert(0, (0, if sh.glyphs.first().map_or(false, |g| g.rtl) { sh.width } else { 0.0 }));
        }
        let end_x = m.iter().next_back().map_or(0.0, |(_, &(l, r, rtl))| if rtl { l } else { r });
        out.push((text.len(), end_x));
        out
    }

    pub fn metrics_style(&mut self, st: &TextStyle) -> Metrics {
        let chain = self.chain_for(st);
        let fi = chain.first().and_then(|&id| self.load(id));
        match fi.and_then(|i| self.face(i)) {
            Some(f) => {
                let sc = st.size / f.upem;
                let ul = f.hb.underline_metrics();
                Metrics {
                    ascent: f.hb.ascender() as f32 * sc,
                    descent: -(f.hb.descender() as f32) * sc,
                    line_gap: f.hb.line_gap() as f32 * sc,
                    x_height: f.hb.x_height().map_or(st.size * 0.5, |v| v as f32 * sc),
                    underline_pos: ul.map_or(-st.size * 0.1, |l| l.position as f32 * sc),
                    underline_thick: ul.map_or(st.size / 14.0, |l| l.thickness as f32 * sc),
                }
            }
            None => Metrics { ascent: st.size * 0.8, descent: st.size * 0.2, line_gap: 0.0, x_height: st.size * 0.5, underline_pos: -st.size * 0.1, underline_thick: st.size / 14.0 },
        }
    }

    /// (ascent, descent) in px for the primary face.
    pub fn metrics(&mut self, size: f32, bold: bool) -> (f32, f32) {
        let m = self.metrics_style(&TextStyle::new(size).bold(bold));
        (m.ascent, m.descent)
    }

    // ------------------------------------------------------------ web fonts
    /// Register a TTF/OTF/TTC web font under a CSS family `alias`. WOFF/WOFF2 must be decompressed first.
    pub fn register_web_font(&mut self, alias: &str, data: Vec<u8>, weight: u16, italic: bool) -> bool {
        let magic = data.get(..4).unwrap_or(&[]);
        let ok = magic == &[0u8, 1, 0, 0][..] || magic == &b"OTTO"[..] || magic == &b"true"[..] || magic == &b"ttcf"[..];
        if !ok || self.web_bytes + data.len() > WEB_MAX_BYTES { return false; }
        let before: HashSet<fontdb::ID> = self.db.faces().map(|f| f.id).collect();
        let n = data.len();
        self.db.load_font_data(data);
        let new: Vec<fontdb::ID> = self.db.faces().map(|f| f.id).filter(|id| !before.contains(id)).collect();
        if new.is_empty() { return false; }
        self.web_bytes += n;
        let key = alias.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
        let e = self.web.entry(key).or_default();
        for id in new { e.push(WebFace { id, weight: weight.clamp(100, 900), italic }); }
        self.chains.clear();
        self.shaped.clear();
        true
    }

    /// Drop all web fonts (call on navigation): frees their RAM.
    pub fn clear_web_fonts(&mut self) {
        let ids: Vec<fontdb::ID> = self.web.values().flat_map(|v| v.iter().map(|w| w.id)).collect();
        for id in &ids {
            if let Some(i) = self.by_id.remove(id) { if let Some(slot) = self.faces.get_mut(i) { *slot = None; } }
            self.db.remove_face(*id);
        }
        self.web.clear();
        self.web_bytes = 0;
        self.chains.clear();
        self.shaped.clear();
        self.paths.clear();
        self.masks.clear();
        self.mask_bytes = 0;
    }

    // ------------------------------------------------------------ drawing
    fn path(&mut self, face: usize, gid: u16) -> Option<Path> {
        if let Some(p) = self.paths.get(&(face, gid)) { return p.clone(); }
        let p = self.face(face).and_then(|f| {
            let mut b = Pb(PathBuilder::new());
            f.hb.outline_glyph(rustybuzz::ttf_parser::GlyphId(gid), &mut b)?;
            b.0.finish()
        });
        if self.paths.len() > 20000 { self.paths.clear(); }
        self.paths.insert((face, gid), p.clone());
        p
    }

    fn mask(&mut self, key: MaskKey, sc: f32, sub: f32) -> Option<Arc<GlyphMask>> {
        if let Some(m) = self.masks.get(&key) { return m.clone(); }
        let m = self.path(key.face, key.gid).and_then(|p| raster_glyph(&p, sc, key.skew, sub)).map(Arc::new);
        let sz = m.as_ref().map_or(0, |m| m.a.len());
        if self.masks.len() >= GLYPH_CACHE_MAX || self.mask_bytes + sz > GLYPH_CACHE_BYTES { self.masks.clear(); self.mask_bytes = 0; }
        self.mask_bytes += sz;
        self.masks.insert(key, m.clone());
        m
    }

    /// Draw shaped text with its baseline at `baseline`, left edge at `x`.
    pub fn draw(&mut self, pm: &mut Pixmap, s: &Shaped, x: f32, baseline: f32, rgb: (u8, u8, u8), bold: bool) {
        let want_bold = bold || s.weight >= 600;
        for g in &s.glyphs {
            let (upem, weight, fitalic) = match self.face(g.face) { Some(f) => (f.upem, f.weight, f.italic), None => continue };
            let sc = s.size / upem;
            let skew = s.italic && !fitalic; // synthetic italic
            let passes = if want_bold && weight < 600 { 2 } else { 1 }; // synthetic bold
            for k in 0..passes {
                let dx = if k == 1 { (s.size * 0.035).max(0.5) } else { 0.0 };
                let (gx, gy) = (x + g.x + dx, baseline + g.y);
                if s.size > RASTER_MAX_SIZE {
                    if let Some(path) = self.path(g.face, g.id) {
                        let mut paint = Paint::default();
                        paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
                        paint.anti_alias = true;
                        let kx = if skew { 0.2 * sc } else { 0.0 };
                        pm.fill_path(&path, &paint, FillRule::Winding, Transform::from_row(sc, 0.0, kx, -sc, gx, gy), None);
                    }
                    continue;
                }
                let ix = gx.floor();
                let sub = (((gx - ix) * SUBPIXEL as f32) as u8).min(SUBPIXEL - 1);
                let key = MaskKey { face: g.face, gid: g.id, size_bits: s.size.to_bits(), sub, skew };
                if let Some(m) = self.mask(key, sc, sub as f32 / SUBPIXEL as f32) { blit(pm, &m, ix as i32, gy.round() as i32, rgb); }
            }
        }
    }

    pub fn draw_decoration(&mut self, pm: &mut Pixmap, x: f32, baseline: f32, width: f32, st: &TextStyle, rgb: (u8, u8, u8), underline: bool, strike: bool) {
        let m = self.metrics_style(st);
        let mut paint = Paint::default();
        paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
        let th = m.underline_thick.max(1.0).round();
        if underline { if let Some(r) = Rect::from_xywh(x, (baseline - m.underline_pos).round(), width, th) { pm.fill_rect(r, &paint, Transform::identity(), None); } }
        if strike { if let Some(r) = Rect::from_xywh(x, (baseline - m.x_height * 0.5).round(), width, th) { pm.fill_rect(r, &paint, Transform::identity(), None); } }
    }
}

// ---------------------------------------------------------------- thread-local convenience API
thread_local! { static FONTS: RefCell<Option<Fonts>> = RefCell::new(None); }

pub fn with<R>(f: impl FnOnce(&mut Fonts) -> R) -> R {
    FONTS.with(|c| {
        let mut b = c.borrow_mut();
        f(b.get_or_insert_with(Fonts::new))
    })
}

/// True when at least one usable system font exists (else callers use the bitmap font).
pub fn available() -> bool { with(|f| f.has_fonts()) }
pub fn measure(text: &str, size: f32, bold: bool) -> f32 { with(|f| f.measure(text, size, bold)) }
pub fn measure_style(text: &str, st: &TextStyle) -> f32 { with(|f| f.measure_style(text, st)) }
pub fn caret_stops(text: &str, st: &TextStyle) -> Vec<(usize, f32)> { with(|f| f.caret_stops(text, st)) }
pub fn register_web_font(alias: &str, data: Vec<u8>, weight: u16, italic: bool) -> bool { with(|f| f.register_web_font(alias, data, weight, italic)) }
pub fn clear_web_fonts() { with(|f| f.clear_web_fonts()) }

/// Draw `text` vertically centred in a box of height `box_h` whose top is `y`. Returns false if no fonts.
pub fn draw_text(pm: &mut Pixmap, text: &str, x: f32, y: f32, box_h: f32, size: f32, bold: bool, rgb: (u8, u8, u8)) -> bool {
    draw_text_style(pm, text, x, y, box_h, &TextStyle::new(size).bold(bold), rgb)
}
/// Same with a full style (family list, weight, italic, letter-spacing).
pub fn draw_text_style(pm: &mut Pixmap, text: &str, x: f32, y: f32, box_h: f32, st: &TextStyle, rgb: (u8, u8, u8)) -> bool {
    with(|f| {
        if !f.has_fonts() { return false; }
        let m = f.metrics_style(st);
        let base = y + (box_h - (m.ascent + m.descent)) / 2.0 + m.ascent;
        let sh = f.shape_style(text, st);
        f.draw(pm, &sh, x, base, rgb, false);
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect_path() -> Path { let mut b = PathBuilder::new(); b.move_to(0.0, 0.0); b.line_to(10.0, 0.0); b.line_to(10.0, 10.0); b.line_to(0.0, 10.0); b.close(); b.finish().unwrap() }

    #[test] fn css_family_parsing() {
        assert_eq!(parse_families("Roboto, 'Helvetica Neue', \"Segoe UI\", sans-serif"), vec!["Roboto", "Helvetica Neue", "Segoe UI", "sans-serif"]);
        assert!(parse_families("  ,, ").is_empty());
        assert_eq!(parse_families("'A, B', C"), vec!["A, B", "C"]);
    }
    #[test] fn style_builder_clamps() {
        let s = TextStyle::new(f32::NAN).weight(5000).spacing(f32::INFINITY);
        assert!(s.size >= 1.0 && s.weight == 900 && s.letter_spacing == 0.0);
    }
    #[test] fn bidi_fast_path_and_rtl() {
        assert_eq!(bidi_runs("hello world"), vec![(0..11, false)]);
        let runs = bidi_runs("abc \u{05E9}\u{05DC}\u{05D5}\u{05DD}");
        assert!(runs.iter().any(|r| r.1) && runs.iter().any(|r| !r.1));
        assert_eq!(bidi_runs("").len(), 1);
    }
    #[test] fn line_break_opportunities() {
        let b = line_breaks("ab cd");
        assert!(b.iter().any(|&(i, _)| i == 3));
        assert!(line_breaks("a\nb").iter().any(|&(_, forced)| forced));
    }
    #[test] fn mask_raster_and_blit() {
        let m = raster_glyph(&rect_path(), 1.0, false, 0.0).expect("mask");
        let total: u32 = m.a.iter().map(|&a| a as u32).sum();
        assert!(total > 90 * 255 && total < 110 * 255, "{total}");
        let mut pm = Pixmap::new(30, 30).unwrap();
        pm.fill(tiny_skia::Color::WHITE);
        blit(&mut pm, &m, 10, 20, (0, 0, 0));
        let px = |x: usize, y: usize| pm.data()[(y * 30 + x) * 4];
        assert!(px(15, 15) < 10, "inside glyph is dark");
        assert_eq!(px(2, 2), 255, "outside untouched");
        blit(&mut pm, &m, -50, -50, (0, 0, 0)); // fully clipped: no panic
        blit(&mut pm, &m, 25, 25, (0, 0, 0));   // partially clipped
    }
    #[test] fn synthetic_italic_shifts_top() {
        let a = raster_glyph(&rect_path(), 4.0, false, 0.0).unwrap();
        let b = raster_glyph(&rect_path(), 4.0, true, 0.0).unwrap();
        assert!(b.w > a.w, "skew widens bounding box");
    }
    #[test] fn index_at_x_picks_nearest() {
        let st = [(0usize, 0.0f32), (1, 10.0), (2, 20.0)];
        assert_eq!(index_at_x(&st, 4.0), 0);
        assert_eq!(index_at_x(&st, 16.0), 2);
        assert_eq!(index_at_x(&[], 5.0), 0);
    }
    #[test] fn web_font_rejects_garbage_and_woff() {
        let mut f = Fonts::new();
        assert!(!f.register_web_font("x", vec![1, 2, 3, 4, 5], 400, false));
        assert!(!f.register_web_font("x", b"wOF2....".to_vec(), 400, false));
        assert!(!f.register_web_font("x", vec![], 400, false));
    }
    #[test] fn latin_and_bangla_shape() {
        let mut f = Fonts::new();
        if !f.has_fonts() { eprintln!("no fonts on this machine, skipping"); return; }
        let w1 = f.measure("Hello", 20.0, false);
        let w2 = f.measure("Hello Hello", 20.0, false);
        assert!(w1 > 10.0 && w2 > w1 * 1.8, "{w1} {w2}");
        let sh = f.shape("বাংলা ভাষা", 24.0, false);
        assert!(sh.width > 20.0, "bangla width {}", sh.width);
        assert!(sh.glyphs.iter().all(|g| g.id != 0), "bangla glyph missing (.notdef)");
    }
    #[test] fn lazy_loading_latin_loads_few_faces() {
        let mut f = Fonts::new();
        if !f.has_fonts() { return; }
        let _ = f.shape("Only latin text", 16.0, false);
        assert!(f.loaded_faces() <= 2, "loaded {}", f.loaded_faces());
    }
    #[test] fn caret_stops_are_monotonic_and_cluster_aware() {
        let mut f = Fonts::new();
        if !f.has_fonts() { return; }
        let st = TextStyle::new(20.0);
        let s = f.caret_stops("abc", &st);
        assert_eq!(s.first().unwrap().0, 0);
        assert_eq!(s.last().unwrap().0, 3);
        assert!(s.windows(2).all(|w| w[0].1 <= w[1].1 + 0.01));
        if f.shape("\u{0995}\u{09CD}\u{09B7}", 20.0, false).glyphs.iter().any(|g| g.id == 0) { return; } // no Bengali font here
        let bn = f.caret_stops("\u{0995}\u{09CD}\u{09B7}", &st); // conjunct ksha = one cluster
        assert!(bn.len() <= 3, "caret must not split the conjunct: {bn:?}");
        assert_eq!(bn.last().unwrap().0, "\u{0995}\u{09CD}\u{09B7}".len());
    }
    #[test] fn rtl_shaping_does_not_panic() {
        let mut f = Fonts::new();
        if !f.has_fonts() { return; }
        let sh = f.shape("abc \u{05E9}\u{05DC}\u{05D5}\u{05DD} def", 20.0, false);
        assert!(sh.width >= 0.0 && !sh.glyphs.is_empty());
    }
    #[test] fn styles_change_width_and_cache_hits() {
        let mut f = Fonts::new();
        if !f.has_fonts() { return; }
        let a = f.measure_style("Spacing", &TextStyle::new(20.0));
        let b = f.measure_style("Spacing", &TextStyle::new(20.0).spacing(3.0));
        assert!(b > a + 10.0, "{a} {b}");
        assert_eq!(f.measure_style("Spacing", &TextStyle::new(20.0)), a);
    }
    #[test] fn shape_cache_does_not_alias_nearby_font_sizes() {
        let mut f = Fonts::new();
        if !f.has_fonts() { return; }
        let first = f.shape("cache size", 16.0, false);
        let second = f.shape("cache size", 16.1, false);
        assert_eq!(first.size, 16.0);
        assert_eq!(second.size, 16.1);
    }
    #[test] fn renders_pixels() {
        let mut f = Fonts::new();
        if !f.has_fonts() { return; }
        let mut pm = Pixmap::new(420, 90).expect("pixmap");
        pm.fill(tiny_skia::Color::WHITE);
        let s = f.shape("Sparrow বাংলা ভাষা fi", 32.0, false);
        f.draw(&mut pm, &s, 10.0, 40.0, (0, 0, 0), false);
        let b = f.shape_style("Bold italic বাংলা", &TextStyle::new(28.0).bold(true).italic(true));
        f.draw(&mut pm, &b, 10.0, 80.0, (200, 0, 0), false);
        f.draw_decoration(&mut pm, 10.0, 80.0, b.width, &TextStyle::new(28.0), (200, 0, 0), true, false);
        let dark = pm.data().chunks(4).filter(|p| p[0] < 128).count();
        assert!(dark > 300, "dark pixels {dark}");
        let _ = pm.save_png("/tmp/font_test.png");
    }
}
