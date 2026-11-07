//! dro_net.rs — Sparrow network stack. RAM-only: cookies, cache, HSTS never touch disk.
//! Cargo: reqwest = { version = "0.11", default-features = false,
//!   features = ["blocking","rustls-tls","cookies","gzip","brotli","deflate","http2"] }  (>= 0.11.18)
//! Old API kept: Response, fetch, fetch_sub, par_map(urls, f), decode, resolve.
//! New: fetch_max, fetch_range, post_form, send(Request), par_map_page, RAM cookie jar (export/import
//! for a login vault), HTTP cache + revalidation, HSTS, HTTPS-first, retries, Chrome-like headers,
//! referrer policy, private-network protection, data: URLs, MIME sniffing, stats.
use reqwest::cookie::CookieStore;
use reqwest::header::HeaderValue;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAIN_LIMIT: u64 = 20 * 1024 * 1024;
const SUB_LIMIT: u64 = 5 * 1024 * 1024;
const MAX_PAR: usize = 8;
const MAX_REDIRECTS: usize = 10;
const CACHE_MAX: usize = 48 * 1024 * 1024;
const CACHE_ENTRY_MAX: usize = 4 * 1024 * 1024;
const MAX_COOKIES: usize = 3000;
const SLDS: &[&str] = &["co.uk", "org.uk", "ac.uk", "gov.uk", "com.au", "net.au", "org.au", "co.in", "net.in", "com.bd", "net.bd", "org.bd", "edu.bd", "gov.bd", "co.jp", "ne.jp", "com.br", "com.cn", "co.nz", "co.za", "com.sg", "com.my", "com.pk", "com.tr", "com.mx"];
const DEFAULT_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Sparrow/0.2";

#[derive(Clone, Debug)]
pub struct Response {
    pub url: String,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

// ---------------------------------------------------------------- config / stats
#[derive(Clone)]
pub struct Config {
    pub user_agent: String,
    pub accept_language: String,
    pub https_first: bool,
    pub block_third_party_cookies: bool,
    pub send_dnt: bool,
    pub cache: bool,
    pub block_private_subresources: bool,
}
impl Default for Config {
    fn default() -> Self {
        Config { user_agent: DEFAULT_UA.into(), accept_language: "en-US,en;q=0.9,bn;q=0.8".into(), https_first: true, block_third_party_cookies: false, send_dnt: true, cache: true, block_private_subresources: true }
    }
}
fn cfg() -> &'static RwLock<Config> { static C: OnceLock<RwLock<Config>> = OnceLock::new(); C.get_or_init(|| RwLock::new(Config::default())) }
pub fn config() -> Config { cfg().read().map(|c| c.clone()).unwrap_or_default() }
pub fn set_config(c: Config) { if let Ok(mut g) = cfg().write() { *g = c; } }

static REQS: AtomicU64 = AtomicU64::new(0);
static HITS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static BLOCKED: AtomicU64 = AtomicU64::new(0);
static RETRIES: AtomicU64 = AtomicU64::new(0);
static UPGRADES: AtomicU64 = AtomicU64::new(0);
#[derive(Debug, Clone, Copy)]
pub struct Stats { pub requests: u64, pub cache_hits: u64, pub bytes_in: u64, pub blocked: u64, pub retries: u64, pub https_upgrades: u64 }
pub fn stats() -> Stats { Stats { requests: REQS.load(Relaxed), cache_hits: HITS.load(Relaxed), bytes_in: BYTES.load(Relaxed), blocked: BLOCKED.load(Relaxed), retries: RETRIES.load(Relaxed), https_upgrades: UPGRADES.load(Relaxed) } }

// current top-level page (used for third-party decisions and the old 2-arg par_map)
static PAGE: RwLock<String> = RwLock::new(String::new());
pub fn set_page(u: &str) { if let Ok(mut g) = PAGE.write() { *g = u.to_string(); } }
fn page() -> String { PAGE.read().map(|g| g.clone()).unwrap_or_default() }
fn page_host() -> Option<String> { url::Url::parse(&page()).ok().and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase())) }

fn now_secs() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

// ---------------------------------------------------------------- domains
pub fn reg_domain(host: &str) -> String {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    if h.parse::<std::net::IpAddr>().is_ok() { return h; }
    let p: Vec<&str> = h.split('.').collect();
    if p.len() <= 2 { return h; }
    let two = format!("{}.{}", p[p.len() - 2], p[p.len() - 1]);
    let n = if SLDS.contains(&two.as_str()) { 3 } else { 2 };
    p[p.len().saturating_sub(n)..].join(".")
}
pub fn same_site(a: &str, b: &str) -> bool { reg_domain(a) == reg_domain(b) }

fn is_private_url(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Domain(d)) => { let d = d.to_ascii_lowercase(); d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local") || d.ends_with(".internal") }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified(),
        Some(url::Host::Ipv6(ip)) => { let s = ip.segments()[0]; ip.is_loopback() || ip.is_unspecified() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80 }
        None => false,
    }
}

// ---------------------------------------------------------------- RAM cookie jar
#[derive(Clone, Debug)]
pub struct Cookie { pub name: String, pub value: String, pub domain: String, pub host_only: bool, pub path: String, pub expires: Option<u64>, pub secure: bool, pub http_only: bool, pub same_site: String }

#[derive(Default)]
pub struct RamJar { jar: Mutex<Vec<Cookie>> }

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}
pub fn parse_http_date(s: &str) -> Option<u64> {
    let s = s.replace('-', " ");
    let p: Vec<&str> = s.split_whitespace().collect();
    if p.len() < 5 { return None; }
    let day: u32 = p[1].parse().ok()?;
    let mon = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"].iter().position(|m| p[2].to_ascii_lowercase().starts_with(m))? as u32 + 1;
    let mut year: i64 = p[3].parse().ok()?;
    if year < 100 { year += if year < 70 { 2000 } else { 1900 }; }
    let hms: Vec<i64> = p[4].split(':').filter_map(|x| x.parse().ok()).collect();
    if hms.len() != 3 { return None; }
    let secs = days_from_civil(year, mon, day) * 86400 + hms[0] * 3600 + hms[1] * 60 + hms[2];
    Some(secs.max(0) as u64)
}
fn domain_match(host: &str, c: &Cookie) -> bool { if c.host_only { host == c.domain } else { host == c.domain || host.ends_with(&format!(".{}", c.domain)) } }
fn path_match(req: &str, cp: &str) -> bool { req == cp || (req.starts_with(cp) && (cp.ends_with('/') || req[cp.len()..].starts_with('/'))) }

fn parse_set_cookie(h: &str, url: &url::Url, now: u64) -> Option<Cookie> {
    let host = url.host_str()?.to_ascii_lowercase();
    let mut it = h.split(';');
    let (n, v) = it.next()?.split_once('=')?;
    let name = n.trim();
    if name.is_empty() || name.len() + v.len() > 4096 { return None; }
    let (mut domain, mut path, mut maxage, mut expires) = (None::<String>, None::<String>, None::<i64>, None::<u64>);
    let (mut secure, mut http_only, mut same) = (false, false, String::from("lax"));
    for a in it {
        let (k, val) = match a.split_once('=') { Some((k, v)) => (k.trim().to_ascii_lowercase(), v.trim()), None => (a.trim().to_ascii_lowercase(), "") };
        match k.as_str() {
            "domain" => domain = Some(val.trim_start_matches('.').to_ascii_lowercase()),
            "path" => if val.starts_with('/') { path = Some(val.to_string()) },
            "max-age" => maxage = val.parse().ok(),
            "expires" => expires = parse_http_date(val),
            "secure" => secure = true,
            "httponly" => http_only = true,
            "samesite" => same = val.to_ascii_lowercase(),
            _ => {}
        }
    }
    if secure && url.scheme() != "https" { return None; }
    let host_only = domain.is_none();
    let dom = match domain {
        Some(d) => {
            let ok = d.contains('.') && !SLDS.contains(&d.as_str()) && (host == d || host.ends_with(&format!(".{d}")));
            if !ok { return None; }
            d
        }
        None => host,
    };
    let path = path.unwrap_or_else(|| { let p = url.path(); match p.rfind('/') { Some(i) if i > 0 => p[..i].to_string(), _ => "/".into() } });
    if name.starts_with("__Secure-") && !secure { return None; }
    if name.starts_with("__Host-") && !(secure && path == "/" && host_only) { return None; }
    let expires = match maxage { Some(m) if m <= 0 => Some(0), Some(m) => Some(now + m as u64), None => expires };
    Some(Cookie { name: name.into(), value: v.trim().into(), domain: dom, host_only, path, expires, secure, http_only, same_site: same })
}

impl RamJar {
    pub fn new() -> Self { RamJar { jar: Mutex::new(Vec::new()) } }
    fn store(&self, headers: &[String], url: &url::Url, top: Option<&str>, block_3p: bool) {
        let host = match url.host_str() { Some(h) => h.to_ascii_lowercase(), None => return };
        if block_3p { if let Some(t) = top { if !same_site(&host, t) { return; } } }
        let now = now_secs();
        let mut g = match self.jar.lock() { Ok(g) => g, Err(_) => return };
        for h in headers {
            let c = match parse_set_cookie(h, url, now) { Some(c) => c, None => continue };
            g.retain(|o| !(o.name == c.name && o.domain == c.domain && o.path == c.path && o.host_only == c.host_only));
            if c.expires.map_or(false, |e| e <= now) { continue; }
            if g.len() >= MAX_COOKIES { g.remove(0); }
            g.push(c);
        }
    }
    fn header_for(&self, url: &url::Url, top: Option<&str>, block_3p: bool) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        if block_3p { if let Some(t) = top { if !same_site(&host, t) { return None; } } }
        let (now, https, path) = (now_secs(), url.scheme() == "https", url.path());
        let g = self.jar.lock().ok()?;
        let mut m: Vec<&Cookie> = g.iter().filter(|c| c.expires.map_or(true, |e| e > now) && (!c.secure || https) && domain_match(&host, c) && path_match(path, &c.path)).collect();
        if m.is_empty() { return None; }
        m.sort_by(|a, b| b.path.len().cmp(&a.path.len()));
        Some(m.iter().map(|c| format!("{}={}", c.name, c.value)).collect::<Vec<_>>().join("; "))
    }
    pub fn len(&self) -> usize { self.jar.lock().map(|g| g.len()).unwrap_or(0) }
    pub fn clear(&self) { if let Ok(mut g) = self.jar.lock() { g.clear(); } }
    pub fn clear_host(&self, host: &str) { let r = reg_domain(host); if let Ok(mut g) = self.jar.lock() { g.retain(|c| reg_domain(&c.domain) != r); } }
    /// Tab-separated lines. `only_sites`: registrable domains to export (None = all).
    /// Session cookies get `now + session_ttl` so a login survives a restart.
    pub fn export(&self, only_sites: Option<&[&str]>, session_ttl: u64) -> String {
        let now = now_secs();
        let g = match self.jar.lock() { Ok(g) => g, Err(_) => return String::new() };
        g.iter().filter(|c| c.expires.map_or(true, |e| e > now))
            .filter(|c| only_sites.map_or(true, |s| s.iter().any(|d| reg_domain(d) == reg_domain(&c.domain))))
            .map(|c| format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", c.domain, c.host_only as u8, c.path, c.expires.unwrap_or(now + session_ttl), c.secure as u8, c.http_only as u8, c.same_site, c.name, c.value))
            .collect::<Vec<_>>().join("\n")
    }
    pub fn import(&self, text: &str) -> usize {
        let now = now_secs();
        let mut g = match self.jar.lock() { Ok(g) => g, Err(_) => return 0 };
        let mut n = 0;
        for l in text.lines() {
            let f: Vec<&str> = l.splitn(9, '\t').collect();
            if f.len() < 9 { continue; }
            let exp: u64 = match f[3].parse() { Ok(e) => e, Err(_) => continue };
            if exp <= now || g.len() >= MAX_COOKIES { continue; }
            let c = Cookie { domain: f[0].into(), host_only: f[1] == "1", path: f[2].into(), expires: Some(exp), secure: f[4] == "1", http_only: f[5] == "1", same_site: f[6].into(), name: f[7].into(), value: f[8].into() };
            g.retain(|o| !(o.name == c.name && o.domain == c.domain && o.path == c.path));
            g.push(c);
            n += 1;
        }
        n
    }
}
impl CookieStore for RamJar {
    fn set_cookies(&self, hs: &mut dyn Iterator<Item = &HeaderValue>, url: &url::Url) {
        let vals: Vec<String> = hs.filter_map(|h| h.to_str().ok().map(|s| s.to_string())).collect();
        self.store(&vals, url, page_host().as_deref(), config().block_third_party_cookies);
    }
    fn cookies(&self, url: &url::Url) -> Option<HeaderValue> {
        HeaderValue::from_str(&self.header_for(url, page_host().as_deref(), config().block_third_party_cookies)?).ok()
    }
}
fn jar() -> &'static Arc<RamJar> { static J: OnceLock<Arc<RamJar>> = OnceLock::new(); J.get_or_init(|| Arc::new(RamJar::new())) }
pub fn cookies_export(only_sites: Option<&[&str]>, session_ttl: u64) -> String { jar().export(only_sites, session_ttl) }
pub fn cookies_import(text: &str) -> usize { jar().import(text) }
pub fn cookies_clear() { jar().clear(); }
pub fn cookies_clear_host(host: &str) { jar().clear_host(host); }
pub fn cookie_count() -> usize { jar().len() }

// ---------------------------------------------------------------- HSTS
fn hsts() -> &'static Mutex<HashMap<String, (Instant, bool)>> { static H: OnceLock<Mutex<HashMap<String, (Instant, bool)>>> = OnceLock::new(); H.get_or_init(|| Mutex::new(HashMap::new())) }
fn hsts_note(u: &url::Url, header: &str) {
    let host = match u.host_str() { Some(h) => h.to_ascii_lowercase(), None => return };
    let h = header.to_ascii_lowercase();
    let age = h.split(';').find_map(|p| p.trim().strip_prefix("max-age=").and_then(|v| v.trim().trim_matches('"').parse::<u64>().ok())).unwrap_or(0);
    let sub = h.contains("includesubdomains");
    if let Ok(mut g) = hsts().lock() { if age == 0 { g.remove(&host); } else { g.insert(host, (Instant::now() + Duration::from_secs(age.min(31_536_000)), sub)); } }
}
fn hsts_known(u: &url::Url) -> bool {
    let host = match u.host_str() { Some(h) => h.to_ascii_lowercase(), None => return false };
    let g = match hsts().lock() { Ok(g) => g, Err(_) => return false };
    let now = Instant::now();
    let mut cur = host.as_str();
    let mut first = true;
    loop {
        if let Some((exp, sub)) = g.get(cur) { if *exp > now && (first || *sub) { return true; } }
        first = false;
        match cur.find('.') { Some(i) if cur[i + 1..].contains('.') => cur = &cur[i + 1..], _ => return false }
    }
}

// ---------------------------------------------------------------- RAM HTTP cache
struct CEntry { resp: Response, etag: Option<String>, last_mod: Option<String>, fresh_until: Instant, size: usize }
#[derive(Default)]
struct Cache { map: HashMap<String, CEntry>, order: VecDeque<String>, bytes: usize }
fn cache() -> &'static Mutex<Cache> { static C: OnceLock<Mutex<Cache>> = OnceLock::new(); C.get_or_init(|| Mutex::new(Cache::default())) }
enum Lookup { Fresh(Response), Stale(Option<String>, Option<String>), Miss }
pub fn cache_clear() { if let Ok(mut g) = cache().lock() { *g = Cache::default(); } }
pub fn cache_bytes() -> usize { cache().lock().map(|g| g.bytes).unwrap_or(0) }

fn parse_cc(cc: &str) -> (bool, bool, Option<u64>) {
    let l = cc.to_ascii_lowercase();
    let age = l.split(',').find_map(|p| p.trim().strip_prefix("max-age=").and_then(|v| v.trim().trim_matches('"').parse::<u64>().ok()));
    (l.contains("no-store"), l.contains("no-cache"), age)
}
fn cache_lookup(key: &str) -> Lookup {
    let mut g = match cache().lock() { Ok(g) => g, Err(_) => return Lookup::Miss };
    let found = g.map.get(key).map(|e| (e.resp.clone(), e.etag.clone(), e.last_mod.clone(), e.fresh_until));
    match found {
        None => Lookup::Miss,
        Some((r, et, lm, fresh)) => {
            if let Some(p) = g.order.iter().position(|k| k == key) { g.order.remove(p); }
            g.order.push_back(key.to_string());
            if Instant::now() < fresh { Lookup::Fresh(r) } else if et.is_some() || lm.is_some() { Lookup::Stale(et, lm) } else { Lookup::Miss }
        }
    }
}
fn cache_store(key: &str, resp: &Response, etag: Option<String>, last_mod: Option<String>, fresh: Duration) {
    let size = resp.body.len();
    if size == 0 || size > CACHE_ENTRY_MAX { return; }
    let mut g = match cache().lock() { Ok(g) => g, Err(_) => return };
    if let Some(old) = g.map.remove(key) { g.bytes -= old.size; if let Some(p) = g.order.iter().position(|k| k == key) { g.order.remove(p); } }
    while g.bytes + size > CACHE_MAX {
        match g.order.pop_front() { Some(k) => { if let Some(e) = g.map.remove(&k) { g.bytes -= e.size; } } None => break }
    }
    g.bytes += size;
    g.order.push_back(key.to_string());
    g.map.insert(key.to_string(), CEntry { resp: resp.clone(), etag, last_mod, fresh_until: Instant::now() + fresh, size });
}
fn cache_refresh(key: &str, cc: Option<&str>) -> Option<Response> {
    let mut g = cache().lock().ok()?;
    let e = g.map.get_mut(key)?;
    let (ns, nc, age) = cc.map(parse_cc).unwrap_or((false, false, None));
    e.fresh_until = Instant::now() + if ns || nc { Duration::ZERO } else { Duration::from_secs(age.unwrap_or(0)) };
    Some(e.resp.clone())
}

// ---------------------------------------------------------------- client
fn client() -> Result<&'static reqwest::blocking::Client, String> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut b = reqwest::blocking::Client::builder()
            .use_rustls_tls()
            .cookie_provider(jar().clone())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(20))
            .pool_idle_timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(6)
            .tcp_nodelay(true)
            .tcp_keepalive(Duration::from_secs(30))
            .referer(false)
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() >= MAX_REDIRECTS { a.error("too many redirects") }
                else if !matches!(a.url().scheme(), "http" | "https") { a.stop() }
                else { a.follow() }
            }));
        // direct connection by default; set SPARROW_USE_PROXY=1 to honour system proxy
        if std::env::var_os("SPARROW_USE_PROXY").is_none() { b = b.no_proxy(); }
        b.build().map_err(|e| e.to_string())
    }).as_ref().map_err(|e| e.clone())
}

// ---------------------------------------------------------------- requests
#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub timeout: Option<Duration>,
    pub max_bytes: u64,
    pub page: Option<String>,
    pub dest: &'static str,
    pub sub: bool,
    pub cache: bool,
    pub range: Option<(u64, Option<u64>)>,
    pub require_ok: bool,
}
impl Request {
    pub fn get(url: &str) -> Self {
        Request { method: "GET".into(), url: url.into(), headers: vec![], body: None, timeout: None, max_bytes: MAIN_LIMIT, page: None, dest: "document", sub: false, cache: false, range: None, require_ok: false }
    }
    pub fn sub(url: &str, page: &str) -> Self {
        Request { timeout: Some(Duration::from_secs(5)), max_bytes: SUB_LIMIT, page: Some(page.into()), dest: dest_for_url(url), sub: true, cache: true, require_ok: true, ..Request::get(url) }
    }
    pub fn post_form(url: &str, fields: &[(String, String)], page: Option<&str>) -> Self {
        let body = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(fields.iter().map(|(k, v)| (k.as_str(), v.as_str()))).finish();
        Request { method: "POST".into(), headers: vec![("Content-Type".into(), "application/x-www-form-urlencoded".into())], body: Some(body.into_bytes()), page: page.map(|p| p.into()), ..Request::get(url) }
    }
    pub fn post(url: &str, content_type: &str, body: Vec<u8>, page: Option<&str>) -> Self {
        Request { method: "POST".into(), headers: vec![("Content-Type".into(), content_type.into())], body: Some(body), page: page.map(|p| p.into()), ..Request::get(url) }
    }
}
fn dest_for_url(u: &str) -> &'static str {
    let p = u.split(|c| c == '?' || c == '#').next().unwrap_or("").to_ascii_lowercase();
    if p.ends_with(".css") { "style" }
    else if p.ends_with(".js") || p.ends_with(".mjs") { "script" }
    else if [".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".svg", ".ico", ".bmp"].iter().any(|e| p.ends_with(e)) { "image" }
    else { "empty" }
}
fn accept_for(dest: &str) -> &'static str {
    match dest {
        "document" => "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        "style" => "text/css,*/*;q=0.1",
        "image" => "image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
        _ => "*/*",
    }
}
fn origin_of(page: &str) -> Option<String> {
    let p = url::Url::parse(page).ok()?;
    if matches!(p.scheme(), "http" | "https") { Some(p.origin().ascii_serialization()) } else { None }
}
/// strict-origin-when-cross-origin
fn referrer_for(page: &str, target: &url::Url) -> Option<String> {
    let p = url::Url::parse(page).ok()?;
    if !matches!(p.scheme(), "http" | "https") || (p.scheme() == "https" && target.scheme() == "http") { return None; }
    if p.origin() == target.origin() {
        let mut q = p.clone();
        q.set_fragment(None);
        let _ = q.set_username("");
        let _ = q.set_password(None);
        Some(q.to_string())
    } else { Some(format!("{}/", p.origin().ascii_serialization())) }
}
fn site_relation(u: &url::Url, page: &str) -> &'static str {
    let p = match url::Url::parse(page) { Ok(p) => p, Err(_) => return "none" };
    if p.origin() == u.origin() { "same-origin" }
    else if matches!((p.host_str(), u.host_str()), (Some(a), Some(b)) if same_site(a, b)) { "same-site" }
    else { "cross-site" }
}

fn apply_headers(rq: reqwest::blocking::RequestBuilder, r: &Request, u: &url::Url, c: &Config, cond: &(Option<String>, Option<String>)) -> reqwest::blocking::RequestBuilder {
    let pg = r.page.as_deref().unwrap_or("");
    let mut rq = rq
        .header("User-Agent", c.user_agent.as_str())
        .header("Accept", accept_for(r.dest))
        .header("Accept-Language", c.accept_language.as_str())
        .header("Sec-Fetch-Dest", r.dest)
        .header("Sec-Fetch-Mode", if r.dest == "document" { "navigate" } else { "no-cors" })
        .header("Sec-Fetch-Site", site_relation(u, pg));
    if r.dest == "document" { rq = rq.header("Upgrade-Insecure-Requests", "1"); }
    if c.send_dnt { rq = rq.header("DNT", "1").header("Sec-GPC", "1"); }
    if let Some(rf) = referrer_for(pg, u) { rq = rq.header("Referer", rf); }
    if r.method == "POST" { if let Some(o) = origin_of(pg) { rq = rq.header("Origin", o); } }
    if let Some((a, b)) = r.range { rq = rq.header("Range", match b { Some(b) => format!("bytes={a}-{b}"), None => format!("bytes={a}-") }); }
    if let Some(e) = &cond.0 { rq = rq.header("If-None-Match", e.as_str()); }
    if let Some(l) = &cond.1 { rq = rq.header("If-Modified-Since", l.as_str()); }
    for (k, v) in &r.headers { rq = rq.header(k.as_str(), v.as_str()); }
    rq
}

fn exec(r: &Request, u: &url::Url, c: &Config) -> Result<Response, String> {
    let key = u.as_str().to_string();
    let cacheable = r.method == "GET" && r.cache && c.cache && r.range.is_none();
    let mut cond: (Option<String>, Option<String>) = (None, None);
    if cacheable {
        match cache_lookup(&key) {
            Lookup::Fresh(x) => { HITS.fetch_add(1, Relaxed); return Ok(x); }
            Lookup::Stale(e, l) => cond = (e, l),
            Lookup::Miss => {}
        }
    }
    let idem = matches!(r.method.as_str(), "GET" | "HEAD");
    let method = reqwest::Method::from_bytes(r.method.as_bytes()).map_err(|e| e.to_string())?;
    let mut attempt = 0u32;
    let resp = loop {
        let mut rq = apply_headers(client()?.request(method.clone(), u.clone()), r, u, c, &cond);
        if let Some(b) = &r.body { rq = rq.body(b.clone()); }
        if let Some(t) = r.timeout { rq = rq.timeout(t); }
        match rq.send() {
            Ok(x) => {
                if idem && attempt < 2 && matches!(x.status().as_u16(), 502 | 503 | 504) {
                    attempt += 1; RETRIES.fetch_add(1, Relaxed);
                    std::thread::sleep(Duration::from_millis(200 << attempt));
                    continue;
                }
                break x;
            }
            Err(e) => {
                if idem && attempt < 2 && (e.is_timeout() || e.is_connect()) {
                    attempt += 1; RETRIES.fetch_add(1, Relaxed);
                    std::thread::sleep(Duration::from_millis(200 << attempt));
                    continue;
                }
                return Err(e.to_string());
            }
        }
    };
    let status = resp.status().as_u16();
    let final_url = resp.url().clone();
    let h = |n: &str| resp.headers().get(n).and_then(|v| v.to_str().ok()).map(|s| s.to_string());
    let content_type = h("content-type").unwrap_or_default();
    let (cc, etag, lmod, vary, sts) = (h("cache-control"), h("etag"), h("last-modified"), h("vary"), h("strict-transport-security"));
    let clen = resp.content_length();
    if final_url.scheme() == "https" { if let Some(s) = &sts { hsts_note(&final_url, s); } }
    if status == 304 { if let Some(x) = cache_refresh(&key, cc.as_deref()) { HITS.fetch_add(1, Relaxed); return Ok(x); } }
    if r.require_ok && !(200..300).contains(&status) { return Err(format!("HTTP {status}")); }
    if clen.map_or(false, |n| n > r.max_bytes) { return Err("Response too large".into()); }
    let mut body = Vec::new();
    resp.take(r.max_bytes.saturating_add(1)).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() as u64 > r.max_bytes { return Err("Response too large".into()); }
    BYTES.fetch_add(body.len() as u64, Relaxed);
    let out = Response { url: final_url.to_string(), status, content_type, body };
    if cacheable && status == 200 {
        let (ns, nc, age) = cc.as_deref().map(parse_cc).unwrap_or((false, false, None));
        let vary_bad = vary.as_deref().map_or(false, |v| { let v = v.to_ascii_lowercase(); v.contains('*') || v.contains("cookie") });
        let fresh = if nc { Some(Duration::ZERO) } else { age.map(Duration::from_secs) };
        let fresh = fresh.or(if etag.is_some() || lmod.is_some() { Some(Duration::ZERO) } else { None });
        if let (false, false, Some(f)) = (ns, vary_bad, fresh) { cache_store(&key, &out, etag, lmod, f); }
    }
    Ok(out)
}

/// Full pipeline: data: URLs, blocklist, private-network guard, HSTS / HTTPS-first, cache, retries.
pub fn send(r: &Request) -> Result<Response, String> {
    REQS.fetch_add(1, Relaxed);
    if r.url.get(..5).map_or(false, |p| p.eq_ignore_ascii_case("data:")) { return data_url(&r.url, r.max_bytes); }
    let u = url::Url::parse(&r.url).map_err(|e| e.to_string())?;
    if !matches!(u.scheme(), "http" | "https") { return Err("Only http/https/data supported".into()); }
    let c = config();
    let pg = r.page.as_deref().unwrap_or("");
    if r.sub && !pg.is_empty() {
        if crate::dro_blocklist::global_blocked(u.as_str(), pg) { BLOCKED.fetch_add(1, Relaxed); return Err("Blocked by tracker protection".into()); }
        let page_private = url::Url::parse(pg).map_or(true, |p| is_private_url(&p));
        if c.block_private_subresources && !page_private && is_private_url(&u) { BLOCKED.fetch_add(1, Relaxed); return Err("Blocked: private network from public page".into()); }
    }
    let upgradable = u.scheme() == "http" && u.port().is_none() && !is_private_url(&u);
    if upgradable && (c.https_first || hsts_known(&u)) {
        let mut up = u.clone();
        if up.set_scheme("https").is_ok() {
            UPGRADES.fetch_add(1, Relaxed);
            let strict = hsts_known(&u) || (r.sub && url::Url::parse(pg).map_or(false, |p| p.scheme() == "https"));
            return match exec(r, &up, &c) { Ok(x) => Ok(x), Err(e) if strict => Err(e), Err(_) => exec(r, &u, &c) };
        }
    }
    exec(r, &u, &c)
}

// ---------------------------------------------------------------- old + convenient API
/// Main page: any status is returned (error pages are rendered).
pub fn fetch(url: &str) -> Result<Response, String> { fetch_max(url, MAIN_LIMIT) }
pub fn fetch_max(url: &str, max: u64) -> Result<Response, String> {
    set_page(url);
    let mut rq = Request::get(url);
    rq.max_bytes = max;
    let r = send(&rq)?;
    set_page(&r.url);
    Ok(r)
}
/// Byte range (media). `end` inclusive. Server may answer 206 or 200.
pub fn fetch_range(url: &str, start: u64, end: Option<u64>, max: u64) -> Result<Response, String> {
    let mut rq = Request::get(url);
    rq.range = Some((start, end));
    rq.max_bytes = max;
    rq.dest = "empty";
    send(&rq)
}
/// Subresource (CSS/img/js): short timeout, 2xx only, cached, tracker-blocked.
pub fn fetch_sub(url: &str) -> Result<Response, String> { fetch_sub_page(url, &page()) }
pub fn fetch_sub_page(url: &str, page: &str) -> Result<Response, String> { send(&Request::sub(url, page)) }
pub fn post_form(url: &str, fields: &[(String, String)], page: Option<&str>) -> Result<Response, String> {
    let r = send(&Request::post_form(url, fields, page))?;
    set_page(&r.url);
    Ok(r)
}

/// Parallel subresource fetch (order preserved). Uses the current page for blocking/referrer.
pub fn par_map<T: Send>(urls: &[String], f: impl Fn(Response) -> Option<T> + Sync) -> Vec<Option<T>> { par_map_page(urls, &page(), f) }
pub fn par_map_page<T: Send>(urls: &[String], page: &str, f: impl Fn(Response) -> Option<T> + Sync) -> Vec<Option<T>> {
    let n = urls.len();
    if n == 0 { return vec![]; }
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..n.min(MAX_PAR) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Relaxed);
                if i >= n { break; }
                let v = fetch_sub_page(&urls[i], page).ok().and_then(|r| f(r));
                if let Ok(mut g) = slots[i].lock() { *g = v; }
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().ok().flatten()).collect()
}

/// Forget everything held in RAM (call on exit or "clear browsing data").
pub fn clear_all() { cache_clear(); cookies_clear(); if let Ok(mut g) = hsts().lock() { g.clear(); } }

// ---------------------------------------------------------------- data: URLs, sniffing, decoding
fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let (mut out, mut acc, mut bits) = (Vec::with_capacity(s.len() * 3 / 4), 0u32, 0u32);
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A', b'a'..=b'z' => b - b'a' + 26, b'0'..=b'9' => b - b'0' + 52,
            b'+' | b'-' => 62, b'/' | b'_' => 63,
            b'=' | b' ' | b'\n' | b'\r' | b'\t' => continue,
            _ => return Err("bad base64".into()),
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 { bits -= 8; out.push((acc >> bits) as u8); acc &= (1u32 << bits) - 1; }
    }
    Ok(out)
}
fn pct_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let (mut out, mut i) = (Vec::with_capacity(b.len()), 0);
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) { out.push(v); i += 3; continue; }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}
fn data_url(s: &str, max: u64) -> Result<Response, String> {
    let (meta, data) = s[5..].split_once(',').ok_or("bad data URL")?;
    let b64 = meta.to_ascii_lowercase().ends_with(";base64");
    let ct = if b64 { &meta[..meta.len() - 7] } else { meta };
    let ct = if ct.is_empty() { "text/plain;charset=US-ASCII" } else { ct };
    let body = if b64 { b64_decode(data)? } else { pct_decode(data) };
    if body.len() as u64 > max { return Err("Response too large".into()); }
    Ok(Response { url: "data:".into(), status: 200, content_type: ct.into(), body })
}

/// Fix wrong/missing Content-Type by magic bytes.
pub fn sniff_mime(body: &[u8], declared: &str) -> String {
    let d = declared.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if !(d.is_empty() || d == "text/plain" || d == "application/octet-stream" || d == "unknown/unknown") { return d; }
    let h = &body[..body.len().min(512)];
    let st = |p: &[u8]| h.starts_with(p);
    let mime: &str = if st(b"\x89PNG\r\n\x1a\n") { "image/png" } else if st(&[0xFF, 0xD8, 0xFF]) { "image/jpeg" } else if st(b"GIF87a") || st(b"GIF89a") { "image/gif" }
    else if h.len() > 12 && &h[..4] == b"RIFF" && &h[8..12] == b"WEBP" { "image/webp" } else if st(b"%PDF-") { "application/pdf" }
    else {
        let t = String::from_utf8_lossy(h).to_ascii_lowercase();
        let t = t.trim_start();
        if t.starts_with("<!doctype html") || t.starts_with("<html") { "text/html" } else if t.starts_with("<svg") || t.starts_with("<?xml") { "image/svg+xml" } else if d.is_empty() { "application/octet-stream" } else { "text/plain" }
    };
    mime.to_string()
}

/// Decode using BOM, Content-Type charset, then <meta charset> sniff, else UTF-8.
pub fn decode(body: &[u8], content_type: &str) -> String {
    if let Some((enc, n)) = encoding_rs::Encoding::for_bom(body) { return enc.decode_without_bom_handling(&body[n..]).0.into_owned(); }
    let label = content_type.to_ascii_lowercase().split("charset=").nth(1).map(|c| c.trim().trim_matches(|ch| ch == '"' || ch == '\'').split(';').next().unwrap_or("").trim().to_string());
    let sniff = || {
        let head = String::from_utf8_lossy(&body[..body.len().min(1024)]).to_ascii_lowercase();
        let rest = head.split("charset=").nth(1)?;
        Some(rest.trim_start_matches(|c| c == '"' || c == '\'').chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>())
    };
    let enc = label.or_else(sniff).and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes())).unwrap_or(encoding_rs::UTF_8);
    enc.decode(body).0.into_owned()
}

pub fn resolve(base: &str, href: &str) -> Option<String> {
    url::Url::parse(base).ok()?.join(href).ok().map(|u| u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn u(s: &str) -> url::Url { url::Url::parse(s).unwrap() }

    #[test] fn http_dates() {
        assert_eq!(parse_http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(parse_http_date("Sun, 06 Nov 1994 08:49:37 GMT"), Some(784111777));
        assert_eq!(parse_http_date("Sunday, 06-Nov-94 08:49:37 GMT"), Some(784111777));
        assert!(parse_http_date("garbage").is_none());
    }
    #[test] fn cookie_basic_and_security() {
        let j = RamJar::new();
        j.store(&["sid=abc; Path=/; Secure; HttpOnly".into(), "pref=1; Domain=example.com; Path=/".into()], &u("https://www.example.com/a/b"), None, false);
        assert_eq!(j.header_for(&u("https://www.example.com/x"), None, false).unwrap(), "sid=abc; pref=1");
        let over_http = j.header_for(&u("http://www.example.com/x"), None, false).unwrap();
        assert_eq!(over_http, "pref=1");                       // Secure cookie not sent over http
        assert!(j.header_for(&u("https://sub.example.com/"), None, false).unwrap().contains("pref=1"));
        assert!(j.header_for(&u("https://evil.com/"), None, false).is_none());
    }
    #[test] fn cookie_rejections_and_delete() {
        let j = RamJar::new();
        j.store(&["a=1; Domain=com".into(), "b=1; Domain=other.com".into(), "__Host-x=1; Path=/".into(), "s=1; Secure".into()], &u("http://www.example.com/"), None, false);
        assert_eq!(j.len(), 0);
        j.store(&["k=v".into()], &u("https://example.com/"), None, false);
        assert_eq!(j.len(), 1);
        j.store(&["k=; Max-Age=0".into()], &u("https://example.com/"), None, false);
        assert_eq!(j.len(), 0);
    }
    #[test] fn third_party_cookie_blocking() {
        let j = RamJar::new();
        j.store(&["t=1".into()], &u("https://tracker.net/"), Some("news.com"), true);
        assert_eq!(j.len(), 0);
        j.store(&["t=1".into()], &u("https://cdn.news.com/"), Some("news.com"), true);
        assert_eq!(j.len(), 1);
    }
    #[test] fn cookie_export_import_roundtrip() {
        let a = RamJar::new();
        a.store(&["sid=abc; Path=/".into()], &u("https://example.com/"), None, false);
        a.store(&["x=1; Path=/".into()], &u("https://other.org/"), None, false);
        let txt = a.export(Some(&["example.com"][..]), 3600);
        let b = RamJar::new();
        assert_eq!(b.import(&txt), 1);
        assert_eq!(b.header_for(&u("https://example.com/"), None, false).unwrap(), "sid=abc");
    }
    #[test] fn domains() {
        assert_eq!(reg_domain("a.b.example.co.uk"), "example.co.uk");
        assert_eq!(reg_domain("www.example.com"), "example.com");
        assert!(same_site("a.example.com", "b.example.com"));
        assert!(!same_site("example.com", "example.org"));
    }
    #[test] fn private_hosts() {
        for s in ["http://localhost/", "http://127.0.0.1/", "http://192.168.1.5/", "http://10.0.0.1/", "http://[::1]/", "http://printer.local/"] { assert!(is_private_url(&u(s)), "{s}"); }
        assert!(!is_private_url(&u("https://example.com/")));
    }
    #[test] fn data_urls_and_base64() {
        assert_eq!(b64_decode("SGVsbG8=").unwrap(), b"Hello");
        let r = send(&Request::get("data:text/plain;base64,SGVsbG8=")).unwrap();
        assert_eq!(r.body, b"Hello");
        assert_eq!(send(&Request::get("data:,a%20b")).unwrap().body, b"a b");
    }
    #[test] fn par_map_keeps_order() {
        let urls: Vec<String> = (0..20).map(|i| format!("data:text/plain,n{i}")).collect();
        let out = par_map_page(&urls, "", |r| Some(String::from_utf8_lossy(&r.body).to_string()));
        for (i, o) in out.iter().enumerate() { assert_eq!(o.as_deref(), Some(format!("n{i}").as_str())); }
        assert!(par_map_page(&[], "", |_r| Some(1)).is_empty());
    }
    #[test] fn cache_fresh_and_stale() {
        let r = Response { url: "https://t.test/a.css".into(), status: 200, content_type: "text/css".into(), body: b"x{}".to_vec() };
        cache_store("https://t.test/a.css", &r, Some("\"e1\"".into()), None, Duration::from_secs(60));
        assert!(matches!(cache_lookup("https://t.test/a.css"), Lookup::Fresh(_)));
        cache_store("https://t.test/b.css", &r, Some("\"e2\"".into()), None, Duration::ZERO);
        assert!(matches!(cache_lookup("https://t.test/b.css"), Lookup::Stale(Some(_), None)));
        assert!(matches!(cache_lookup("https://t.test/none"), Lookup::Miss));
        assert_eq!(parse_cc("Public, max-age=300"), (false, false, Some(300)));
        assert_eq!(parse_cc("no-store"), (true, false, None));
    }
    #[test] fn hsts_rules() {
        hsts_note(&u("https://secure.test/"), "max-age=1000; includeSubDomains");
        assert!(hsts_known(&u("http://secure.test/")));
        assert!(hsts_known(&u("http://a.secure.test/")));
        assert!(!hsts_known(&u("http://other.test/")));
    }
    #[test] fn referrer_policy() {
        let t = u("https://other.com/x");
        assert_eq!(referrer_for("https://a.com/p?q=1#frag", &t).unwrap(), "https://a.com/");
        assert_eq!(referrer_for("https://a.com/p?q=1#frag", &u("https://a.com/z")).unwrap(), "https://a.com/p?q=1");
        assert!(referrer_for("https://a.com/", &u("http://a.com/")).is_none());
        assert_eq!(site_relation(&u("https://cdn.a.com/x"), "https://a.com/"), "same-site");
    }
    #[test] fn sniff_and_decode() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\nrest", ""), "image/png");
        assert_eq!(sniff_mime(b"<!DOCTYPE html><p>", "text/plain"), "text/html");
        assert_eq!(sniff_mime(b"abc", "image/png"), "image/png");
        assert_eq!(decode(b"\xEF\xBB\xBFhi", ""), "hi");
        assert_eq!(decode(&[0xE9], "text/html; charset=iso-8859-1"), "é");
        assert_eq!(resolve("https://a.com/x/y", "../z").unwrap(), "https://a.com/z");
    }
}
