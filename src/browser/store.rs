//! Tiny persistent store for the ONLY data that lives on disk: bookmarks and settings.
//! Plain text file (`key<TAB>value` per line, `\\ \t \n` escaped), atomic write (tmp + rename). No SQLite.
//! History, sessions, cache are RAM-only by design.
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub struct Store {
    path: Option<PathBuf>, // None = in-memory only (tests / private mode)
    map: BTreeMap<String, String>,
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

fn unesc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' { o.push(c); continue; }
        match it.next() { Some('t') => o.push('\t'), Some('n') => o.push('\n'), Some('r') => o.push('\r'), Some(x) => o.push(x), None => {} }
    }
    o
}

impl Store {
    pub fn memory() -> Self { Store { path: None, map: BTreeMap::new() } }

    /// Missing file = empty store. Corrupt lines are skipped, never fatal.
    pub fn open(path: &str) -> Result<Self, String> {
        let p = PathBuf::from(path);
        let mut map = BTreeMap::new();
        match fs::read_to_string(&p) {
            Ok(t) => for line in t.lines() {
                if let Some((k, v)) = line.split_once('\t') { map.insert(unesc(k), unesc(v)); }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("store read: {e}")),
        }
        Ok(Store { path: Some(p), map })
    }

    fn flush(&self) -> Result<(), String> {
        let Some(p) = &self.path else { return Ok(()) };
        let mut out = String::new();
        for (k, v) in &self.map { out.push_str(&esc(k)); out.push('\t'); out.push_str(&esc(v)); out.push('\n'); }
        let tmp = p.with_extension("tmp");
        fs::write(&tmp, out).map_err(|e| format!("store write: {e}"))?;
        fs::rename(&tmp, p).map_err(|e| format!("store rename: {e}"))
    }

    pub fn get(&self, k: &str) -> Option<&str> { self.map.get(k).map(|s| s.as_str()) }
    pub fn set(&mut self, k: &str, v: &str) -> Result<(), String> { self.map.insert(k.into(), v.into()); self.flush() }
    pub fn remove(&mut self, k: &str) -> Result<bool, String> { let had = self.map.remove(k).is_some(); if had { self.flush()?; } Ok(had) }
    pub fn with_prefix(&self, prefix: &str) -> Vec<(String, String)> {
        self.map.range(prefix.to_string()..).take_while(|(k, _)| k.starts_with(prefix)).map(|(k, v)| (k[prefix.len()..].to_string(), v.clone())).collect()
    }
    pub fn len(&self) -> usize { self.map.len() }
}

/// Same API as the old SQLite `Bookmarks`.
pub struct Bookmarks { s: Store }

impl Bookmarks {
    pub fn open(path: &str) -> Result<Self, String> { Ok(Bookmarks { s: Store::open(path)? }) }
    pub fn memory() -> Self { Bookmarks { s: Store::memory() } }
    pub fn add(&mut self, url: &str, title: &str) -> Result<(), String> { self.s.set(&format!("bm:{url}"), title) }
    pub fn remove(&mut self, url: &str) -> Result<(), String> { self.s.remove(&format!("bm:{url}")).map(|_| ()) }
    pub fn contains(&self, url: &str) -> Result<bool, String> { Ok(self.s.get(&format!("bm:{url}")).is_some()) }
    /// (url, title), sorted by url.
    pub fn list(&self) -> Result<Vec<(String, String)>, String> { Ok(self.s.with_prefix("bm:")) }
}

/// Same API as the old SQLite `Settings`.
pub struct Settings { s: Store }

impl Settings {
    pub fn open(path: &str) -> Result<Self, String> { Ok(Settings { s: Store::open(path)? }) }
    pub fn memory() -> Self { Settings { s: Store::memory() } }
    pub fn get(&self, k: &str) -> Result<Option<String>, String> { Ok(self.s.get(&format!("set:{k}")).map(|v| v.to_string())) }
    pub fn get_or(&self, k: &str, default: &str) -> String { self.s.get(&format!("set:{k}")).unwrap_or(default).to_string() }
    pub fn set(&mut self, k: &str, v: &str) -> Result<(), String> { self.s.set(&format!("set:{k}"), v) }
    pub fn remove(&mut self, k: &str) -> Result<(), String> { self.s.remove(&format!("set:{k}")).map(|_| ()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tmp(name: &str) -> String { format!("{}/sparrow_store_{}_{}", std::env::temp_dir().display(), std::process::id(), name) }
    #[test]
    fn roundtrip_with_special_chars() {
        let p = tmp("a");
        let _ = fs::remove_file(&p);
        let mut b = Bookmarks::open(&p).expect("open");
        b.add("https://x.org/a?b=1", "Tab\there\nnew \\ line বাংলা").expect("add");
        b.add("https://y.org/", "Y").expect("add");
        let b2 = Bookmarks::open(&p).expect("reopen");
        assert_eq!(b2.list().expect("list"), vec![("https://x.org/a?b=1".to_string(), "Tab\there\nnew \\ line বাংলা".to_string()), ("https://y.org/".to_string(), "Y".to_string())]);
        assert!(b2.contains("https://y.org/").expect("c"));
        let _ = fs::remove_file(&p);
    }
    #[test]
    fn settings_defaults_and_remove() {
        let p = tmp("b");
        let _ = fs::remove_file(&p);
        let mut s = Settings::open(&p).expect("open");
        assert_eq!(s.get_or("theme", "dark"), "dark");
        s.set("theme", "light").expect("set");
        assert_eq!(Settings::open(&p).expect("o").get_or("theme", "dark"), "light");
        s.remove("theme").expect("rm");
        assert_eq!(Settings::open(&p).expect("o").get("theme").expect("g"), None);
        let _ = fs::remove_file(&p);
    }
    #[test]
    fn corrupt_lines_skipped_and_prefixes_isolated() {
        let p = tmp("c");
        fs::write(&p, "garbage line without tab\nbm:u\tT\nset:k\tv\n").expect("w");
        let s = Store::open(&p).expect("open");
        assert_eq!(s.len(), 2);
        assert_eq!(s.with_prefix("bm:"), vec![("u".to_string(), "T".to_string())]);
        let _ = fs::remove_file(&p);
    }
    #[test]
    fn memory_mode_never_touches_disk() {
        let mut b = Bookmarks::memory();
        b.add("u", "t").expect("add");
        assert_eq!(b.list().expect("l").len(), 1);
    }
}
