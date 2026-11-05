use crate::dro_css::{Color, Display as CssDisplay, Style};
use crate::dro_dom::{Dom, NodeId, NodeKind};
use crate::dro_paint::advance;
use std::collections::HashMap;
use taffy::prelude::*;

pub enum Paint {
    Box(Color),
    Word { text: String, color: Color, size: f32, bold: bool, link: Option<String> },
}
pub struct Item { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub paint: Paint }
pub struct Layout { pub items: Vec<Item>, pub height: f32, pub bg: Color, pub anchors: HashMap<String, f32> }

struct B<'a> {
    dom: &'a Dom,
    st: &'a [Option<Style>],
    tree: TaffyTree<()>,
    paints: HashMap<taffy::NodeId, Paint>,
    ids: HashMap<taffy::NodeId, String>,
    anchors: HashMap<String, f32>,
}

type R<T> = Result<T, String>;
fn e<T: std::fmt::Debug>(x: T) -> String { format!("{x:?}") }

impl<'a> B<'a> {
    fn link(&self, mut n: NodeId) -> Option<String> {
        loop {
            if self.dom.tag(n) == Some("a") { return self.dom.attr(n, "href").map(|s| s.to_string()); }
            n = self.dom.nodes[n].parent?;
        }
    }
    fn words(&mut self, parent: NodeId, text: &str, run: &mut Vec<taffy::NodeId>) -> R<()> {
        let Some(Some(s)) = self.st.get(parent) else { return Ok(()) };
        let (size, color, bold) = (s.size, s.color, s.bold);
        let adv = advance(size);
        let link = self.link(parent);
        if text.starts_with(char::is_whitespace) {
            if let Some(&p) = run.last() {
                if self.paints.contains_key(&p) {
                    let mut st = self.tree.style(p).map_err(e)?.clone();
                    st.margin.right = LengthPercentageAuto::Length(adv);
                    self.tree.set_style(p, st).map_err(e)?;
                }
            }
        }
        let ws: Vec<&str> = text.split_whitespace().collect();
        let trail = text.ends_with(char::is_whitespace);
        for (i, w) in ws.iter().enumerate() {
            let gap = if i + 1 < ws.len() || trail { adv } else { 0.0 };
            let width = w.chars().count() as f32 * adv;
            let style = taffy::Style {
                size: Size { width: Dimension::Length(width), height: Dimension::Length(size * 1.3) },
                margin: Rect { left: LengthPercentageAuto::Length(0.0), right: LengthPercentageAuto::Length(gap), top: LengthPercentageAuto::Length(0.0), bottom: LengthPercentageAuto::Length(0.0) },
                flex_shrink: 0.0,
                ..Default::default()
            };
            let n = self.tree.new_leaf(style).map_err(e)?;
            self.paints.insert(n, Paint::Word { text: w.to_string(), color, size, bold, link: link.clone() });
            run.push(n);
        }
        Ok(())
    }
    fn flush(&mut self, run: &mut Vec<taffy::NodeId>, out: &mut Vec<taffy::NodeId>) -> R<()> {
        if run.is_empty() { return Ok(()); }
        let style = taffy::Style { display: taffy::Display::Flex, flex_wrap: FlexWrap::Wrap, align_items: Some(AlignItems::FlexEnd), ..Default::default() };
        out.push(self.tree.new_with_children(style, run).map_err(e)?);
        run.clear();
        Ok(())
    }
    fn inline(&mut self, n: NodeId, run: &mut Vec<taffy::NodeId>, out: &mut Vec<taffy::NodeId>, d: usize) -> R<()> {
        if d > 200 { return Ok(()); }
        for c in self.dom.nodes[n].children.clone() {
            match &self.dom.nodes[c].kind {
                NodeKind::Text(t) => { let t = t.clone(); self.words(n, &t, run)?; }
                NodeKind::Element { tag, .. } => {
                    let Some(Some(s)) = self.st.get(c) else { continue };
                    if s.display == CssDisplay::None { continue; }
                    if tag == "br" {
                        let br = taffy::Style { size: Size { width: Dimension::Percent(1.0), height: Dimension::Length(0.0) }, ..Default::default() };
                        run.push(self.tree.new_leaf(br).map_err(e)?);
                    } else if s.display == CssDisplay::Inline {
                        self.inline(c, run, out, d + 1)?;
                    } else {
                        self.flush(run, out)?;
                        if let Some(b) = self.block(c, d + 1)? { out.push(b); }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    fn block(&mut self, n: NodeId, d: usize) -> R<Option<taffy::NodeId>> {
        let Some(Some(s)) = self.st.get(n) else { return Ok(None) };
        if s.display == CssDisplay::None || d > 200 { return Ok(None); }
        let s = s.clone();
        let (mut kids, mut run) = (vec![], vec![]);
        if s.display == CssDisplay::Block && self.dom.tag(n) == Some("li") {
            self.words(n, "* ", &mut run)?;
        }
        self.inline(n, &mut run, &mut kids, d)?;
        self.flush(&mut run, &mut kids)?;
        let l = |v: f32| LengthPercentageAuto::Length(v);
        let p = |v: f32| LengthPercentage::Length(v);
        let style = taffy::Style {
            display: taffy::Display::Block,
            size: Size { width: s.width.map(Dimension::Length).unwrap_or(Dimension::Auto), height: Dimension::Auto },
            margin: Rect { top: l(s.margin[0]), right: l(s.margin[1]), bottom: l(s.margin[2]), left: l(s.margin[3]) },
            padding: Rect { top: p(s.pad[0]), right: p(s.pad[1]), bottom: p(s.pad[2]), left: p(s.pad[3]) },
            ..Default::default()
        };
        let node = self.tree.new_with_children(style, &kids).map_err(e)?;
        if let Some(bg) = s.bg { self.paints.insert(node, Paint::Box(bg)); }
        if let Some(id) = self.dom.attr(n, "id").map(|v| v.to_string()) { self.ids.insert(node, id); }
        Ok(Some(node))
    }
    fn collect(&mut self, n: taffy::NodeId, ox: f32, oy: f32, out: &mut Vec<Item>) -> R<()> {
        let l = *self.tree.layout(n).map_err(e)?;
        let (x, y) = (ox + l.location.x, oy + l.location.y);
        if let Some(id) = self.ids.get(&n).cloned() { self.anchors.entry(id).or_insert(y); }
        if let Some(p) = self.paints.remove(&n) {
            out.push(Item { x, y, w: l.size.width, h: l.size.height, paint: p });
        }
        for c in self.tree.children(n).map_err(e)? { self.collect(c, x, y, out)?; }
        Ok(())
    }
}

pub fn compute(dom: &Dom, st: &[Option<Style>], width: f32) -> Result<Layout, String> {
    let mut b = B { dom, st, tree: TaffyTree::new(), paints: HashMap::new(), ids: HashMap::new(), anchors: HashMap::new() };
    let Some(root_dom) = dom.find_tag("html") else { return Ok(Layout { items: vec![], height: 0.0, bg: Color(255, 255, 255), anchors: HashMap::new() }) };
    let Some(root) = b.block(root_dom, 0)? else { return Ok(Layout { items: vec![], height: 0.0, bg: Color(255, 255, 255), anchors: HashMap::new() }) };
    let mut rs = b.tree.style(root).map_err(e)?.clone();
    rs.size.width = Dimension::Length(width);
    b.tree.set_style(root, rs).map_err(e)?;
    b.tree.compute_layout(root, Size { width: AvailableSpace::Definite(width), height: AvailableSpace::MaxContent }).map_err(e)?;
    let height = b.tree.layout(root).map_err(e)?.size.height;
    let bg = [root_dom].into_iter().chain(dom.find_tag("body")).filter_map(|n| st[n].as_ref().and_then(|s| s.bg)).last().unwrap_or(Color(255, 255, 255));
    let mut items = vec![];
    b.collect(root, 0.0, 0.0, &mut items)?;
    Ok(Layout { items, height, bg, anchors: std::mem::take(&mut b.anchors) })
}
