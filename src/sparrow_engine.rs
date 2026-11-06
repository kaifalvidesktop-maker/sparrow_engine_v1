use crate::dro_css::{self, Style};
use crate::dro_dom::*;
use crate::dro_js::JsRuntime;
use crate::dro_layout::{self, Layout, Paint};
use crate::{dro_html, dro_net, dro_paint};
use std::panic::{catch_unwind, AssertUnwindSafe};
mod media_bridge;
mod bridge;

pub const CRASH_MSG: &str = "Page crashed, reload?";
const MAX_SHEETS: usize = 10;
const JS_TYPES: [&str; 5] = ["text/javascript", "application/javascript", "application/x-javascript", "text/ecmascript", "application/ecmascript"];

enum Src { Inline(String), Url(String) }

fn is_js(t: Option<&str>) -> bool {
    let t = t.unwrap_or("").trim();
    t.is_empty() || JS_TYPES.iter().any(|m| t.eq_ignore_ascii_case(m))
}
fn is_sheet(rel: Option<&str>) -> bool {
    rel.map_or(false, |r| r.split_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet")))
}

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
}

impl SparrowEngine {
    pub fn new() -> Self {
        SparrowEngine { url: "about:blank".into(), title: String::new(), status: 0, dom: Dom::new(), styles: vec![], layout: None, vp: (1024, 768), scroll: 0.0, js: None, crashed: false }
    }

    /// Every public method goes through here: a panic never reaches the shell.
    fn guard<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, String>) -> Result<T, String> {
        if self.crashed { return Err(CRASH_MSG.into()); }
        match catch_unwind(AssertUnwindSafe(|| f(self))) {
            Ok(r) => r,
            Err(_) => { self.crashed = true; Err(CRASH_MSG.into()) }
        }
    }

    pub fn is_crashed(&self) -> bool { self.crashed }
    pub fn recover(&mut self) { *self = SparrowEngine { vp: self.vp, url: self.url.clone(), ..SparrowEngine::new() }; }
    pub fn url(&self) -> &str { &self.url }
    pub fn title(&self) -> &str { &self.title }
    /// HTTP status of the last main-page load (0 = none).
    pub fn status(&self) -> u16 { self.status }
    pub fn viewport(&self) -> (u32, u32) { self.vp }
    pub fn set_viewport(&mut self, w: u32, h: u32) { self.vp = (w.max(1), h.max(1)); self.layout = None; }

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

    /// Load raw HTML (no network); external stylesheets resolve against `base`.
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
        self.url = full.to_string();
        self.scroll = if frag.is_empty() || frag == "top" { 0.0 } else { self.layout.as_ref().and_then(|l| l.anchors.get(frag).copied()).unwrap_or(self.scroll) };
        self.ensure_layout()
    }

    fn fetch_page(&mut self, t: &str) -> Result<(), String> {
        if t.is_empty() || t == "about:blank" { return self.install("", "about:blank"); }
        let r = dro_net::fetch(t)?;
        let ct = r.content_type.to_lowercase();
        let text = dro_net::decode(&r.body, &r.content_type);
        let html = if ct.contains("html") || ct.is_empty() { text }
            else if ct.starts_with("text/") { format!("<pre>{}</pre>", text.replace('&', "&amp;").replace('<', "&lt;")) }
            else { return Err(format!("Cannot display {ct}")); };
        self.install(&html, &r.url)?;
        self.status = r.status;
        Ok(())
    }

    /// Builds everything into locals first; state changes only on success.
    fn install(&mut self, html: &str, base: &str) -> Result<(), String> {
        let dom = dro_html::parse(html)?;
        let title = dom.find_tag("title").map(|t| dom.text_content(t).trim().to_string()).unwrap_or_default();
        let (mut srcs, mut scripts, mut links) = (vec![], vec![], 0);
        for n in dom.descendants(0) {
            match dom.tag(n) {
                Some("style") => srcs.push(Src::Inline(dom.text_content(n))),
                Some("script") if dom.attr(n, "src").is_none() && is_js(dom.attr(n, "type")) => scripts.push(dom.text_content(n)),
                Some("link") if is_sheet(dom.attr(n, "rel")) && links < MAX_SHEETS => {
                    if let Some(u) = dom.attr(n, "href").and_then(|h| dro_net::resolve(base, h)) { links += 1; srcs.push(Src::Url(u)); }
                }
                _ => {}
            }
        }
        // external stylesheets are fetched in parallel, applied in document order
        let css: Vec<String> = std::thread::scope(|s| {
            let hs: Vec<_> = srcs.iter().map(|src| match src {
                Src::Inline(t) => Err(t.clone()),
                Src::Url(u) => Ok(s.spawn(move || dro_net::fetch_sub(u).map(|r| dro_net::decode(&r.body, &r.content_type)).unwrap_or_default())),
            }).collect();
            hs.into_iter().map(|h| match h { Err(t) => t, Ok(h) => h.join().unwrap_or_default() }).collect()
        });
        let mut rules = dro_css::parse(dro_css::UA);
        for c in &css { rules.extend(dro_css::parse(c)); }
        let styles = dro_css::compute(&dom, &rules);
        let mut js = JsRuntime::new()?;
        for s in &scripts { let _ = js.eval(s); } // script errors never break the page
        self.url = base.to_string();
        self.dom = dom;
        self.title = title;
        self.styles = styles;
        self.js = Some(js);
        self.layout = None;
        self.scroll = 0.0;
        Ok(())
    }

    fn ensure_layout(&mut self) -> Result<(), String> {
        if self.layout.is_none() {
            self.layout = Some(dro_layout::compute(&self.dom, &self.styles, self.vp.0 as f32)?);
        }
        let max = (self.layout.as_ref().map_or(0.0, |l| l.height) - self.vp.1 as f32).max(0.0);
        self.scroll = if self.scroll.is_finite() { self.scroll.clamp(0.0, max) } else { 0.0 };
        Ok(())
    }

    /// Returns RGBA pixels (viewport width * height * 4).
    pub fn render(&mut self) -> Result<Vec<u8>, String> {
        self.guard(|e| {
            e.ensure_layout()?;
            let l = e.layout.as_ref().ok_or("no layout")?;
            dro_paint::paint(l, e.vp.0, e.vp.1, e.scroll)
        })
    }

    pub fn eval_js(&mut self, code: &str) -> Result<String, String> {
        let code = code.to_string();
        self.guard(move |e| {
            if e.js.is_none() { e.js = Some(JsRuntime::new()?); }
            e.js.as_mut().ok_or("js unavailable")?.eval(&code)
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
    pub fn hit_test(&self, x: f32, y: f32) -> Option<String> {
        let l = self.layout.as_ref()?;
        let y = y + self.scroll;
        l.items.iter().rev().find_map(|it| match &it.paint {
            Paint::Word { link: Some(h), .. } if x >= it.x && x <= it.x + it.w && y >= it.y && y <= it.y + it.h => dro_net::resolve(&self.url, h),
            _ => None,
        })
    }
}
