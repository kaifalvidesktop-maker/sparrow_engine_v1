pub type NodeId = usize;

#[derive(Debug, Clone)]
pub enum NodeKind {
    Document,
    Element { tag: String, attrs: Vec<(String, String)> },
    Text(String),
    Comment,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

#[derive(Debug, Clone)]
pub struct Dom {
    pub nodes: Vec<Node>,
}

impl Dom {
    pub fn new() -> Self {
        Dom { nodes: vec![Node { kind: NodeKind::Document, parent: None, children: vec![] }] }
    }
    pub fn create(&mut self, kind: NodeKind) -> NodeId {
        self.nodes.push(Node { kind, parent: None, children: vec![] });
        self.nodes.len() - 1
    }
    pub fn detach(&mut self, id: NodeId) {
        if let Some(p) = self.nodes[id].parent.take() {
            self.nodes[p].children.retain(|&c| c != id);
        }
    }
    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        self.detach(child);
        self.nodes[child].parent = Some(parent);
        self.nodes[parent].children.push(child);
    }
    pub fn insert_before(&mut self, sibling: NodeId, child: NodeId) {
        self.detach(child);
        if let Some(p) = self.nodes[sibling].parent {
            let pos = self.nodes[p].children.iter().position(|&c| c == sibling).unwrap_or(self.nodes[p].children.len());
            self.nodes[p].children.insert(pos, child);
            self.nodes[child].parent = Some(p);
        }
    }
    pub fn tag(&self, id: NodeId) -> Option<&str> {
        match &self.nodes[id].kind {
            NodeKind::Element { tag, .. } => Some(tag),
            _ => None,
        }
    }
    pub fn attr(&self, id: NodeId, name: &str) -> Option<&str> {
        match &self.nodes[id].kind {
            NodeKind::Element { attrs, .. } => attrs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()),
            _ => None,
        }
    }
    /// Preorder list of all descendants
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = vec![];
        let mut stack: Vec<NodeId> = self.nodes[id].children.iter().rev().cloned().collect();
        while let Some(n) = stack.pop() {
            out.push(n);
            stack.extend(self.nodes[n].children.iter().rev());
        }
        out
    }
    pub fn text_content(&self, id: NodeId) -> String {
        let mut s = String::new();
        for d in self.descendants(id) {
            if let NodeKind::Text(t) = &self.nodes[d].kind {
                s.push_str(t);
            }
        }
        s
    }
    pub fn find_tag(&self, tag: &str) -> Option<NodeId> {
        self.descendants(0).into_iter().find(|&n| self.tag(n) == Some(tag))
    }
}
