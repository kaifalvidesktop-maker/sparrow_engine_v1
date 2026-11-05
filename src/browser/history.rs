use rusqlite::{params, Connection};

pub struct History { conn: Connection }

fn e(x: rusqlite::Error) -> String { x.to_string() }

impl History {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(e)?;
        conn.execute("CREATE TABLE IF NOT EXISTS history(url TEXT PRIMARY KEY, title TEXT, visits INTEGER DEFAULT 1, last INTEGER)", []).map_err(e)?;
        Ok(History { conn })
    }
    pub fn add(&self, url: &str, title: &str) -> Result<(), String> {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        self.conn.execute(
            "INSERT INTO history(url,title,last) VALUES(?1,?2,?3) ON CONFLICT(url) DO UPDATE SET visits=visits+1,title=?2,last=?3",
            params![url, title, now],
        ).map_err(e)?;
        Ok(())
    }
    pub fn search(&self, q: &str, limit: usize) -> Result<Vec<(String, String)>, String> {
        let mut st = self.conn.prepare("SELECT url,title FROM history WHERE url LIKE ?1 ESCAPE '\\' OR title LIKE ?1 ESCAPE '\\' ORDER BY last DESC LIMIT ?2").map_err(e)?;
        let rows = st.query_map(params![format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")), limit as i64], |r| Ok((r.get(0)?, r.get(1)?))).map_err(e)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(e)
    }
    pub fn clear(&self) -> Result<(), String> {
        self.conn.execute("DELETE FROM history", []).map_err(e)?;
        Ok(())
    }
}
