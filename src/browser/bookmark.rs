use rusqlite::{params, Connection};

pub struct Bookmarks { conn: Connection }

fn e(x: rusqlite::Error) -> String { x.to_string() }

impl Bookmarks {
    pub fn open(path: &str) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(e)?;
        conn.execute("CREATE TABLE IF NOT EXISTS bookmarks(id INTEGER PRIMARY KEY, url TEXT UNIQUE, title TEXT)", []).map_err(e)?;
        Ok(Bookmarks { conn })
    }
    pub fn add(&self, url: &str, title: &str) -> Result<(), String> {
        self.conn.execute("INSERT OR REPLACE INTO bookmarks(url,title) VALUES(?1,?2)", params![url, title]).map_err(e)?;
        Ok(())
    }
    pub fn remove(&self, url: &str) -> Result<(), String> {
        self.conn.execute("DELETE FROM bookmarks WHERE url=?1", params![url]).map_err(e)?;
        Ok(())
    }
    pub fn contains(&self, url: &str) -> Result<bool, String> {
        let n: i64 = self.conn.query_row("SELECT COUNT(*) FROM bookmarks WHERE url=?1", params![url], |r| r.get(0)).map_err(e)?;
        Ok(n > 0)
    }
    pub fn list(&self) -> Result<Vec<(String, String)>, String> {
        let mut st = self.conn.prepare("SELECT url,title FROM bookmarks ORDER BY id DESC").map_err(e)?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(e)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(e)
    }
}
