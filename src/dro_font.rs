//! Real text: system font discovery (fontdb), OpenType shaping (rustybuzz = pure-Rust HarfBuzz),
//! outline rendering with tiny-skia (anti-aliased). Handles Bangla/Indic conjuncts, ligatures, kerning.
//! Per-character font fallback (Latin font -> Bengali font -> symbols/CJK/emoji).
//! NOT yet: bidi reordering of mixed LTR/RTL, vertical text, color emoji.
//! Font bytes are leaked on purpose (`&'static`): bounded by the number of distinct fonts used.
use std::cell::RefCell;
use std::collections::HashMap;
use tiny_skia::{FillRule, Paint, Path, PathBuilder, Pixmap, Transform};

struct FaceData { hb: rustybuzz::Face<'static>, upem: f32, weight: u16 }

#[derive(Clone, Copy)]
pub struct G { pub face: usize, pub id: u16, pub x: f32, pub y: f32 }

#[derive(Clone)]
pub struct Shaped { pub glyphs: Vec<G>, pub width: f32, pub size: f32 }

pub struct Fonts {
    db: fontdb::Database,
    faces: Vec<FaceData>,
    by_id: HashMap<fontdb::ID, usize>,
    chains: [Option<Vec<usize>>; 2],
    paths: HashMap<(usize, u16), Option<Path>>,
    shaped: HashMap<(String, u32, bool), Shaped>,
}

const SANS: &[&str] = &["Segoe UI", "Arial", "Noto Sans", "DejaVu Sans", "Liberation Sans", "FreeSans", "Helvetica"];
const BENGALI: &[&str] = &["Nirmala UI", "Vrinda", "Shonar Bangla", "Noto Sans Bengali", "Siyam Rupali", "Lohit Bengali", "FreeSans", "Noto Serif Bengali"];
const OTHER: &[&str] = &["Segoe UI Symbol", "Noto Sans Symbols", "Microsoft YaHei", "Noto Sans CJK SC", "Yu Gothic", "Malgun Gothic", "Segoe UI Emoji", "Arial Unicode MS", "Noto Sans Devanagari", "Nirmala UI", "DejaVu Sans", "Unifont"];

struct Pb(PathBuilder);
impl rustybuzz::ttf_parser::OutlineBuilder for Pb {
    fn move_to(&mut self, x: f32, y: f32) { self.0.move_to(x, y) }
    fn line_to(&mut self, x: f32, y: f32) { self.0.line_to(x, y) }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) { self.0.quad_to(x1, y1, x, y) }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) { self.0.cubic_to(x1, y1, x2, y2, x, y) }
    fn close(&mut self) { self.0.close() }
}

fn joiner(c: char) -> bool { matches!(c, '\u{200C}' | '\u{200D}' | '\u{FE00}'..='\u{FE0F}') }

impl Fonts {
    pub fn new() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Fonts { db, faces: vec![], by_id: HashMap::new(), chains: [None, None], paths: HashMap::new(), shaped: HashMap::new() }
    }

    /// Number of fonts found on this system.
    pub fn count(&self) -> usize { self.db.len() }

    fn load(&mut self, id: fontdb::ID) -> Option<usize> {
        if let Some(&i) = self.by_id.get(&id) { return Some(i); }
        let (data, index) = self.db.with_face_data(id, |d, i| (d.to_vec(), i))?;
        let leaked: &'static [u8] = Box::leak(data.into_boxed_slice());
        let hb = rustybuzz::Face::from_slice(leaked, index)?;
        let upem = hb.units_per_em() as f32;
        let weight = self.db.face(id).map(|f| f.weight.0).unwrap_or(400);
        self.faces.push(FaceData { hb, upem: upem.max(1.0), weight });
        self.by_id.insert(id, self.faces.len() - 1);
        Some(self.faces.len() - 1)
    }

    fn chain(&mut self, bold: bool) -> Vec<usize> {
        let k = bold as usize;
        if let Some(c) = &self.chains[k] { return c.clone(); }
        let weight = if bold { 700 } else { 400 };
        let mut out: Vec<usize> = vec![];
        // primary: first sans that exists; then Bengali; then others
        let mut groups: Vec<&[&str]> = vec![SANS, BENGALI, OTHER];
        for (gi, names) in groups.drain(..).enumerate() {
            let mut taken = 0;
            for n in names {
                // fallback scripts use the regular face + synthetic bold (bold Indic faces are often incomplete)
                let w = if gi == 0 { weight } else { 400 };
                let q = fontdb::Query { families: &[fontdb::Family::Name(n)], weight: fontdb::Weight(w), stretch: fontdb::Stretch::Normal, style: fontdb::Style::Normal };
                if let Some(id) = self.db.query(&q) {
                    // query() falls back to any face when the family is missing: require same family name
                    let ok = self.db.face(id).map_or(false, |f| f.families.iter().any(|(fam, _)| fam.eq_ignore_ascii_case(n)));
                    if !ok { continue; }
                    if let Some(i) = self.load(id) {
                        if !out.contains(&i) { out.push(i); taken += 1; }
                    }
                }
                if gi == 0 && taken >= 1 { break; } // one primary sans is enough
                if gi > 0 && taken >= 3 { break; }
            }
        }
        if out.is_empty() {
            // last resort: any loadable face
            let ids: Vec<fontdb::ID> = self.db.faces().take(8).map(|f| f.id).collect();
            for id in ids { if let Some(i) = self.load(id) { out.push(i); break; } }
        }
        self.chains[k] = Some(out.clone());
        out
    }

    pub fn has_fonts(&mut self) -> bool { !self.chain(false).is_empty() }

    fn covers(&self, face: usize, c: char) -> bool {
        self.faces.get(face).map_or(false, |f| f.hb.glyph_index(c).is_some())
    }

    pub fn shape(&mut self, text: &str, size: f32, bold: bool) -> Shaped {
        let key = (text.to_string(), (size * 4.0) as u32, bold);
        if let Some(s) = self.shaped.get(&key) { return s.clone(); }
        let chain = self.chain(bold);
        let mut out = Shaped { glyphs: vec![], width: 0.0, size };
        if chain.is_empty() {
            out.width = text.chars().count() as f32 * size * 0.6;
            return out;
        }
        // split into runs by font coverage (prefer staying on the current face)
        let mut runs: Vec<(usize, String)> = vec![];
        for ch in text.chars() {
            let cur = runs.last().map(|r| r.0);
            let f = match cur {
                Some(cf) if self.covers(cf, ch) || joiner(ch) => cf,
                _ => chain.iter().copied().find(|&i| self.covers(i, ch)).or(cur).unwrap_or(chain[0]),
            };
            match runs.last_mut() { Some(r) if r.0 == f => r.1.push(ch), _ => runs.push((f, ch.to_string())) }
        }
        let mut pen = 0.0f32;
        for (fi, s) in runs {
            let Some(fd) = self.faces.get(fi) else { continue };
            let sc = size / fd.upem;
            let mut buf = rustybuzz::UnicodeBuffer::new();
            buf.push_str(&s);
            buf.guess_segment_properties();
            let gb = rustybuzz::shape(&fd.hb, &[], buf);
            for (info, pos) in gb.glyph_infos().iter().zip(gb.glyph_positions()) {
                out.glyphs.push(G { face: fi, id: info.glyph_id as u16, x: pen + pos.x_offset as f32 * sc, y: -(pos.y_offset as f32 * sc) });
                pen += pos.x_advance as f32 * sc;
            }
        }
        out.width = pen;
        if self.shaped.len() > 4000 { self.shaped.clear(); }
        self.shaped.insert(key, out.clone());
        out
    }

    pub fn measure(&mut self, text: &str, size: f32, bold: bool) -> f32 { self.shape(text, size, bold).width }

    /// (ascent, descent) in px for the primary face.
    pub fn metrics(&mut self, size: f32, bold: bool) -> (f32, f32) {
        let chain = self.chain(bold);
        match chain.first().and_then(|&i| self.faces.get(i)) {
            Some(f) => (f.hb.ascender() as f32 * size / f.upem, -(f.hb.descender() as f32) * size / f.upem),
            None => (size * 0.8, size * 0.2),
        }
    }

    fn path(&mut self, face: usize, gid: u16) -> Option<Path> {
        if let Some(p) = self.paths.get(&(face, gid)) { return p.clone(); }
        let p = self.faces.get(face).and_then(|f| {
            let mut b = Pb(PathBuilder::new());
            f.hb.outline_glyph(rustybuzz::ttf_parser::GlyphId(gid), &mut b)?;
            b.0.finish()
        });
        if self.paths.len() > 20000 { self.paths.clear(); }
        self.paths.insert((face, gid), p.clone());
        p
    }

    /// Draw shaped text with its baseline at `baseline`, left edge at `x`.
    pub fn draw(&mut self, pm: &mut Pixmap, s: &Shaped, x: f32, baseline: f32, rgb: (u8, u8, u8), bold: bool) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
        paint.anti_alias = true;
        for g in &s.glyphs {
            let Some(path) = self.path(g.face, g.id) else { continue };
            let (upem, weight) = match self.faces.get(g.face) { Some(f) => (f.upem, f.weight), None => continue };
            let sc = s.size / upem;
            let offs = [0.0f32, (s.size * 0.035).max(0.5)];
            let n = if bold && weight < 600 { 2 } else { 1 };
            for dx in &offs[..n] {
                let t = Transform::from_row(sc, 0.0, 0.0, -sc, x + g.x + dx, baseline + g.y);
                pm.fill_path(&path, &paint, FillRule::Winding, t, None);
            }
        }
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

/// Draw `text` vertically centred in a box of height `box_h` whose top is `y`. Returns false if no fonts.
pub fn draw_text(pm: &mut Pixmap, text: &str, x: f32, y: f32, box_h: f32, size: f32, bold: bool, rgb: (u8, u8, u8)) -> bool {
    with(|f| {
        if !f.has_fonts() { return false; }
        let (asc, desc) = f.metrics(size, bold);
        let base = y + (box_h - (asc + desc)) / 2.0 + asc;
        let sh = f.shape(text, size, bold);
        f.draw(pm, &sh, x, base, rgb, bold);
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn latin_and_bangla_shape() {
        let mut f = Fonts::new();
        if !f.has_fonts() { eprintln!("no fonts on this machine, skipping"); return; }
        let w1 = f.measure("Hello", 20.0, false);
        let w2 = f.measure("Hello Hello", 20.0, false);
        assert!(w1 > 10.0 && w2 > w1 * 1.8, "{w1} {w2}");
        let sh = f.shape("বাংলা ভাষা", 24.0, false);
        assert!(sh.width > 20.0, "bangla width {}", sh.width);
        assert!(sh.glyphs.iter().all(|g| g.id != 0), "bangla glyph missing (.notdef)");
    }
    #[test]
    fn renders_pixels() {
        let mut f = Fonts::new();
        if !f.has_fonts() { return; }
        let mut pm = Pixmap::new(420, 90).expect("pixmap");
        pm.fill(tiny_skia::Color::WHITE);
        let s = f.shape("Sparrow বাংলা ভাষা fi", 32.0, false);
        f.draw(&mut pm, &s, 10.0, 40.0, (0, 0, 0), false);
        let b = f.shape("Bold বাংলা", 28.0, true);
        f.draw(&mut pm, &b, 10.0, 80.0, (200, 0, 0), true);
        let dark = pm.data().chunks(4).filter(|p| p[0] < 128).count();
        assert!(dark > 300, "dark pixels {dark}");
        let _ = pm.save_png("/tmp/font_test.png");
    }
}
