//! Bridge between our Rust DOM and the JS DOM layer (`dom_bootstrap.js`).
//! Only uses `JsRuntime::eval` (no boa-specific API) so it is robust to boa version changes.
//! Flow: Rust DOM -> JS literal -> hydrate -> scripts mutate JS DOM -> `__serialize()` -> html5ever re-parse.
use crate::dro_dom::{Dom, NodeKind};
use crate::dro_js::JsRuntime;

const BOOT: &str = include_str!("dom_bootstrap.js");
const MAX_LITERAL: usize = 16 * 1024 * 1024;

/// Escape a Rust string as a double-quoted JS string literal.
pub fn js_string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            '\u{2028}' => o.push_str("\\u2028"),
            '\u{2029}' => o.push_str("\\u2029"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// DOM -> `[[kind, text_or_tag, [[attr,val]..], [childIds..]], ...]` (index == NodeId).
/// kind: 0 document, 1 element, 2 text, 3 comment. Script bodies are omitted (already executed by Rust).
pub fn dom_literal(dom: &Dom) -> Result<String, String> {
    let mut o = String::with_capacity(dom.nodes.len() * 48);
    o.push('[');
    for (i, n) in dom.nodes.iter().enumerate() {
        if i > 0 { o.push(','); }
        let (kind, text, attrs): (u8, String, String) = match &n.kind {
            NodeKind::Document => (0, String::new(), "[]".into()),
            NodeKind::Comment => (3, String::new(), "[]".into()),
            NodeKind::Text(t) => {
                let in_script = n.parent.and_then(|p| dom.tag(p)) == Some("script");
                (2, if in_script { String::new() } else { t.clone() }, "[]".into())
            }
            NodeKind::Element { tag, attrs } => {
                let a: Vec<String> = attrs.iter().map(|(k, v)| format!("[{},{}]", js_string(k), js_string(v))).collect();
                (1, tag.clone(), format!("[{}]", a.join(",")))
            }
        };
        let kids: Vec<String> = n.children.iter().map(|c| c.to_string()).collect();
        o.push_str(&format!("[{},{},{},[{}]]", kind, js_string(&text), attrs, kids.join(",")));
        if o.len() > MAX_LITERAL { return Err("document too large for JS DOM".into()); }
    }
    o.push(']');
    Ok(o)
}

pub fn install(rt: &mut JsRuntime, dom: &Dom, url: &str, _title: &str, vw: u32, vh: u32) -> Result<(), String> {
    let lit = dom_literal(dom)?;
    rt.eval(&format!("var __DOM={};var __URL={};var __VW={};var __VH={};", lit, js_string(url), vw, vh))?;
    rt.eval(BOOT)?;
    Ok(())
}

/// Fire DOMContentLoaded, run pending timers (max 1s virtual time), fire load.
pub fn lifecycle(rt: &mut JsRuntime) {
    let _ = rt.eval("__fire('DOMContentLoaded');");
    let _ = rt.eval("__runTimers(1000, 300);");
    let _ = rt.eval("__fire('load');");
}

/// Advance the virtual clock by `ms` and run due timers. Returns how many ran.
pub fn run_timers(rt: &mut JsRuntime, ms: u32) -> usize {
    rt.eval(&format!("__runTimers({}, 100)", ms)).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0)
}

/// `Some(html)` only if JS changed the DOM since the last call.
pub fn serialize_if_dirty(rt: &mut JsRuntime) -> Result<Option<String>, String> {
    if rt.eval("__isDirty()")? != "true" { return Ok(None); }
    let h = rt.eval("__serialize()")?;
    Ok(if h.is_empty() { None } else { Some(h) })
}

/// Navigation requested by JS (`location.href = ..`, clicked link, `window.open`).
pub fn take_navigation(rt: &mut JsRuntime) -> Option<String> {
    rt.eval("__takeNav()").ok().filter(|s| !s.is_empty() && s != "undefined")
}

/// Dispatch a DOM event on the element with the given `id` attribute.
pub fn fire(rt: &mut JsRuntime, element_id: &str, event: &str) -> Result<bool, String> {
    Ok(rt.eval(&format!("__fireOn({},{})", js_string(element_id), js_string(event)))? == "true")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes() {
        assert_eq!(js_string("a\"b\\c\nd\u{2028}"), "\"a\\\"b\\\\c\\nd\\u2028\"");
    }
    #[test]
    fn literal_shape() {
        let mut d = Dom::new();
        let h = d.create(NodeKind::Element { tag: "html".into(), attrs: vec![("lang".into(), "en".into())] });
        let t = d.create(NodeKind::Text("hi \"x\"".into()));
        d.append_child(0, h);
        d.append_child(h, t);
        assert_eq!(dom_literal(&d).unwrap_or_default(), "[[0,\"\",[],[1]],[1,\"html\",[[\"lang\",\"en\"]],[2]],[2,\"hi \\\"x\\\"\",[],[]]]");
    }
}
