//! Tracker / ad blocking for sub-resources (scripts, images, CSS, media). Top-level navigation is never blocked.
//! Rules: hosts-file lines (`0.0.0.0 ads.example.com`), `||domain^`, plain domains, and `/path/` substring patterns.
//! Pure std + `url`. Shell can load bigger lists (EasyList-hosts style) with `with_global(|b| b.load_list(text))`.
use std::collections::HashSet;
use std::sync::{OnceLock, RwLock};

const DEFAULT: &str = "doubleclick.net googlesyndication.com googleadservices.com google-analytics.com googletagmanager.com googletagservices.com
adservice.google.com pagead2.googlesyndication.com connect.facebook.net facebook.com/tr scorecardresearch.com quantserve.com
taboola.com outbrain.com adnxs.com criteo.com criteo.net pubmatic.com rubiconproject.com openx.net adsrvr.org casalemedia.com
hotjar.com mixpanel.com segment.io segment.com amplitude.com fullstory.com mc.yandex.ru yandex.ru/metrika bat.bing.com
clarity.ms moatads.com amazon-adsystem.com advertising.com adform.net smartadserver.com 2mdn.net serving-sys.com
ads.twitter.com analytics.twitter.com ads.linkedin.com px.ads.linkedin.com snap.licdn.com analytics.tiktok.com
popads.net propellerads.com exoclick.com juicyads.com adsterra.com mgid.com revcontent.com zedo.com chartbeat.com
/ads/ /adserver/ /doubleclick/ /pagead/ /tracking/ /analytics.js /gtag/js /beacon.js";

pub struct Blocker {
    hosts: HashSet<String>,
    patterns: Vec<String>,
    pub enabled: bool,
}

fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(|h| h.to_ascii_lowercase())
}

impl Blocker {
    pub fn new() -> Self {
        let mut b = Blocker { hosts: HashSet::new(), patterns: vec![], enabled: true };
        b.load_list(DEFAULT);
        b
    }
    pub fn empty() -> Self { Blocker { hosts: HashSet::new(), patterns: vec![], enabled: true } }

    pub fn add_rule(&mut self, rule: &str) {
        let r = rule.trim();
        if r.is_empty() || r.starts_with('#') || r.starts_with('!') { return; }
        let r = r.strip_prefix("0.0.0.0 ").or_else(|| r.strip_prefix("127.0.0.1 ")).unwrap_or(r).trim();
        let r = r.strip_prefix("||").unwrap_or(r).trim_end_matches('^').to_ascii_lowercase();
        if r.is_empty() || r == "localhost" { return; }
        if r.starts_with('/') || r.contains('/') { self.patterns.push(r); } else { self.hosts.insert(r); }
    }

    /// Whitespace/newline separated rules, `#`/`!` comments allowed.
    pub fn load_list(&mut self, text: &str) {
        for line in text.lines() {
            if line.trim_start().starts_with('!') { continue; } // ABP comment line
            let line = line.split('#').next().unwrap_or("");
            // hosts-file format has "ip domain"; other formats have one rule per token
            let toks: Vec<&str> = line.split_whitespace().collect();
            if toks.len() == 2 && (toks[0] == "0.0.0.0" || toks[0] == "127.0.0.1") { self.add_rule(toks[1]); continue; }
            for t in toks { self.add_rule(t); }
        }
    }

    pub fn rule_count(&self) -> usize { self.hosts.len() + self.patterns.len() }

    /// Is `url` (a sub-resource of the page at `page_url`) blocked? First-party requests are never blocked.
    pub fn is_blocked(&self, url: &str, page_url: &str) -> bool {
        if !self.enabled { return false; }
        let Some(host) = host_of(url) else { return false };
        if let Some(ph) = host_of(page_url) {
            if same_site(&host, &ph) { return false; }
        }
        // host and every parent domain: a.b.c.com -> b.c.com -> c.com
        let mut h = host.as_str();
        loop {
            if self.hosts.contains(h) { return true; }
            match h.split_once('.') { Some((_, rest)) if rest.contains('.') => h = rest, _ => break }
        }
        let lower = url.to_ascii_lowercase();
        let after_scheme = lower.split_once("://").map(|x| x.1).unwrap_or(&lower);
        self.patterns.iter().any(|p| after_scheme.contains(p.as_str()))
    }
}

fn same_site(a: &str, b: &str) -> bool {
    let base = |h: &str| { let p: Vec<&str> = h.split('.').collect(); if p.len() >= 2 { p[p.len() - 2..].join(".") } else { h.to_string() } };
    base(a) == base(b)
}

// ---- process-wide instance used by dro_net ----
static GLOBAL: OnceLock<RwLock<Blocker>> = OnceLock::new();

fn global() -> &'static RwLock<Blocker> { GLOBAL.get_or_init(|| RwLock::new(Blocker::new())) }

pub fn with_global<R>(f: impl FnOnce(&mut Blocker) -> R) -> Option<R> {
    global().write().ok().map(|mut g| f(&mut g))
}

/// Sub-resource check used by the network layer.
pub fn global_blocked(url: &str, page_url: &str) -> bool {
    global().read().map(|g| g.is_blocked(url, page_url)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocks_third_party_trackers() {
        let b = Blocker::new();
        assert!(b.is_blocked("https://www.google-analytics.com/analytics.js", "https://news.example.com/"));
        assert!(b.is_blocked("https://stats.g.doubleclick.net/x", "https://news.example.com/"));
        assert!(b.is_blocked("https://cdn.site.com/ads/banner.png", "https://other.org/"));
        assert!(!b.is_blocked("https://news.example.com/app.js", "https://news.example.com/"));
        assert!(!b.is_blocked("https://static.example.com/a.css", "https://www.example.com/"));
    }
    #[test]
    fn first_party_never_blocked() {
        let b = Blocker::new();
        assert!(!b.is_blocked("https://www.doubleclick.net/x", "https://doubleclick.net/"));
    }
    #[test]
    fn list_formats() {
        let mut b = Blocker::empty();
        b.load_list("# c\n0.0.0.0 bad.example\n||tracker.test^\n! comment\nplain.example\n/pixel/\n");
        assert_eq!(b.rule_count(), 4);
        assert!(b.is_blocked("https://bad.example/a", "https://x.org/"));
        assert!(b.is_blocked("https://sub.tracker.test/a", "https://x.org/"));
        assert!(b.is_blocked("https://x.com/pixel/1.gif", "https://y.org/"));
        let mut off = Blocker::new();
        off.enabled = false;
        assert!(!off.is_blocked("https://doubleclick.net/", "https://y.org/"));
    }
}
