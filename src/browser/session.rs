use super::tabs::Tabs;
use rusqlite::{params, Connection};

/// Save/restore open tabs between runs.
pub struct Session { conn: Connection }

pub struct Restored {
    pub urls: Vec<String>,
    pub active: usize,
}

fn e(x: rusqlite::Error) -> String { x.to_string() }

impl Session {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(e)?;
        conn.execute("CREATE TABLE IF NOT EXISTS session(pos INTEGER PRIMARY KEY, url TEXT NOT NULL, active INTEGER NOT NULL)", []).map_err(e)?;
        Ok(Session { conn })
    }

    /// Snapshot all tabs (blank tabs are skipped).
    pub fn save(&mut self, tabs: &mut Tabs) -> Result<(), String> {
        let active_id = tabs.active().id;
        let rows: Vec<(String, bool)> = tabs.list().iter().filter(|t| t.engine.url() != "about:blank").map(|t| (t.engine.url().to_string(), t.id == active_id)).collect();
        let tx = self.conn.transaction().map_err(e)?;
        tx.execute("DELETE FROM session", []).map_err(e)?;
        for (i, (u, a)) in rows.iter().enumerate() {
            tx.execute("INSERT INTO session(pos,url,active) VALUES(?1,?2,?3)", params![i as i64, u, *a as i64]).map_err(e)?;
        }
        tx.commit().map_err(e)
    }

    /// Read the saved list. The shell decides how to load (recommended: load only the active tab, others lazily).
    pub fn load(&self) -> Result<Restored, String> {
        let mut st = self.conn.prepare("SELECT url, active FROM session ORDER BY pos").map_err(e)?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))).map_err(e)?;
        let mut urls = vec![];
        let mut active = 0;
        for (i, r) in rows.enumerate() {
            let (u, a) = r.map_err(e)?;
            if a { active = i; }
            urls.push(u);
        }
        Ok(Restored { urls, active })
    }

    pub fn clear(&self) -> Result<(), String> {
        self.conn.execute("DELETE FROM session", []).map_err(e)?;
        Ok(())
    }
}
