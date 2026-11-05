use crate::dro_dom::*;
use html5ever::tendril::{StrTendril, TendrilSink};
use html5ever::tree_builder::{ElementFlags, NodeOrText, QuirksMode, TreeSink};
use html5ever::{namespace_url, parse_document, Attribute, ExpandedName, ParseOpts, QualName};
use std::borrow::Cow;
use std::rc::Rc;

pub struct H {
    id: NodeId,
    name: QualName,
}
type Handle = Rc<H>;

struct Sink {
    dom: Dom,
    doc: Handle,
}

fn blank() -> QualName {
    QualName::new(None, html5ever::ns!(), html5ever::LocalName::from(""))
}

impl Sink {
    fn mk(&mut self, k: NodeKind, name: QualName) -> Handle {
        let id = self.dom.create(k);
        Rc::new(H { id, name })
    }
    fn node(&mut self, c: NodeOrText<Handle>) -> NodeId {
        match c {
            NodeOrText::AppendNode(h) => h.id,
            NodeOrText::AppendText(t) => self.dom.create(NodeKind::Text(t.to_string())),
        }
    }
}

impl TreeSink for Sink {
    type Handle = Handle;
    type Output = Dom;

    fn finish(self) -> Dom {
        self.dom
    }
    fn parse_error(&mut self, _: Cow<'static, str>) {}
    fn get_document(&mut self) -> Handle {
        self.doc.clone()
    }
    fn elem_name<'a>(&'a self, t: &'a Handle) -> ExpandedName<'a> {
        t.name.expanded()
    }
    fn create_element(&mut self, name: QualName, attrs: Vec<Attribute>, _: ElementFlags) -> Handle {
        let kind = NodeKind::Element {
            tag: name.local.to_string(),
            attrs: attrs.iter().map(|a| (a.name.local.to_string(), a.value.to_string())).collect(),
        };
        self.mk(kind, name)
    }
    fn create_comment(&mut self, _: StrTendril) -> Handle {
        self.mk(NodeKind::Comment, blank())
    }
    fn create_pi(&mut self, _: StrTendril, _: StrTendril) -> Handle {
        self.mk(NodeKind::Comment, blank())
    }
    fn append(&mut self, p: &Handle, c: NodeOrText<Handle>) {
        if let NodeOrText::AppendText(t) = &c {
            if let Some(&last) = self.dom.nodes[p.id].children.last() {
                if let NodeKind::Text(s) = &mut self.dom.nodes[last].kind {
                    s.push_str(t);
                    return;
                }
            }
        }
        let n = self.node(c);
        self.dom.append_child(p.id, n);
    }
    fn append_based_on_parent_node(&mut self, e: &Handle, prev: &Handle, c: NodeOrText<Handle>) {
        if self.dom.nodes[e.id].parent.is_some() {
            self.append_before_sibling(e, c)
        } else {
            self.append(prev, c)
        }
    }
    fn append_doctype_to_document(&mut self, _: StrTendril, _: StrTendril, _: StrTendril) {}
    fn get_template_contents(&mut self, t: &Handle) -> Handle {
        t.clone()
    }
    fn same_node(&self, a: &Handle, b: &Handle) -> bool {
        a.id == b.id
    }
    fn set_quirks_mode(&mut self, _: QuirksMode) {}
    fn append_before_sibling(&mut self, s: &Handle, c: NodeOrText<Handle>) {
        let n = self.node(c);
        self.dom.insert_before(s.id, n);
    }
    fn add_attrs_if_missing(&mut self, t: &Handle, attrs: Vec<Attribute>) {
        if let NodeKind::Element { attrs: have, .. } = &mut self.dom.nodes[t.id].kind {
            for a in attrs {
                let k = a.name.local.to_string();
                if !have.iter().any(|(n, _)| *n == k) {
                    have.push((k, a.value.to_string()));
                }
            }
        }
    }
    fn remove_from_parent(&mut self, t: &Handle) {
        self.dom.detach(t.id);
    }
    fn reparent_children(&mut self, n: &Handle, new: &Handle) {
        let kids = std::mem::take(&mut self.dom.nodes[n.id].children);
        for k in kids {
            self.dom.append_child(new.id, k);
        }
    }
}

pub fn parse(html: &str) -> Result<Dom, String> {
    let sink = Sink { dom: Dom::new(), doc: Rc::new(H { id: 0, name: blank() }) };
    Ok(parse_document(sink, ParseOpts::default()).one(StrTendril::from_slice(html)))
}
