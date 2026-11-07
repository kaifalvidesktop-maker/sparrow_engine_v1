//! Sparrow layout: DOM + computed styles -> positioned paint items (via taffy).
//!
//! Public API (unchanged): `Paint`, `Item`, `Layout`, `compute`.

use crate::dro_css::{Color, Display as CssDisplay, Style};
use crate::dro_dom::{Dom, NodeId, NodeKind};
use crate::dro_paint::advance;
use std::collections::{HashMap, HashSet};
use taffy::prelude::*;

pub enum Paint {
    Box(Color),
    Word { text: String, color: Color, size: f32, bold: bool, link: Option<String> },
}
pub struct Item { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub paint: Paint }
pub struct Layout { pub items: Vec<Item>, pub height: f32, pub bg: Color, pub anchors: HashMap<String, f32> }

/// Line height as a multiple of the font size.
const LINE: f32 = 1.3;
/// Maximum nesting depth we are willing to lay out (stack safety).
const MAX_DEPTH: usize = 200;

type R<T> = Result<T, String>;
fn err<T: std::fmt::Debug>(x: T) -> String { format!("{x:?}") }

// ---------------------------------------------------------------------------
// Text measurement helpers
// ---------------------------------------------------------------------------

/// HTML whitespace only. NBSP (U+00A0) and friends are NOT collapsible and
/// must not be treated as a break opportunity.
fn is_ws(c: char) -> bool { matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{0c}') }

pub(crate) fn is_virama(c: char) -> bool {
    matches!(
        c,
        '\u{094D}' | '\u{09CD}' | '\u{0A4D}' | '\u{0ACD}' | '\u{0B4D}' | '\u{0BCD}' | '\u{0C4D}' | '\u{0CCD}' | '\u{0D4D}'
    )
}

/// Approximate width of one char in "cells" (1 cell = one `advance(size)`).
/// Combining marks take no space, a consonant joined by a virama (Bengali
/// conjuncts such as ক্ষ) takes about half a cell, wide (CJK/emoji) takes two.
/// This is a heuristic until a real shaper (parley / cosmic-text) is plugged in.
pub(crate) fn cells(c: char, after_virama: bool) -> f32 {
    let u = c as u32;
    let zero = is_virama(c)
        || matches!(
            u,
            0x0300..=0x036F
                | 0x200B..=0x200F
                | 0x2060
                | 0xFE00..=0xFE0F
                | 0x0981
                | 0x09BC
                | 0x09C1..=0x09C4
                | 0x09E2
                | 0x09E3
        );
    if zero {
        0.0
    } else if after_virama {
        0.5
    } else if matches!(
        u,
        0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD
    ) {
        2.0
    } else {
        1.0
    }
}

fn word_cells(w: &str) -> f32 {
    let (mut total, mut prev_v) = (0.0, false);
    for ch in w.chars() {
        total += cells(ch, prev_v);
        prev_v = is_virama(ch);
    }
    total
}

/// Split a word that is wider than the line into pieces that each fit.
/// Never splits between a base char and its combining marks / after a virama.
fn split_word(w: &str, adv: f32, max_w: f32) -> Vec<String> {
    let limit = (max_w / adv.max(0.01)).max(1.0);
    let (mut out, mut cur) = (Vec::new(), String::new());
    let (mut used, mut prev_v) = (0.0f32, false);
    for ch in w.chars() {
        let c = cells(ch, prev_v);
        if c > 0.0 && !prev_v && !cur.is_empty() && used + c > limit {
            out.push(std::mem::take(&mut cur));
            used = 0.0;
        }
        cur.push(ch);
        used += c;
        prev_v = is_virama(ch);
    }
    if !cur.is_empty() { out.push(cur); }
    if out.is_empty() { out.push(w.to_string()); }
    out
}

fn gap_of(st: &taffy::Style) -> f32 {
    match st.margin.right { LengthPercentageAuto::Length(v) => v, _ => 0.0 }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

struct B<'a> {
    dom: &'a Dom,
    st: &'a [Option<Style>],
    tree: TaffyTree<()>,
    paints: HashMap<taffy::NodeId, Paint>,
    ids: HashMap<taffy::NodeId, String>,
    anchors: HashMap<String, f32>,
    /// `<br>` leaves.
    brs: HashSet<taffy::NodeId>,
    /// Zero-size anchor markers (inline elements with an id).
    marks: HashSet<taffy::NodeId>,
    /// Vertical margins (top, bottom) of block boxes, for sibling margin collapsing.
    vm: HashMap<taffy::NodeId, (f32, f32)>,
    /// Memoised nearest-`<a href>` lookup.
    links: HashMap<NodeId, Option<String>>,
}

impl<'a> B<'a> {
    /// Nearest enclosing `<a href>` (memoised, iterative).
    fn link(&mut self, start: NodeId) -> Option<String> {
        let dom = self.dom;
        let mut path = Vec::new();
        let mut n = start;
        let res = loop {
            if let Some(v) = self.links.get(&n) { break v.clone(); }
            path.push(n);
            if dom.tag(n) == Some("a") { break dom.attr(n, "href").map(|s| s.to_string()); }
            match dom.nodes[n].parent { Some(p) => n = p, None => break None }
        };
        for p in path { self.links.insert(p, res.clone()); }
        res
    }

    /// `id`, or `name` on `<a>` (legacy fragment targets).
    fn anchor_id(&self, n: NodeId) -> Option<String> {
        let d = self.dom;
        let id = d.attr(n, "id").map(|v| v.to_string()).filter(|v| !v.is_empty());
        if id.is_some() { return id; }
        if d.tag(n) == Some("a") {
            return d.attr(n, "name").map(|v| v.to_string()).filter(|v| !v.is_empty());
        }
        None
    }

    /// Bullet or number for a list item.
    fn list_marker(&self, n: NodeId) -> String {
        let d = self.dom;
        if let Some(p) = d.nodes[n].parent {
            if d.tag(p) == Some("ol") {
                let start: i64 = d.attr(p, "start").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(1);
                let mut idx = 0i64;
                for &c in d.nodes[p].children.iter() {
                    if d.tag(c) == Some("li") {
                        if c == n { break; }
                        idx += 1;
                    }
                }
                return format!("{}. ", start + idx);
            }
        }
        "* ".to_string()
    }

    fn set_last_gap(&mut self, run: &[taffy::NodeId], f: impl Fn(f32) -> Option<f32>) -> R<()> {
        if let Some(&p) = run.last() {
            if self.paints.contains_key(&p) {
                let mut st = self.tree.style(p).map_err(err)?.clone();
                if let Some(g) = f(gap_of(&st)) {
                    st.margin.right = LengthPercentageAuto::Length(g);
                    self.tree.set_style(p, st).map_err(err)?;
                }
            }
        }
        Ok(())
    }

    /// Leading whitespace in a text node: make sure the previous word has a gap.
    fn widen_last_gap(&mut self, run: &[taffy::NodeId], adv: f32) -> R<()> {
        self.set_last_gap(run, |cur| if cur < adv { Some(adv) } else { None })
    }

    /// A line must not end with a collapsible space.
    fn trim_last_gap(&mut self, run: &[taffy::NodeId]) -> R<()> {
        self.set_last_gap(run, |cur| if cur > 0.0 { Some(0.0) } else { None })
    }

    fn line_is_empty(&self, run: &[taffy::NodeId]) -> bool {
        for n in run.iter().rev() {
            if self.marks.contains(n) { continue; }
            return self.brs.contains(n);
        }
        true
    }

    fn words(&mut self, parent: NodeId, text: &str, run: &mut Vec<taffy::NodeId>, avail: f32) -> R<()> {
        let st_all = self.st;
        let Some(Some(s)) = st_all.get(parent) else { return Ok(()) };
        let (size, color, bold) = (s.size, s.color, s.bold);
        let adv = advance(size);
        let link = self.link(parent);

        if text.starts_with(is_ws) { self.widen_last_gap(run, adv)?; }

        let ws: Vec<&str> = text.split(is_ws).filter(|w| !w.is_empty()).collect();
        let trail = text.ends_with(is_ws);
        let count = ws.len();
        for (i, w) in ws.iter().enumerate() {
            let gap = if i + 1 < count || trail { adv } else { 0.0 };
            let pieces: Vec<String> = if avail > 0.0 && word_cells(w) * adv > avail {
                split_word(w, adv, avail)
            } else {
                vec![w.to_string()]
            };
            let last = pieces.len() - 1;
            for (j, piece) in pieces.into_iter().enumerate() {
                let width = word_cells(&piece) * adv;
                let g = if j == last { gap } else { 0.0 };
                let style = taffy::Style {
                    size: Size { width: Dimension::Length(width), height: Dimension::Length(size * LINE) },
                    margin: Rect {
                        left: LengthPercentageAuto::Length(0.0),
                        right: LengthPercentageAuto::Length(g),
                        top: LengthPercentageAuto::Length(0.0),
                        bottom: LengthPercentageAuto::Length(0.0),
                    },
                    flex_shrink: 0.0,
                    ..Default::default()
                };
                let n = self.tree.new_leaf(style).map_err(err)?;
                self.paints.insert(n, Paint::Word { text: piece, color, size, bold, link: link.clone() });
                run.push(n);
            }
        }
        Ok(())
    }

    /// Zero-size marker so `<span id>` / `<a name>` get a correct anchor position.
    fn mark(&mut self, c: NodeId, run: &mut Vec<taffy::NodeId>) -> R<()> {
        let Some(id) = self.anchor_id(c) else { return Ok(()) };
        let style = taffy::Style {
            size: Size { width: Dimension::Length(0.0), height: Dimension::Length(0.0) },
            align_self: Some(AlignSelf::FlexStart),
            flex_shrink: 0.0,
            ..Default::default()
        };
        let n = self.tree.new_leaf(style).map_err(err)?;
        self.marks.insert(n);
        self.ids.insert(n, id);
        run.push(n);
        Ok(())
    }

    fn flush(&mut self, run: &mut Vec<taffy::NodeId>, out: &mut Vec<taffy::NodeId>) -> R<()> {
        if run.is_empty() { return Ok(()); }
        self.trim_last_gap(run)?;
        let style = taffy::Style {
            display: taffy::Display::Flex,
            flex_wrap: FlexWrap::Wrap,
            align_items: Some(AlignItems::FlexEnd),
            ..Default::default()
        };
        out.push(self.tree.new_with_children(style, run).map_err(err)?);
        run.clear();
        Ok(())
    }

    fn inline(
        &mut self,
        n: NodeId,
        run: &mut Vec<taffy::NodeId>,
        out: &mut Vec<taffy::NodeId>,
        d: usize,
        avail: f32,
    ) -> R<()> {
        if d > MAX_DEPTH { return Ok(()); }
        let dom = self.dom;
        let st_all = self.st;
        for &c in dom.nodes[n].children.iter() {
            match &dom.nodes[c].kind {
                NodeKind::Text(t) => self.words(n, t, run, avail)?,
                NodeKind::Element { tag, .. } => {
                    let Some(Some(s)) = st_all.get(c) else { continue };
                    if s.display == CssDisplay::None { continue; }
                    if tag == "br" {
                        // A <br> ends the line. If the line is empty (start of block or
                        // right after another <br>) it must still occupy one line height.
                        self.trim_last_gap(run)?;
                        let h = if self.line_is_empty(run) { s.size * LINE } else { 0.0 };
                        let br = taffy::Style {
                            size: Size { width: Dimension::Percent(1.0), height: Dimension::Length(h) },
                            ..Default::default()
                        };
                        let leaf = self.tree.new_leaf(br).map_err(err)?;
                        self.brs.insert(leaf);
                        run.push(leaf);
                    } else if s.display == CssDisplay::Inline {
                        self.mark(c, run)?;
                        self.inline(c, run, out, d + 1, avail)?;
                    } else {
                        self.flush(run, out)?;
                        if let Some(b) = self.block(c, d + 1, avail)? { out.push(b); }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Adjacent sibling block margins collapse to max(a, b) instead of a + b.
    fn collapse(&mut self, kids: &[taffy::NodeId]) -> R<()> {
        let mut prev_bottom: Option<f32> = None;
        for &k in kids {
            match self.vm.get(&k).copied() {
                Some((top, bottom)) => {
                    if let Some(pb) = prev_bottom {
                        if pb > 0.0 && top > 0.0 {
                            let new_top = (top - pb).max(0.0);
                            if new_top < top {
                                let mut st = self.tree.style(k).map_err(err)?.clone();
                                st.margin.top = LengthPercentageAuto::Length(new_top);
                                self.tree.set_style(k, st).map_err(err)?;
                            }
                        }
                    }
                    prev_bottom = Some(bottom);
                }
                None => prev_bottom = None,
            }
        }
        Ok(())
    }

    fn block(&mut self, n: NodeId, d: usize, avail: f32) -> R<Option<taffy::NodeId>> {
        let Some(Some(s)) = self.st.get(n) else { return Ok(None) };
        if s.display == CssDisplay::None || d > MAX_DEPTH { return Ok(None); }
        let s = s.clone();

        let (mt, mr, mb, ml) = (s.margin[0], s.margin[1], s.margin[2], s.margin[3]);
        let (pt, pr, pb, pl) = (s.pad[0], s.pad[1], s.pad[2], s.pad[3]);
        // Width available to this box's inline content (border-box sizing).
        let outer = s.width.unwrap_or((avail - ml - mr).max(1.0));
        let inner = (outer - pl - pr).max(1.0);

        let (mut kids, mut run) = (vec![], vec![]);
        if s.display == CssDisplay::Block && self.dom.tag(n) == Some("li") {
            let m = self.list_marker(n);
            self.words(n, &m, &mut run, inner)?;
        }
        self.inline(n, &mut run, &mut kids, d, inner)?;
        self.flush(&mut run, &mut kids)?;
        self.collapse(&kids)?;

        let l = |v: f32| LengthPercentageAuto::Length(v);
        let p = |v: f32| LengthPercentage::Length(v);
        let style = taffy::Style {
            display: taffy::Display::Block,
            size: Size { width: s.width.map(Dimension::Length).unwrap_or(Dimension::Auto), height: Dimension::Auto },
            margin: Rect { top: l(mt), right: l(mr), bottom: l(mb), left: l(ml) },
            padding: Rect { top: p(pt), right: p(pr), bottom: p(pb), left: p(pl) },
            ..Default::default()
        };
        let node = self.tree.new_with_children(style, &kids).map_err(err)?;
        self.vm.insert(node, (mt, mb));
        if let Some(bg) = s.bg { self.paints.insert(node, Paint::Box(bg)); }
        if let Some(id) = self.anchor_id(n) { self.ids.insert(node, id); }
        Ok(Some(node))
    }

    fn collect(&mut self, n: taffy::NodeId, ox: f32, oy: f32, out: &mut Vec<Item>) -> R<()> {
        let l = *self.tree.layout(n).map_err(err)?;
        let (x, y) = (ox + l.location.x, oy + l.location.y);
        if let Some(id) = self.ids.get(&n).cloned() { self.anchors.entry(id).or_insert(y); }
        if let Some(p) = self.paints.remove(&n) {
            out.push(Item { x, y, w: l.size.width, h: l.size.height, paint: p });
        }
        for c in self.tree.children(n).map_err(err)? { self.collect(c, x, y, out)?; }
        Ok(())
    }
}

pub fn compute(dom: &Dom, st: &[Option<Style>], width: f32) -> Result<Layout, String> {
    let empty = || Layout { items: vec![], height: 0.0, bg: Color(255, 255, 255), anchors: HashMap::new() };
    let width = width.max(1.0); // also guards against NaN

    let mut b = B {
        dom,
        st,
        tree: TaffyTree::new(),
        paints: HashMap::new(),
        ids: HashMap::new(),
        anchors: HashMap::new(),
        brs: HashSet::new(),
        marks: HashSet::new(),
        vm: HashMap::new(),
        links: HashMap::new(),
    };
    // Fragments without <html> fall back to <body>.
    let Some(root_dom) = dom.find_tag("html").or_else(|| dom.find_tag("body")) else { return Ok(empty()) };
    let Some(root) = b.block(root_dom, 0, width)? else { return Ok(empty()) };

    let mut rs = b.tree.style(root).map_err(err)?.clone();
    rs.size.width = Dimension::Length(width);
    b.tree.set_style(root, rs).map_err(err)?;
    b.tree
        .compute_layout(root, Size { width: AvailableSpace::Definite(width), height: AvailableSpace::MaxContent })
        .map_err(err)?;
    let height = b.tree.layout(root).map_err(err)?.size.height;

    // CSS canvas background: <html>'s wins; if it has none, <body>'s propagates.
    let bg_of = |n: NodeId| st.get(n).and_then(|s| s.as_ref()).and_then(|s| s.bg);
    let bg = bg_of(root_dom)
        .or_else(|| dom.find_tag("body").and_then(|bd| bg_of(bd)))
        .unwrap_or(Color(255, 255, 255));

    let mut items = vec![];
    b.collect(root, 0.0, 0.0, &mut items)?;
    Ok(Layout { items, height, bg, anchors: std::mem::take(&mut b.anchors) })
}
