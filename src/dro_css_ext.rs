//! CSS pre-processor: turns modern CSS into the subset `dro_css::parse` understands.
//! Handles: @media (min/max-width/height, orientation, screen/print), @supports (assumed true),
//! `:root` -> `html`, custom properties `var(--x, fallback)`, `vw/vh/vmin/vmax` units, `calc()`.
//! Pure text -> text, no dependencies. Call as `dro_css::parse(&preprocess(css, vw, vh))`.

pub fn preprocess(css: &str, vw: f32, vh: f32) -> String {
    let flat = flatten(&strip_comments(css), vw, vh, 0);
    let flat = flat.replace(":root", "html");
    let flat = resolve_vars(&flat);
    let flat = convert_units(&flat, vw, vh);
    eval_calc(&flat)
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        rest = match rest[i + 2..].find("*/") { Some(j) => &rest[i + 2 + j + 2..], None => "" };
    }
    out.push_str(rest);
    out
}

/// Index of the `}` matching the `{` at `open` (byte index of '{'). Returns len if unbalanced.
fn block_end(s: &str, open: usize) -> usize {
    let mut d = 0i32;
    for (i, b) in s.bytes().enumerate().skip(open) {
        match b { b'{' => d += 1, b'}' => { d -= 1; if d == 0 { return i; } } _ => {} }
    }
    s.len().saturating_sub(1)
}

fn flatten(css: &str, vw: f32, vh: f32, depth: u32) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < css.len() {
        let rest = &css[i..];
        let Some(at) = rest.find('@') else { out.push_str(rest); break };
        // text before the @-rule may contain complete rules; but a '{' before '@' means the @ is nested, handled by recursion
        let brace = rest.find('{');
        if let Some(b) = brace { if b < at { // normal rule first: copy through its closing brace
            let e = block_end(css, i + b);
            out.push_str(&css[i..=e.min(css.len() - 1)]);
            i = e + 1;
            continue;
        } }
        out.push_str(&rest[..at]);
        let a = i + at;
        let after = &css[a..];
        let semi = after.find(';');
        let open = after.find('{');
        match (open, semi) {
            (Some(o), s) if s.map_or(true, |s| o < s) => {
                let name = after[1..o].split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                let cond = after[1 + name.len()..o].trim();
                let end = block_end(css, a + o);
                let inner = &css[a + o + 1..end.max(a + o + 1)];
                match name.as_str() {
                    "media" if depth < 8 => if media_matches(cond, vw, vh) { out.push_str(&flatten(inner, vw, vh, depth + 1)); },
                    "supports" | "layer" | "container" if depth < 8 => out.push_str(&flatten(inner, vw, vh, depth + 1)),
                    _ => {} // @font-face, @keyframes, @page, ... dropped
                }
                i = end + 1;
            }
            (_, Some(s)) => i = a + s + 1, // @import / @charset / @namespace
            _ => break,
        }
    }
    out
}

fn px_of(v: &str) -> Option<f32> {
    let v = v.trim();
    if let Some(x) = v.strip_suffix("px") { return x.trim().parse().ok(); }
    if let Some(x) = v.strip_suffix("rem") { return x.trim().parse::<f32>().ok().map(|n| n * 16.0); }
    if let Some(x) = v.strip_suffix("em") { return x.trim().parse::<f32>().ok().map(|n| n * 16.0); }
    v.parse().ok()
}

pub fn media_matches(q: &str, vw: f32, vh: f32) -> bool {
    let q = q.to_ascii_lowercase();
    q.split(',').any(|one| {
        let one = one.trim();
        let (neg, one) = match one.strip_prefix("not ") { Some(r) => (true, r), None => (false, one) };
        let one = one.strip_prefix("only ").unwrap_or(one);
        let ok = one.split(" and ").all(|p| {
            let p = p.trim();
            match p {
                "" | "screen" | "all" => return true,
                "print" | "speech" => return false,
                _ => {}
            }
            let inner = p.trim_start_matches('(').trim_end_matches(')');
            let (k, v) = match inner.split_once(':') { Some((k, v)) => (k.trim(), v.trim()), None => return false };
            match k {
                "min-width" => px_of(v).map_or(false, |x| vw >= x),
                "max-width" => px_of(v).map_or(false, |x| vw <= x),
                "min-height" => px_of(v).map_or(false, |x| vh >= x),
                "max-height" => px_of(v).map_or(false, |x| vh <= x),
                "orientation" => (v == "landscape") == (vw >= vh),
                "hover" | "pointer" => v == "hover" || v == "fine",
                "prefers-color-scheme" => v == "light",
                _ => false,
            }
        });
        ok != neg
    })
}

/// Collect `--name: value` declarations (last wins) and substitute `var(--name[, fallback])`.
fn resolve_vars(css: &str) -> String {
    if !css.contains("var(") { return css.to_string(); }
    let mut vars: Vec<(String, String)> = vec![];
    let b = css.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'-' && b[i + 1] == b'-' && (i == 0 || !(b[i - 1].is_ascii_alphanumeric())) {
            if let Some(c) = css[i..].find(':') {
                let name = css[i..i + c].trim().to_string();
                if !name.contains(|ch: char| ch.is_whitespace() || ch == '{' || ch == ';') {
                    let end = css[i + c..].find(|ch| ch == ';' || ch == '}').map(|e| i + c + e).unwrap_or(css.len());
                    let val = css[i + c + 1..end].trim().to_string();
                    vars.retain(|(n, _)| *n != name);
                    vars.push((name, val));
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    let mut s = css.to_string();
    for _ in 0..5 {
        let Some(p) = s.find("var(") else { break };
        let mut d = 0;
        let mut end = None;
        for (k, ch) in s[p + 3..].char_indices() {
            if ch == '(' { d += 1 } else if ch == ')' { d -= 1; if d == 0 { end = Some(p + 3 + k); break; } }
        }
        let Some(e) = end else { break };
        let inner = s[p + 4..e].to_string();
        let (name, fb) = match inner.split_once(',') { Some((n, f)) => (n.trim(), Some(f.trim())), None => (inner.trim(), None) };
        let rep = vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).or(fb.map(|f| f.to_string())).unwrap_or_default();
        s.replace_range(p..=e, &rep);
        if !s.contains("var(") { break; }
    }
    s
}

fn convert_units(css: &str, vw: f32, vh: f32) -> String {
    let chars: Vec<char> = css.chars().collect();
    let mut out = String::with_capacity(css.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev_ok = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == '#' || chars[i - 1] == '.');
        if (c.is_ascii_digit() || (c == '-' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit())) && prev_ok {
            let s = i;
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') { i += 1; }
            let num: String = chars[s..i].iter().collect();
            let us = i;
            while i < chars.len() && chars[i].is_ascii_alphabetic() { i += 1; }
            let unit: String = chars[us..i].iter().collect();
            let n: Option<f32> = num.parse().ok();
            match (unit.as_str(), n) {
                ("vw", Some(n)) => out.push_str(&format!("{}px", n * vw / 100.0)),
                ("vh", Some(n)) => out.push_str(&format!("{}px", n * vh / 100.0)),
                ("vmin", Some(n)) => out.push_str(&format!("{}px", n * vw.min(vh) / 100.0)),
                ("vmax", Some(n)) => out.push_str(&format!("{}px", n * vw.max(vh) / 100.0)),
                _ => { out.push_str(&num); out.push_str(&unit); }
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

// ---- calc() ----
#[derive(Clone, Copy, PartialEq)]
enum U { None, Px, Pct }

struct P<'a> { s: &'a [u8], i: usize }

impl<'a> P<'a> {
    fn ws(&mut self) { while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() { self.i += 1; } }
    fn expr(&mut self) -> Result<(f32, U), ()> {
        let mut l = self.term()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) { Some(b'+') => 1.0, Some(b'-') => -1.0, _ => return Ok(l) };
            self.i += 1;
            let r = self.term()?;
            if l.1 != r.1 { return Err(()); }
            l.0 += op * r.0;
        }
    }
    fn term(&mut self) -> Result<(f32, U), ()> {
        let mut l = self.factor()?;
        loop {
            self.ws();
            let op = match self.s.get(self.i) { Some(b'*') => '*', Some(b'/') => '/', _ => return Ok(l) };
            self.i += 1;
            let r = self.factor()?;
            if op == '*' {
                if l.1 == U::None { l = (l.0 * r.0, r.1) } else if r.1 == U::None { l.0 *= r.0 } else { return Err(()) }
            } else {
                if r.1 != U::None || r.0 == 0.0 { return Err(()); }
                l.0 /= r.0;
            }
        }
    }
    fn factor(&mut self) -> Result<(f32, U), ()> {
        self.ws();
        if self.s.get(self.i) == Some(&b'(') {
            self.i += 1;
            let v = self.expr()?;
            self.ws();
            if self.s.get(self.i) != Some(&b')') { return Err(()); }
            self.i += 1;
            return Ok(v);
        }
        let st = self.i;
        if matches!(self.s.get(self.i), Some(b'-') | Some(b'+')) { self.i += 1; }
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') { self.i += 1; }
        let n: f32 = std::str::from_utf8(&self.s[st..self.i]).map_err(|_| ())?.parse().map_err(|_| ())?;
        let us = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphabetic() || self.s[self.i] == b'%') { self.i += 1; }
        match &self.s[us..self.i] {
            b"" => Ok((n, U::None)),
            b"px" => Ok((n, U::Px)),
            b"em" | b"rem" => Ok((n * 16.0, U::Px)),
            b"pt" => Ok((n * 1.333, U::Px)),
            b"%" => Ok((n, U::Pct)),
            _ => Err(()),
        }
    }
}

fn eval_calc(css: &str) -> String {
    let mut s = css.to_string();
    let mut guard = 0;
    while let Some(p) = s.find("calc(") {
        guard += 1;
        if guard > 200 { break; }
        let mut d = 0;
        let mut end = None;
        for (k, ch) in s[p + 4..].char_indices() {
            if ch == '(' { d += 1 } else if ch == ')' { d -= 1; if d == 0 { end = Some(p + 4 + k); break; } }
        }
        let Some(e) = end else { break };
        let expr = s[p + 5..e].replace("calc(", "(");
        let mut parser = P { s: expr.as_bytes(), i: 0 };
        let rep = match parser.expr() {
            Ok((v, U::Px)) => format!("{}px", v),
            Ok((v, U::Pct)) => format!("{}%", v),
            Ok((v, U::None)) => format!("{}", v),
            Err(_) => "0px".to_string(), // unsupported mix (e.g. 100% - 20px): safest neutral value
        };
        s.replace_range(p..=e, &rep);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_filtering() {
        let css = "a{color:red}@media (max-width: 600px){a{color:blue}}@media (min-width: 900px){b{color:green}}";
        let out = preprocess(css, 1024.0, 768.0);
        assert!(out.contains("a{color:red}") && out.contains("b{color:green}") && !out.contains("blue"));
        let small = preprocess(css, 500.0, 700.0);
        assert!(small.contains("blue") && !small.contains("green"));
    }
    #[test]
    fn media_queries() {
        assert!(media_matches("screen and (min-width: 600px)", 800.0, 600.0));
        assert!(!media_matches("print", 800.0, 600.0));
        assert!(media_matches("not print", 800.0, 600.0));
        assert!(media_matches("(max-width: 400px), (min-width: 700px)", 800.0, 600.0));
        assert!(media_matches("(orientation: landscape)", 800.0, 600.0));
    }
    #[test]
    fn vars() {
        let out = preprocess(":root{--main:#ff0000;--pad:8px}p{color:var(--main);padding:var(--pad)}q{color:var(--nope, blue)}", 800.0, 600.0);
        assert!(out.contains("color:#ff0000;padding:8px") && out.contains("color:blue"), "{out}");
        assert!(out.starts_with("html{"));
    }
    #[test]
    fn units() {
        let out = preprocess("div{width:50vw;height:10vh;color:#1a2b3c;margin:1.5em}", 800.0, 600.0);
        assert!(out.contains("width:400px") && out.contains("height:60px") && out.contains("#1a2b3c") && out.contains("1.5em"), "{out}");
    }
    #[test]
    fn calc_eval() {
        assert_eq!(eval_calc("w:calc(100px + 2*8px)"), "w:116px");
        assert_eq!(eval_calc("w:calc((50% - 10%) / 2)"), "w:20%");
        assert_eq!(eval_calc("w:calc(1rem + 4px)"), "w:20px");
        assert_eq!(eval_calc("w:calc(100% - 20px)"), "w:0px");
        let out = preprocess("a{width:calc(100vw - 20px)}", 800.0, 600.0);
        assert!(out.contains("width:780px"), "{out}");
    }
    #[test]
    fn at_rules_dropped() {
        let out = preprocess("@import url(x.css);@font-face{font-family:x;src:url(a)}@keyframes k{from{a:b}to{a:c}}p{color:red}", 800.0, 600.0);
        assert!(out.contains("p{color:red}") && !out.contains("font-face") && !out.contains("keyframes"), "{out}");
    }
}
