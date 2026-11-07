//! RAM-only HTTP cache (never written to disk): Cache-Control max-age / no-store / no-cache / immutable,
//! ETag + Last-Modified revalidation (304), byte-bounded LRU. GET 200 responses only.
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Entry {
    pub url: String, // final URL after redirects
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    stored_at: Instant,
    max_age: Duration,
}

/// Response headers relevant to caching (pass `None` for absent ones).
#[derive(Default, Clone, Copy)]
pub struct Headers<'a> {
    pub cache_control: Option<&'a str>,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
}

#[derive(Debug, PartialEq, Default)]
pub struct Policy { pub no_store: bool, pub no_cache: bool, pub max_age: Option<u64>, pub immutable: bool }

pub fn parse_cache_control(h: &str) -> Policy {
    let mut p = Policy::default();
    for d in h.split(',') {
        let d = d.trim().to_ascii_lowercase();
        match d.split_once('=') {
            Some(("max-age", v)) => p.max_age = v.trim().trim_matches('"').parse().ok(),
            _ => match d.as_str() { "no-store" => p.no_store = true, "no-cache" => p.no_cache = true, "immutable" => p.immutable = true, _ => {} },
        }
    }
    p
}

pub enum Lookup<'a> {
    Fresh(&'a Entry),
    /// Present but expired: send a conditional request with these validators, then call `refresh` on 304.
    Stale { etag: Option<String>, last_modified: Option<String> },
    Miss,
}

pub struct Cache {
    map: HashMap<String, Entry>,
    lru: VecDeque<String>, // front = oldest
    bytes: usize,
    pub max_bytes: usize,
    pub max_entry: usize,
}

impl Cache {
    pub fn new(max_bytes: usize) -> Self { Cache { map: HashMap::new(), lru: VecDeque::new(), bytes: 0, max_bytes, max_entry: 8 * 1024 * 1024 } }

    fn touch(&mut self, url: &str) {
        self.lru.retain(|u| u != url);
        self.lru.push_back(url.to_string());
    }

    pub fn lookup(&mut self, url: &str) -> Lookup<'_> { self.lookup_at(url, Instant::now()) }

    pub fn lookup_at(&mut self, url: &str, now: Instant) -> Lookup<'_> {
        if !self.map.contains_key(url) { return Lookup::Miss; }
        self.touch(url);
        let Some(e) = self.map.get(url) else { return Lookup::Miss };
        if now.saturating_duration_since(e.stored_at) < e.max_age { Lookup::Fresh(e) }
        else { Lookup::Stale { etag: e.etag.clone(), last_modified: e.last_modified.clone() } }
    }

    /// Store a 200 GET response. Returns false when policy/size says not to cache it.
    pub fn store(&mut self, url: &str, status: u16, content_type: &str, body: Vec<u8>, h: Headers<'_>) -> bool {
        self.store_at(url, status, content_type, body, h, Instant::now())
    }

    pub fn store_at(&mut self, url: &str, status: u16, content_type: &str, body: Vec<u8>, h: Headers<'_>, now: Instant) -> bool {
        if status != 200 || body.len() > self.max_entry { return false; }
        let pol = h.cache_control.map(parse_cache_control).unwrap_or_default();
        if pol.no_store { return false; }
        let has_validator = h.etag.is_some() || h.last_modified.is_some();
        let max_age = if pol.no_cache { 0 } else if pol.immutable { 365 * 24 * 3600 } else { pol.max_age.unwrap_or(0) };
        if max_age == 0 && !has_validator { return false; } // useless entry: could never be reused
        self.remove(url);
        let size = body.len() + url.len() + 128;
        let e = Entry { url: url.into(), status, content_type: content_type.into(), body, etag: h.etag.map(|s| s.to_string()), last_modified: h.last_modified.map(|s| s.to_string()), stored_at: now, max_age: Duration::from_secs(max_age) };
        self.bytes += size;
        self.map.insert(url.to_string(), e);
        self.lru.push_back(url.to_string());
        while (self.bytes > self.max_bytes || self.map.len() > 2000) && self.lru.len() > 1 {
            if let Some(old) = self.lru.pop_front() { self.drop_entry(&old); }
        }
        true
    }

    fn drop_entry(&mut self, url: &str) {
        if let Some(e) = self.map.remove(url) { self.bytes = self.bytes.saturating_sub(e.body.len() + e.url.len() + 128); }
    }

    /// Server answered 304: the stored body is valid again (optionally with new max-age).
    pub fn refresh(&mut self, url: &str, h: Headers<'_>) -> Option<&Entry> { self.refresh_at(url, h, Instant::now()) }

    pub fn refresh_at(&mut self, url: &str, h: Headers<'_>, now: Instant) -> Option<&Entry> {
        let pol = h.cache_control.map(parse_cache_control);
        let e = self.map.get_mut(url)?;
        e.stored_at = now;
        if let Some(p) = pol { if let Some(m) = p.max_age { e.max_age = Duration::from_secs(m); } if p.no_cache { e.max_age = Duration::ZERO; } }
        if let Some(t) = h.etag { e.etag = Some(t.to_string()); }
        if let Some(l) = h.last_modified { e.last_modified = Some(l.to_string()); }
        self.touch(url);
        self.map.get(url)
    }

    pub fn remove(&mut self, url: &str) { self.drop_entry(url); self.lru.retain(|u| u != url); }
    pub fn clear(&mut self) { self.map.clear(); self.lru.clear(); self.bytes = 0; }
    pub fn len(&self) -> usize { self.map.len() }
    pub fn bytes(&self) -> usize { self.bytes }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hdr(cc: &str) -> Headers<'_> { Headers { cache_control: Some(cc), ..Default::default() } }

    #[test]
    fn parses_policy() {
        assert_eq!(parse_cache_control("public, max-age=3600"), Policy { max_age: Some(3600), ..Default::default() });
        assert!(parse_cache_control("No-Store").no_store);
        assert!(parse_cache_control("max-age=0, no-cache").no_cache);
        assert!(parse_cache_control("max-age=31536000, immutable").immutable);
        assert_eq!(parse_cache_control("max-age=abc").max_age, None);
    }

    #[test]
    fn fresh_then_stale_then_refresh() {
        let mut c = Cache::new(1 << 20);
        let t0 = Instant::now();
        assert!(c.store_at("u", 200, "text/css", b"body{}".to_vec(), Headers { cache_control: Some("max-age=60"), etag: Some("\"e1\""), last_modified: None }, t0));
        assert!(matches!(c.lookup_at("u", t0 + Duration::from_secs(30)), Lookup::Fresh(e) if e.body == b"body{}"));
        match c.lookup_at("u", t0 + Duration::from_secs(61)) { Lookup::Stale { etag, .. } => assert_eq!(etag.as_deref(), Some("\"e1\"")), _ => panic!("expected stale") }
        c.refresh_at("u", hdr("max-age=120"), t0 + Duration::from_secs(61));
        assert!(matches!(c.lookup_at("u", t0 + Duration::from_secs(150)), Lookup::Fresh(_)));
        assert!(matches!(c.lookup_at("nope", t0), Lookup::Miss));
    }

    #[test]
    fn policy_rules() {
        let mut c = Cache::new(1 << 20);
        assert!(!c.store("a", 200, "t", vec![1], hdr("no-store")));
        assert!(!c.store("b", 404, "t", vec![1], hdr("max-age=60")));
        assert!(!c.store("c", 200, "t", vec![1], Headers::default())); // no freshness, no validators
        assert!(c.store("d", 200, "t", vec![1], Headers { etag: Some("x"), ..Default::default() })); // validator only -> stale+revalidate
        assert!(matches!(c.lookup("d"), Lookup::Stale { .. }));
        assert!(c.store("e", 200, "t", vec![1], Headers { cache_control: Some("no-cache"), etag: Some("y"), last_modified: None }));
        assert!(matches!(c.lookup("e"), Lookup::Stale { .. }));
        assert!(c.store("f", 200, "t", vec![1], hdr("immutable")));
        assert!(matches!(c.lookup("f"), Lookup::Fresh(_)));
    }

    #[test]
    fn lru_eviction_by_bytes_and_entry_cap() {
        let mut c = Cache::new(1000);
        c.max_entry = 600;
        for i in 0..5 { assert!(c.store(&format!("u{i}"), 200, "t", vec![0; 300], hdr("max-age=60"))); }
        assert!(c.bytes() <= 1000 + 400, "{}", c.bytes());
        assert!(matches!(c.lookup("u0"), Lookup::Miss)); // oldest evicted
        assert!(matches!(c.lookup("u4"), Lookup::Fresh(_)));
        assert!(!c.store("big", 200, "t", vec![0; 700], hdr("max-age=60"))); // over max_entry
    }

    #[test]
    fn touch_protects_recent_and_replace_updates_size() {
        let mut c = Cache::new(1000);
        c.store("a", 200, "t", vec![0; 300], hdr("max-age=60"));
        c.store("b", 200, "t", vec![0; 300], hdr("max-age=60"));
        let _ = c.lookup("a"); // a is now newest
        c.store("c", 200, "t", vec![0; 300], hdr("max-age=60"));
        assert!(matches!(c.lookup("b"), Lookup::Miss) && matches!(c.lookup("a"), Lookup::Fresh(_)));
        let before = c.bytes();
        c.store("a", 200, "t", vec![0; 10], hdr("max-age=60"));
        assert!(c.bytes() < before);
        c.clear();
        assert_eq!((c.len(), c.bytes()), (0, 0));
    }
}
