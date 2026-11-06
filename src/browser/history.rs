//! History is RAM-only by design: nothing is written to disk, everything is gone on exit.
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 5000;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub url: String, pub title: String, pub visits: u32, pub last: i64 }

#[derive(Default)]
pub struct History { items: Vec<Entry> }

fn now() -> i64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) }

impl History {
    pub fn new() -> Self { History { items: Vec::new() } }
    /// Compatibility with the old SQLite API: the path is ignored on purpose.
    pub fn open(_path: &str) -> Result<Self, String> { Ok(Self::new()) }

    pub fn add(&mut self, url: &str, title: &str) -> Result<(), String> {
        if url.is_empty() || url == "about:blank" { return Ok(()); }
        let t = now();
        if let Some(e) = self.items.iter_mut().find(|e| e.url == url) {
            e.visits += 1; e.last = t;
            if !title.is_empty() { e.title = title.to_string(); }
        } else {
            if self.items.len() >= MAX_ENTRIES {
                if let Some(i) = (0..self.items.len()).min_by_key(|&i| self.items[i].last) { self.items.remove(i); }
            }
            self.items.push(Entry { url: url.into(), title: title.into(), visits: 1, last: t });
        }
        Ok(())
    }
    pub fn search(&self, q: &str, limit: usize) -> Result<Vec<(String, String)>, String> {
        let q = q.to_lowercase();
        let mut v: Vec<&Entry> = self.items.iter().filter(|e| e.url.to_lowercase().contains(&q) || e.title.to_lowercase().contains(&q)).collect();
        v.sort_by(|a, b| b.last.cmp(&a.last));
        Ok(v.into_iter().take(limit).map(|e| (e.url.clone(), e.title.clone())).collect())
    }
    pub fn recent(&self, limit: usize) -> Vec<Entry> {
        let mut v = self.items.clone();
        v.sort_by(|a, b| b.last.cmp(&a.last));
        v.truncate(limit);
        v
    }
    pub fn delete_since(&mut self, secs_ago: i64) { let cut = now() - secs_ago; self.items.retain(|e| e.last < cut); }
    pub fn clear(&mut self) -> Result<(), String> { self.items.clear(); Ok(()) }
    pub fn len(&self) -> usize { self.items.len() }
    pub fn is_empty(&self) -> bool { self.items.is_empty() }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn add_search_clear() {
        let mut h = History::new();
        h.add("https://a.com/x_y", "Alpha").unwrap();
        h.add("https://a.com/x_y", "").unwrap();
        h.add("https://b.com", "Beta").unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h.recent(10).iter().find(|e| e.url.contains("a.com")).unwrap().visits, 2);
        assert_eq!(h.search("alpha", 5).unwrap().len(), 1);
        assert_eq!(h.search("%", 5).unwrap().len(), 0);
        h.clear().unwrap();
        assert!(h.is_empty());
    }
    #[test] fn ignores_blank_and_caps() {
        let mut h = History::open("ignored.db").unwrap();
        h.add("about:blank", "").unwrap();
        assert!(h.is_empty());
        for i in 0..(MAX_ENTRIES + 50) { h.add(&format!("https://s{i}.com"), "").unwrap(); }
        assert_eq!(h.len(), MAX_ENTRIES);
    }
}
