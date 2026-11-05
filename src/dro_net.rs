use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

const MAIN_LIMIT: u64 = 20 * 1024 * 1024;
const SUB_LIMIT: u64 = 5 * 1024 * 1024;

pub struct Response {
    pub url: String,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();

fn client() -> Result<&'static reqwest::blocking::Client, String> {
    CLIENT
        .get_or_init(|| {
            let mut b = reqwest::blocking::Client::builder().use_rustls_tls().timeout(Duration::from_secs(20)).user_agent("Sparrow/0.1");
            // direct connection by default; set SPARROW_USE_PROXY=1 to honour system proxy
            if std::env::var_os("SPARROW_USE_PROXY").is_none() { b = b.no_proxy(); }
            b.build().map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| e.clone())
}

fn get(url: &str, timeout: Option<Duration>, limit: u64) -> Result<Response, String> {
    let u = url::Url::parse(url).map_err(|e| e.to_string())?;
    if !matches!(u.scheme(), "http" | "https") { return Err("Only http/https supported".into()); }
    let mut rq = client()?.get(u);
    if let Some(t) = timeout { rq = rq.timeout(t); }
    let resp = rq.send().map_err(|e| e.to_string())?;
    if resp.content_length().map_or(false, |n| n > limit) { return Err("Response too large".into()); }
    let status = resp.status().as_u16();
    let final_url = resp.url().to_string();
    let content_type = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut body = Vec::new();
    resp.take(limit + 1).read_to_end(&mut body).map_err(|e| e.to_string())?;
    if body.len() as u64 > limit { return Err("Response too large".into()); }
    Ok(Response { url: final_url, status, content_type, body })
}

/// Main page: any status is returned (error pages are rendered).
pub fn fetch(url: &str) -> Result<Response, String> { get(url, None, MAIN_LIMIT) }

/// Subresource (CSS): short timeout, 2xx only.
pub fn fetch_sub(url: &str) -> Result<Response, String> {
    let r = get(url, Some(Duration::from_secs(5)), SUB_LIMIT)?;
    if !(200..300).contains(&r.status) { return Err(format!("HTTP {}", r.status)); }
    Ok(r)
}

/// Decode using BOM, Content-Type charset, then <meta charset> sniff, else UTF-8.
pub fn decode(body: &[u8], content_type: &str) -> String {
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
