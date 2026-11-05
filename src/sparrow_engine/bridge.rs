//! Child module of `sparrow_engine` (so it can touch private fields).
//! Adds: install_page (JS DOM + charset + css preprocessor), restyle, events, timers,
//! find-in-page, page text, DOM dump. Does not change existing public API.
use super::*;
use crate::{dro_charset, dro_css_ext, dro_dom_js};

enum Sc {
    Inline(String),
    Ext(usize),
}

fn title_of(dom: &Dom) -> String {
    dom.find_tag("title").map(|t| dom.text_content(t).trim().to_string()).unwrap_or_default()
}

impl SparrowEngine {
    /// Rebuild computed styles from UA + fetched <link> CSS + current <style> elements.
    /// Called after load, after viewport change (media queries/vw/vh) and after JS DOM mutation.
    pub(super) fn restyle(&mut self) {
        let (vw, vh) = (self.vp.0 as f32, self.vp.1 as f32);
        let mut rules = dro_css::parse(dro_css::UA);
        for src in &self.link_css {
            rules.extend(dro_css::parse(&dro_css_ext::preprocess(src, vw, vh)));
        }
        for n in self.dom.descendants(0) {
            if self.dom.tag(n) == Some("style") {
                rules.extend(dro_css::parse(&dro_css_ext::preprocess(&self.dom.text_content(n), vw, vh)));
            }
        }
        self.styles = dro_css::compute(&self.dom, &rules);
        self.layout = None;
    }

    /// Full page install. Replaces the old `install` (call sites: load_url, load_html).
    pub(super) fn install_page(&mut self, html: &str, base: &str) -> Result<(), String> {
        self.url = base.to_string();
        self.dom = dro_html::parse(html)?;
        self.scroll = 0.0;
        self.layout = None;
        self.images.clear();
        self.link_css.clear();
        self.pending_nav = None;
        self.title = title_of(&self.dom);

        let (mut css_urls, mut img_urls, mut js_urls): (Vec<String>, Vec<String>, Vec<String>) = (vec![], vec![], vec![]);
        let mut order: Vec<Sc> = vec![];
        for n in self.dom.descendants(0) {
            match self.dom.tag(n) {
                Some("link") if self.dom.attr(n, "rel").map(|r| r.to_lowercase().contains("stylesheet")).unwrap_or(false) => {
                    if let Some(u) = self.dom.attr(n, "href").and_then(|h| dro_net::resolve(base, h)) {
                        if css_urls.len() < 12 { css_urls.push(u); }
                    }
                }
                Some("img") => {
                    if let Some(u) = self.dom.attr(n, "src").and_then(|s| dro_net::resolve(base, s)) {
                        if u.starts_with("http") && !img_urls.contains(&u) && img_urls.len() < 40 { img_urls.push(u); }
                    }
                }
                Some("script") => match self.dom.attr(n, "src") {
                    None => order.push(Sc::Inline(self.dom.text_content(n))),
                    Some(s) => {
                        if let Some(u) = dro_net::resolve(base, s) {
                            if js_urls.len() < 8 { order.push(Sc::Ext(js_urls.len())); js_urls.push(u); }
                        }
                    }
                },
                _ => {}
            }
        }
        // network: CSS, images, external scripts (parallel)
        let css = dro_net::par_map(&css_urls, |r| Some(dro_charset::decode(&r.body, &r.content_type)));
        let imgs = dro_net::par_map(&img_urls, |r| dro_paint::decode_image(&r.body).ok());
        let js_src = dro_net::par_map(&js_urls, |r| Some(dro_charset::decode(&r.body, &r.content_type)));
        self.link_css = css.into_iter().flatten().collect();
        for (u, p) in img_urls.into_iter().zip(imgs) {
            if let Some(p) = p { self.images.insert(u, p); }
        }
        self.restyle();

        // JS: hydrate DOM, run scripts in document order, lifecycle events
        let mut js = JsRuntime::new()?;
        dro_dom_js::install(&mut js, &self.dom, base, &self.title, self.vp.0, self.vp.1)?;
        for s in order {
            let code = match s {
                Sc::Inline(t) => t,
                Sc::Ext(i) => match js_src.get(i) { Some(Some(t)) => t.clone(), _ => continue },
            };
            if let Err(e) = js.eval(&code) {
                let _ = js.eval(&format!("__log({})", dro_dom_js::js_string(&e)));
            }
        }
        dro_dom_js::lifecycle(&mut js);
        self.js = Some(js);
        self.apply_js()
    }

    /// Pull navigation + DOM changes out of JS and rebuild styles if needed.
    pub(super) fn apply_js(&mut self) -> Result<(), String> {
        let Some(js) = self.js.as_mut() else { return Ok(()) };
        if let Some(n) = dro_dom_js::take_navigation(js) {
            self.pending_nav = dro_net::resolve(&self.url, &n).or(Some(n));
        }
        if let Some(html) = dro_dom_js::serialize_if_dirty(js)? {
            self.dom = dro_html::parse(&html)?;
            self.title = title_of(&self.dom);
            self.restyle();
        }
        Ok(())
    }

    // ------------------------------------------------------------ public additions

    /// URL requested by page JS (link click handled in JS, `location.href=`). Shell should `load_url` it.
    pub fn take_navigation(&mut self) -> Option<String> {
        self.pending_nav.take()
    }

    /// Dispatch a DOM event (e.g. "click") on the element whose `id` attribute is `element_id`.
    pub fn fire_event(&mut self, element_id: &str, event: &str) -> Result<bool, String> {
        let (id, ev) = (element_id.to_string(), event.to_string());
        self.guard(move |e| {
            let Some(js) = e.js.as_mut() else { return Ok(false) };
            let hit = dro_dom_js::fire(js, &id, &ev)?;
            e.apply_js()?;
            Ok(hit)
        })
    }

    /// Advance JS timers by `elapsed_ms` (call ~every frame / 16 ms). Returns number of callbacks run.
    pub fn run_timers(&mut self, elapsed_ms: u32) -> Result<usize, String> {
        self.guard(move |e| {
            let Some(js) = e.js.as_mut() else { return Ok(0) };
            let n = dro_dom_js::run_timers(js, elapsed_ms);
            if n > 0 { e.apply_js()?; }
            Ok(n)
        })
    }

    /// Re-run styling for a new viewport (media queries, vw/vh). Replaces the old `set_viewport` body.
    pub fn resize(&mut self, w: u32, h: u32) -> Result<(), String> {
        self.guard(move |e| {
            e.vp = (w.max(1), h.max(1));
            e.restyle();
            Ok(())
        })
    }

    pub fn document_height(&mut self) -> Result<f32, String> {
        self.guard(|e| {
            e.ensure_layout()?;
            Ok(e.layout.as_ref().map(|l| l.height).unwrap_or(0.0))
        })
    }

    pub fn scroll_y(&self) -> f32 {
        self.scroll
    }

    /// Find-in-page: rectangles `(x, y, w, h)` in DOCUMENT coordinates (subtract `scroll_y()` to draw).
    /// Multi-word queries match consecutive words.
    pub fn find_in_page(&mut self, query: &str) -> Result<Vec<(f32, f32, f32, f32)>, String> {
        let q = query.to_lowercase();
        self.guard(move |e| {
            e.ensure_layout()?;
            let toks: Vec<&str> = q.split_whitespace().collect();
            let Some(l) = e.layout.as_ref() else { return Ok(vec![]) };
            if toks.is_empty() { return Ok(vec![]); }
            let words: Vec<(&layout_item::Item, String)> = l
                .items
                .iter()
                .filter_map(|it| match &it.paint {
                    Paint::Word { text, .. } => Some((it, text.to_lowercase())),
                    _ => None,
                })
                .collect();
            let mut out = vec![];
            let mut i = 0;
            while i + toks.len() <= words.len() {
                if toks.iter().enumerate().all(|(j, t)| words[i + j].1.contains(t)) {
                    let (a, b) = (words[i].0, words[i + toks.len() - 1].0);
                    let (x0, x1) = (a.x.min(b.x), (a.x + a.w).max(b.x + b.w));
                    out.push((x0, a.y.min(b.y), x1 - x0, a.h.max(b.h)));
                    i += toks.len();
                } else {
                    i += 1;
                }
            }
            Ok(out)
        })
    }

    /// Visible text of the page (reader-mode style): scripts/styles skipped, blocks separated by newlines.
    pub fn page_text(&self) -> String {
        const BLOCK: [&str; 16] = ["p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "br", "tr", "section", "article", "pre", "blockquote", "hr"];
        let mut out = String::new();
        let Some(body) = self.dom.find_tag("body") else { return out };
        for n in self.dom.descendants(body) {
            match &self.dom.nodes[n].kind {
                NodeKind::Element { tag, .. } if BLOCK.contains(&tag.as_str()) => {
                    if !out.ends_with('\n') { out.push('\n'); }
                }
                NodeKind::Text(t) => {
                    let mut hidden = false;
                    let mut p = self.dom.nodes[n].parent;
                    while let Some(a) = p {
                        if matches!(self.dom.tag(a), Some("script") | Some("style") | Some("noscript") | Some("template")) { hidden = true; break; }
                        p = self.dom.nodes[a].parent;
                    }
                    if hidden { continue; }
                    let w = t.split_whitespace().collect::<Vec<_>>().join(" ");
                    if !w.is_empty() {
                        if !out.is_empty() && !out.ends_with('\n') && !out.ends_with(' ') { out.push(' '); }
                        out.push_str(&w);
                    }
                }
                _ => {}
            }
        }
        out.trim().to_string()
    }

    /// Debug tree dump (devtools-lite). Depth capped at 40, text truncated.
    pub fn dump_dom(&self) -> String {
        let mut out = String::new();
        let mut stack: Vec<(NodeId, usize)> = self.dom.nodes[0].children.iter().rev().map(|&c| (c, 0)).collect();
        while let Some((n, d)) = stack.pop() {
            if d > 40 { continue; }
            let pad = "  ".repeat(d);
            match &self.dom.nodes[n].kind {
                NodeKind::Element { tag, attrs } => {
                    let a: String = attrs.iter().take(4).map(|(k, v)| format!(" {k}=\"{}\"", v.chars().take(30).collect::<String>())).collect();
                    out.push_str(&format!("{pad}<{tag}{a}>\n"));
                }
                NodeKind::Text(t) if !t.trim().is_empty() => {
                    out.push_str(&format!("{pad}\"{}\"\n", t.trim().chars().take(40).collect::<String>()));
                }
                _ => {}
            }
            stack.extend(self.dom.nodes[n].children.iter().rev().map(|&c| (c, d + 1)));
        }
        out
    }
}

use crate::dro_layout as layout_item;
