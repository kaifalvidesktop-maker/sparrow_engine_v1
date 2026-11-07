//! SparrowEngine: one page = DOM + styles + layout + JS + media. Everything is RAM-only.
//! Child modules: `bridge` (page install, JS DOM, events, timers, find) and `media_bridge` (<video>/<audio>).
use crate::dro_css::{self, Style};
use crate::dro_dom::*;
use crate::dro_js::JsRuntime;
use crate::dro_layout::{self, Layout, Paint};
use crate::{dro_charset, dro_html, dro_net, dro_paint, safety};
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use tiny_skia::Pixmap;

mod bridge;
mod media_bridge;

pub const CRASH_MSG: &str = "Page crashed, reload?";

pub struct SparrowEngine {
    url: String,
    title: String,
    status: u16,
    dom: Dom,
    styles: Vec<Option<Style>>,
    layout: Option<Layout>,
    vp: (u32, u32),
    scroll: f32,
    js: Option<JsRuntime>,
    crashed: bool,
    // used by bridge.rs / media_bridge.rs
    link_css: Vec<String>,
    pending_nav: Option<String>,
    images: HashMap<String, Pixmap>,
    media: Vec<media_bridge::Slot>,
}

/// Percent-decode a URL fragment (ids such as `#%E0%A6%AC` -> `ব`).
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl SparrowEngine {
    pub fn new() -> Self {
        safety::install_panic_hook(); // panics go to the RAM log, never to disk/console
        SparrowEngine {
            url: "about:blank".into(), title: String::new(), status: 0, dom: Dom::new(), styles: vec![], layout: None,
            vp: (1024, 768), scroll: 0.0, js: None, crashed: false,
            link_css: vec![], pending_nav: None, images: HashMap::new(), media: vec![],
        }
    }

    /// Every public method goes through here: a panic never reaches the shell.
    fn guard<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        if self.crashed { return Err(CRASH_MSG.into()); }
        match catch_unwind(AssertUnwindSafe(|| f(self))) {
            Ok(r) => r,
            Err(_) => { self.crashed = true; safety::log(format!("engine panic on {}", self.url)); Err(CRASH_MSG.into()) }
        }
    }

    pub fn is_crashed(&self) -> bool { self.crashed }
    pub fn recover(&mut self) { *self = SparrowEngine { vp: self.vp, url: self.url.clone(), ..SparrowEngine::new() }; }
    pub fn url(&self) -> &str { &self.url }
    pub fn title(&self) -> &str { &self.title }
    /// HTTP status of the last main-page load (0 = none).
    pub fn status(&self) -> u16 { self.status }
    pub fn viewport(&self) -> (u32, u32) { self.vp }
    /// Re-styles for the new size (media queries, vw/vh).
    pub fn set_viewport(&mut self, w: u32, h: u32) {
        let (w, h) = (w.max(1), h.max(1));
        if safety::limits::viewport_ok(w, h).is_err() { return; }
        self.vp = (w, h);
        self.layout = None;
        let _ = self.resize(w, h);
    }
    /// Network counters (requests, cache hits, blocked trackers, ...).
    pub fn net_stats(&self) -> dro_net::Stats { dro_net::stats() }
    /// "Clear browsing data": cookies, HTTP cache, HSTS (all RAM) + this page's images/JS storage.
    pub fn clear_session_data(&mut self) {
        dro_net::clear_all();
        self.images.clear();
        if self.js.is_some() { let _ = self.eval_js("try{localStorage.clear();sessionStorage.clear()}catch(e){}"); }
    }

    pub fn reload(&mut self) -> Result<(), String> {
        if self.crashed { self.recover(); }
        let u = self.url.clone();
        self.guard(move |e| e.fetch_page(&u))
    }

    pub fn load_url(&mut self, input: &str) -> Result<(), String> {
        if self.crashed { self.recover(); }
        let t = input.trim().to_string();
        self.guard(move |e| match e.same_doc(&t) {
            Some((full, frag)) => e.jump(&full, &frag),
            None => e.fetch_page(&t),
        })
    }

    /// Load raw HTML (no network for the page itself); sub-resources resolve against `base`.
    pub fn load_html(&mut self, html: &str, base: &str) -> Result<(), String> {
        if self.crashed { self.recover(); }
        let (h, b) = (html.to_string(), base.to_string());
        self.guard(move |e| e.install(&h, &b))
    }

    /// Same page, different #fragment -> scroll only, no refetch.
    fn same_doc(&self, t: &str) -> Option<(String, String)> {
        if t.is_empty() || self.url == "about:blank" || self.dom.nodes.len() < 2 { return None; }
        let new = url::Url::parse(&dro_net::resolve(&self.url, t)?).ok()?;
        let frag = new.fragment()?.to_string();
        let mut a = new.clone();
        a.set_fragment(None);
        let mut b = url::Url::parse(&self.url).ok()?;
        b.set_fragment(None);
        (a == b).then(|| (new.to_string(), frag))
    }

    fn jump(&mut self, full: &str, frag: &str) -> Result<(), String> {
        self.ensure_layout()?;
        let key = pct_decode(frag);
        self.url = full.to_string();
        self.scroll = if frag.is_empty() || frag == "top" { 0.0 } else { self.layout.as_ref().and_then(|l| l.anchors.get(frag).or_else(|| l.anchors.get(&key)).copied()).unwrap_or(self.scroll) };
        self.ensure_layout()
    }

    fn fetch_page(&mut self, t: &str) -> Result<(), String> {
        if t.is_empty() || t == "about:blank" { return self.install("", "about:blank"); }
        let r = dro_net::fetch(t)?;
        // trust the server's Content-Type; sniff only when it is missing/generic
        let declared = r.content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        let ct = if declared.is_empty() || declared == "application/octet-stream" { dro_net::sniff_mime(&r.body, &declared) } else { declared };
        let texty = !r.body.iter().take(512).any(|&b| b == 0);
        let html = if ct == "text/html" || ct == "application/xhtml+xml" || (ct == "application/octet-stream" && texty) {
            dro_charset::decode(&r.body, &r.content_type)
        } else if ct.starts_with("text/") || ct.ends_with("json") || ct.ends_with("javascript") || (ct.ends_with("xml") && !ct.starts_with("image/")) {
            format!("<html><body><pre>{}</pre></body></html>", esc(&dro_charset::decode(&r.body, &r.content_type)))
        } else if ct.starts_with("image/") && ct != "image/svg+xml" {
            format!("<html><body><img src=\"{}\"></body></html>", esc(&r.url))
        } else {
            return Err(format!("Cannot display {ct}"));
        };
        self.install(&html, &r.url)?;
        self.status = r.status;
        Ok(())
    }

    /// Page install (bridge::install_page) + media slots for <video>/<audio>.
    fn install(&mut self, html: &str, base: &str) -> Result<(), String> {
        dro_net::set_page(base);
        self.install_page(html, base)?;
        let b = self.url.clone();
        self.setup_media(&b);
        Ok(())
    }

    fn ensure_layout(&mut self) -> Result<(), String> {
        if self.layout.is_none() {
            let sizes: HashMap<String, (u32, u32)> = self.images.iter().map(|(k, p)| (k.clone(), (p.width(), p.height()))).collect();
            self.layout = Some(dro_layout::compute(&self.dom, &self.styles, self.vp.0 as f32, &self.url, &sizes)?);
        }
        let max = (self.layout.as_ref().map_or(0.0, |l| l.height) - self.vp.1 as f32).max(0.0);
        self.scroll = if self.scroll.is_finite() { self.scroll.clamp(0.0, max) } else { 0.0 };
        Ok(())
    }

    /// Returns RGBA pixels (viewport width * height * 4); media control bars are overlaid.
    pub fn render(&mut self) -> Result<Vec<u8>, String> {
        self.guard(|e| {
            e.ensure_layout()?;
            let px = {
                let l = e.layout.as_ref().ok_or("no layout")?;
                dro_paint::paint(l, e.vp.0, e.vp.1, e.scroll, &e.images)?
            };
            Ok(e.overlay_media(px))
        })
    }

    pub fn eval_js(&mut self, code: &str) -> Result<String, String> {
        let code = code.to_string();
        self.guard(move |e| {
            if e.js.is_none() { e.js = Some(JsRuntime::new()?); }
            let out = e.js.as_mut().ok_or("js unavailable")?.eval(&code);
            e.apply_js()?; // pick up DOM changes / navigation made by the snippet
            out
        })
    }

    /// console.log / alert output since last call
    pub fn take_logs(&mut self) -> Vec<String> {
        self.js.as_ref().map(|j| j.take_logs()).unwrap_or_default()
    }

    pub fn scroll_by(&mut self, dy: f32) -> Result<f32, String> {
        self.guard(move |e| {
            e.ensure_layout()?;
            if dy.is_finite() { e.scroll += dy; }
            e.ensure_layout()?;
            Ok(e.scroll)
        })
    }

    /// Click at viewport (x, y). Returns resolved link URL if a link was hit.
    /// (Shell: call `media_click` first; if it returns true, skip this.)
    pub fn hit_test(&self, x: f32, y: f32) -> Option<String> {
        let l = self.layout.as_ref()?;
        let y = y + self.scroll;
        l.items.iter().rev().find_map(|it| match &it.paint {
            Paint::Word { link: Some(h), .. } | Paint::Image { link: Some(h), .. } if x >= it.x && x <= it.x + it.w && y >= it.y && y <= it.y + it.h => dro_net::resolve(&self.url, h),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn escapes_html() { assert_eq!(esc("<a href=\"x\">&"), "&lt;a href=&quot;x&quot;&gt;&amp;"); }
    #[test] fn blank_engine_state() {
        let e = SparrowEngine::new();
        assert_eq!(e.url(), "about:blank");
        assert!(!e.is_crashed());
        assert_eq!(e.status(), 0);
    }
    #[test] fn same_doc_needs_loaded_page() { assert!(SparrowEngine::new().same_doc("#x").is_none()); }
    #[test] fn load_html_sets_title_and_renders() {
        let mut e = SparrowEngine::new();
        e.set_viewport(200, 100);
        e.load_html("<title>T</title><p>hi", "https://example.com/").unwrap();
        assert_eq!(e.title(), "T");
        assert_eq!(e.render().unwrap().len(), 200 * 100 * 4);
    }
    #[test] fn recover_keeps_viewport_and_url() {
        let mut e = SparrowEngine::new();
        e.set_viewport(300, 200);
        e.recover();
        assert_eq!(e.viewport(), (300, 200));
        assert!(!e.is_crashed());
    }
}
