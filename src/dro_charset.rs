//! Charset detection + decoding without extra crates.
//! Supports: UTF-8 (+BOM), UTF-16 LE/BE (BOM), ISO-8859-1, windows-1252, ASCII.
//! Unknown labels (gbk, shift_jis, ...) fall back to lossy UTF-8.

const CP1252: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x8D, 0x017D, 0x8F,
    0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

/// Extract `charset=xxx` from a Content-Type header value.
pub fn charset_from_header(ct: &str) -> Option<String> {
    let l = ct.to_ascii_lowercase();
    let i = l.find("charset=")?;
    let v = l[i + 8..].split(|c: char| c == ';' || c.is_whitespace()).next()?;
    let v = v.trim_matches(|c| c == '"' || c == '\'');
    if v.is_empty() { None } else { Some(v.to_string()) }
}

/// Look for `<meta charset=..>` / `<meta http-equiv content="..charset=..">` in the first 2 KB.
pub fn charset_from_meta(body: &[u8]) -> Option<String> {
    let head: String = body.iter().take(2048).map(|&b| if b < 128 { (b as char).to_ascii_lowercase() } else { ' ' }).collect();
    let mut from = 0;
    while let Some(p) = head[from..].find("charset") {
        let s = from + p + 7;
        let rest = head[s..].trim_start();
        if let Some(r) = rest.strip_prefix('=') {
            let r = r.trim_start().trim_start_matches(|c| c == '"' || c == '\'');
            let v: String = r.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
            if !v.is_empty() { return Some(v); }
        }
        from = s;
    }
    None
}

fn latin1(body: &[u8], cp1252: bool) -> String {
    body.iter()
        .map(|&b| {
            if cp1252 && (0x80..0xA0).contains(&b) {
                char::from_u32(CP1252[(b - 0x80) as usize] as u32).unwrap_or('\u{FFFD}')
            } else {
                b as char
            }
        })
        .collect()
}

fn utf16(body: &[u8], le: bool) -> String {
    let it = body.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) });
    char::decode_utf16(it).map(|r| r.unwrap_or('\u{FFFD}')).collect()
}

pub fn decode(body: &[u8], content_type: &str) -> String {
    if body.starts_with(&[0xEF, 0xBB, 0xBF]) { return String::from_utf8_lossy(&body[3..]).into_owned(); }
    if body.starts_with(&[0xFF, 0xFE]) { return utf16(&body[2..], true); }
    if body.starts_with(&[0xFE, 0xFF]) { return utf16(&body[2..], false); }
    let label = charset_from_header(content_type).or_else(|| charset_from_meta(body)).unwrap_or_else(|| "utf-8".into());
    match label.as_str() {
        "iso-8859-1" | "latin1" | "l1" | "iso8859-1" => latin1(body, false),
        "windows-1252" | "cp1252" | "us-ascii" | "ascii" | "iso-8859-15" => latin1(body, true),
        "utf-16" | "utf-16le" => utf16(body, true),
        "utf-16be" => utf16(body, false),
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn header_charset() {
        assert_eq!(charset_from_header("text/html; charset=\"ISO-8859-1\""), Some("iso-8859-1".into()));
        assert_eq!(charset_from_header("text/html"), None);
    }
    #[test]
    fn meta_charset() {
        assert_eq!(charset_from_meta(b"<html><meta charset=windows-1252>"), Some("windows-1252".into()));
        assert_eq!(charset_from_meta(b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\">"), Some("utf-8".into()));
    }
    #[test]
    fn decode_latin1_and_cp1252() {
        assert_eq!(decode(&[0x63, 0x61, 0x66, 0xE9], "text/html; charset=iso-8859-1"), "café");
        assert_eq!(decode(&[0x80, 0x93], "text/html; charset=windows-1252"), "€“");
    }
    #[test]
    fn decode_bom_and_utf8() {
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'a'], ""), "a");
        assert_eq!(decode(&[0xFF, 0xFE, b'h', 0, b'i', 0], ""), "hi");
        assert_eq!(decode("বাংলা".as_bytes(), "text/html"), "বাংলা");
    }
}
