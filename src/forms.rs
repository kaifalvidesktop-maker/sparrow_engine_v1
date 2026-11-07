//! HTML form submission: collect successful controls (HTML spec 4.10.21), urlencode, build GET/POST request.
//! Pure logic over `Dom`; the network call itself is done by `dro_net` (needs a POST helper: see FormRequest).
use crate::dro_dom::{Dom, NodeId};
use crate::dro_net::resolve;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method { Get, Post }

#[derive(Clone, Debug, PartialEq)]
pub struct FormRequest {
    pub method: Method,
    pub url: String,                       // GET: includes the query string
    pub body: Option<String>,              // POST only
    pub content_type: Option<&'static str>,
}

/// application/x-www-form-urlencoded byte serializer.
pub fn urlencode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => o.push(b as char),
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{:02X}", b)),
        }
    }
    o
}

pub fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < b.len() => {
                let h = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|x| u8::from_str_radix(x, 16).ok());
                match h { Some(v) => { out.push(v); i += 3; } None => { out.push(b'%'); i += 1; } }
            }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn encode_pairs(p: &[(String, String)]) -> String {
    p.iter().map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v))).collect::<Vec<_>>().join("&")
}

fn crlf(s: &str) -> String { s.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n") }

fn collapse(s: &str) -> String { s.split_whitespace().collect::<Vec<_>>().join(" ") }

/// Closest ancestor `<form>` of `node` (or the node itself).
pub fn find_form(dom: &Dom, node: NodeId) -> Option<NodeId> {
    let mut n = Some(node);
    while let Some(i) = n {
        if dom.tag(i) == Some("form") { return Some(i); }
        n = dom.nodes[i].parent;
    }
    None
}

fn option_value(dom: &Dom, o: NodeId) -> String {
    dom.attr(o, "value").map(|v| v.to_string()).unwrap_or_else(|| collapse(&dom.text_content(o)))
}

/// Successful controls of `form` in tree order. `submitter` is the button that was clicked (if any).
pub fn collect(dom: &Dom, form: NodeId, submitter: Option<NodeId>) -> Vec<(String, String)> {
    let form_id = dom.attr(form, "id").map(|s| s.to_string());
    let mut owned: Vec<NodeId> = dom.descendants(form);
    if let Some(fid) = &form_id {
        // controls outside the <form> that point to it with form="id"
        for n in dom.descendants(0) {
            if !owned.contains(&n) && dom.attr(n, "form") == Some(fid.as_str()) { owned.push(n); }
        }
    }
    let mut out = vec![];
    for n in owned {
        let Some(tag) = dom.tag(n) else { continue };
        if let Some(fa) = dom.attr(n, "form") { if Some(fa) != form_id.as_deref() { continue; } }
        if !matches!(tag, "input" | "select" | "textarea" | "button") || dom.attr(n, "disabled").is_some() { continue; }
        // skip controls inside <datalist>
        let mut p = dom.nodes[n].parent;
        let mut in_datalist = false;
        while let Some(a) = p { if dom.tag(a) == Some("datalist") { in_datalist = true; break; } p = dom.nodes[a].parent; }
        if in_datalist { continue; }
        let name = dom.attr(n, "name").unwrap_or("").to_string();
        let val = dom.attr(n, "value");
        match tag {
            "input" => {
                let ty = dom.attr(n, "type").unwrap_or("text").to_ascii_lowercase();
                match ty.as_str() {
                    "checkbox" | "radio" => { if !name.is_empty() && dom.attr(n, "checked").is_some() { out.push((name, val.unwrap_or("on").to_string())); } }
                    "submit" | "image" => { if !name.is_empty() && Some(n) == submitter { out.push((name, val.unwrap_or("").to_string())); } }
                    "button" | "reset" | "file" => {}
                    _ => { if !name.is_empty() { out.push((name, val.unwrap_or("").to_string())); } }
                }
            }
            "button" => {
                let ty = dom.attr(n, "type").unwrap_or("submit").to_ascii_lowercase();
                if ty == "submit" && !name.is_empty() && Some(n) == submitter { out.push((name, val.unwrap_or("").to_string())); }
            }
            "textarea" => {
                if !name.is_empty() {
                    let t = dom.text_content(n);
                    out.push((name, crlf(t.strip_prefix('\n').unwrap_or(&t))));
                }
            }
            "select" => {
                if name.is_empty() { continue; }
                let opts: Vec<NodeId> = dom.descendants(n).into_iter().filter(|&o| dom.tag(o) == Some("option") && dom.attr(o, "disabled").is_none()).collect();
                let mut picked: Vec<NodeId> = opts.iter().copied().filter(|&o| dom.attr(o, "selected").is_some()).collect();
                if picked.is_empty() && dom.attr(n, "multiple").is_none() { if let Some(&f) = opts.first() { picked.push(f); } }
                if dom.attr(n, "multiple").is_none() { picked.truncate(1); }
                for o in picked { out.push((name.clone(), option_value(dom, o))); }
            }
            _ => {}
        }
    }
    out
}

/// Build the request for submitting `form` (optionally via `submitter`) on a page at `base`.
pub fn build_request(dom: &Dom, form: NodeId, submitter: Option<NodeId>, base: &str) -> Result<FormRequest, String> {
    let sub = |a: &str| submitter.and_then(|s| dom.attr(s, a));
    let action = sub("formaction").or_else(|| dom.attr(form, "action")).map(|s| s.trim()).unwrap_or("");
    let method = sub("formmethod").or_else(|| dom.attr(form, "method")).unwrap_or("get").to_ascii_lowercase();
    let enctype = sub("formenctype").or_else(|| dom.attr(form, "enctype")).unwrap_or("application/x-www-form-urlencoded").to_ascii_lowercase();
    if enctype.contains("multipart") { return Err("multipart/form-data forms are not supported yet".into()); }
    let target = if action.is_empty() { base.to_string() } else { resolve(base, action).ok_or("bad form action URL")? };
    if !(target.starts_with("http://") || target.starts_with("https://")) { return Err(format!("unsupported form action: {target}")); }
    let body = encode_pairs(&collect(dom, form, submitter));
    if method == "post" {
        Ok(FormRequest { method: Method::Post, url: target, body: Some(body), content_type: Some("application/x-www-form-urlencoded") })
    } else {
        let no_frag = target.split('#').next().unwrap_or("");
        let path = no_frag.split('?').next().unwrap_or("");
        let url = if body.is_empty() { path.to_string() } else { format!("{path}?{body}") };
        Ok(FormRequest { method: Method::Get, url, body: None, content_type: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dro_dom::NodeKind;

    fn el(d: &mut Dom, parent: NodeId, tag: &str, attrs: &[(&str, &str)]) -> NodeId {
        let n = d.create(NodeKind::Element { tag: tag.into(), attrs: attrs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() });
        d.append_child(parent, n);
        n
    }
    fn text(d: &mut Dom, parent: NodeId, t: &str) { let n = d.create(NodeKind::Text(t.into())); d.append_child(parent, n); }

    #[test]
    fn encoding_roundtrip() {
        assert_eq!(urlencode("a b&c=d/é"), "a+b%26c%3Dd%2F%C3%A9");
        assert_eq!(urldecode("a+b%26c%3Dd%2F%C3%A9"), "a b&c=d/é");
        assert_eq!(urldecode("100%"), "100%");
        assert_eq!(urldecode("%zz%4"), "%zz%4");
        assert_eq!(urldecode(&urlencode("বাংলা ভাষা")), "বাংলা ভাষা");
    }

    fn sample() -> (Dom, NodeId, NodeId) {
        let mut d = Dom::new();
        let f = el(&mut d, 0, "form", &[("action", "/search?old=1#x"), ("id", "f1")]);
        el(&mut d, f, "input", &[("name", "q"), ("value", "rust lang")]);
        el(&mut d, f, "input", &[("type", "hidden"), ("name", "h"), ("value", "1")]);
        el(&mut d, f, "input", &[("type", "checkbox"), ("name", "c1"), ("checked", "")]);
        el(&mut d, f, "input", &[("type", "checkbox"), ("name", "c2")]);
        el(&mut d, f, "input", &[("type", "radio"), ("name", "r"), ("value", "b"), ("checked", "")]);
        el(&mut d, f, "input", &[("name", "off"), ("value", "x"), ("disabled", "")]);
        let s = el(&mut d, f, "select", &[("name", "sel")]);
        let o1 = el(&mut d, s, "option", &[]); text(&mut d, o1, " One  1 ");
        let o2 = el(&mut d, s, "option", &[("value", "two"), ("selected", "")]); text(&mut d, o2, "Two");
        let t = el(&mut d, f, "textarea", &[("name", "msg")]); text(&mut d, t, "\nline1\nline2");
        let b = el(&mut d, f, "button", &[("name", "go"), ("value", "now")]);
        el(&mut d, 0, "input", &[("name", "outside"), ("form", "f1"), ("value", "yes")]);
        (d, f, b)
    }

    #[test]
    fn collects_successful_controls_in_order() {
        let (d, f, b) = sample();
        let got = collect(&d, f, Some(b));
        let names: Vec<&str> = got.iter().map(|x| x.0.as_str()).collect();
        assert_eq!(names, vec!["q", "h", "c1", "r", "sel", "msg", "go", "outside"]);
        assert_eq!(got[2].1, "on");
        assert_eq!(got[4].1, "two");
        assert_eq!(got[5].1, "line1\r\nline2");
        assert!(collect(&d, f, None).iter().all(|x| x.0 != "go")); // no submitter -> button omitted
    }

    #[test]
    fn select_defaults_to_first_option() {
        let mut d = Dom::new();
        let f = el(&mut d, 0, "form", &[]);
        let s = el(&mut d, f, "select", &[("name", "s")]);
        let o = el(&mut d, s, "option", &[]); text(&mut d, o, " A  b ");
        assert_eq!(collect(&d, f, None), vec![("s".to_string(), "A b".to_string())]);
    }

    #[test]
    fn get_replaces_query_and_drops_fragment() {
        let (d, f, b) = sample();
        let r = build_request(&d, f, Some(b), "https://ex.com/page").expect("req");
        assert_eq!(r.method, Method::Get);
        assert!(r.url.starts_with("https://ex.com/search?q=rust+lang&h=1&c1=on&r=b&sel=two&msg=line1%0D%0Aline2&go=now"), "{}", r.url);
        assert!(!r.url.contains("old=1") && !r.url.contains('#'));
    }

    #[test]
    fn post_and_overrides_and_errors() {
        let mut d = Dom::new();
        let f = el(&mut d, 0, "form", &[("method", "post"), ("action", "https://a.org/p")]);
        el(&mut d, f, "input", &[("name", "k"), ("value", "v w")]);
        let r = build_request(&d, f, None, "https://ex.com/").expect("req");
        assert_eq!((r.method, r.body.as_deref(), r.content_type), (Method::Post, Some("k=v+w"), Some("application/x-www-form-urlencoded")));
        let b = el(&mut d, f, "button", &[("formmethod", "get"), ("formaction", "https://b.org/g")]);
        assert_eq!(build_request(&d, f, Some(b), "https://ex.com/").expect("req").url, "https://b.org/g?k=v+w");
        let mp = el(&mut d, 0, "form", &[("enctype", "multipart/form-data")]);
        assert!(build_request(&d, mp, None, "https://ex.com/").is_err());
        let js = el(&mut d, 0, "form", &[("action", "javascript:alert(1)")]);
        assert!(build_request(&d, js, None, "https://ex.com/").is_err());
    }

    #[test]
    fn empty_action_uses_page_url_and_find_form() {
        let mut d = Dom::new();
        let f = el(&mut d, 0, "form", &[]);
        let div = el(&mut d, f, "div", &[]);
        let i = el(&mut d, div, "input", &[("name", "a"), ("value", "1")]);
        assert_eq!(find_form(&d, i), Some(f));
        assert_eq!(build_request(&d, f, None, "https://ex.com/p?z=9").expect("r").url, "https://ex.com/p?a=1");
        assert_eq!(find_form(&d, 0), None);
    }
}
