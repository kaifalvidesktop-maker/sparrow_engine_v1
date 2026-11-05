use rusqlite::{params, Connection};

/// Simple persistent key/value settings (SQLite). Keys used by the shell:
/// "search_engine", "homepage", "js_enabled", "theme".
pub struct Settings { conn: Connection }

fn e(x: rusqlite::Error) -> String { x.to_string() }

impl Settings {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(e)?;
        conn.execute("CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT)", []).map_err(e)?;
        Ok(Settings { conn })
    }
    pub fn get(&self, key: &str) -> Result<Option<String>, String> {
        let mut st = self.conn.prepare("SELECT value FROM settings WHERE key=?1").map_err(e)?;
        let mut rows = st.query(params![key]).map_err(e)?;
        match rows.next().map_err(e)? {
            Some(r) => Ok(Some(r.get(0).map_err(e)?)),
            None => Ok(None),
        }
    }
    pub fn get_or(&self, key: &str, default: &str) -> String {
        self.get(key).ok().flatten().unwrap_or_else(|| default.to_string())
    }
    pub fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.conn.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2", params![key, value]).map_err(e)?;
        Ok(())
    }
    pub fn remove(&self, key: &str) -> Result<(), String> {
        self.conn.execute("DELETE FROM settings WHERE key=?1", params![key]).map_err(e)?;
        Ok(())
    }
}
