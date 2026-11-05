/// Search engine (not Google/MS). Change if you want.
pub const SEARCH: &str = "https://duckduckgo.com/?q=";

pub fn resolve(input: &str) -> String {
    let s = input.trim();
    if s.is_empty() { return "about:blank".into(); }
    let l = s.to_ascii_lowercase();
    if l.starts_with("http://") || l.starts_with("https://") || l.starts_with("about:") { return s.to_string(); }
    let host = s.split('/').next().unwrap_or("");
    let looks_like_host = !s.contains(' ') && (host.contains('.') || l.starts_with("localhost"));
    if looks_like_host { format!("https://{s}") } else {
        format!("{SEARCH}{}", url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>())
    }
}
