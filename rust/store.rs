//! SQLite WAL: исходные killmail и общая выборка хранятся раздельно.

use crate::model::{id, s};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub struct Store {
    db: Connection,
}
pub fn timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn compress(v: &Value) -> Result<Vec<u8>, String> {
    let mut z = GzEncoder::new(Vec::new(), Compression::fast());
    z.write_all(v.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    z.finish().map_err(|e| e.to_string())
}
pub fn decompress(raw: &[u8]) -> Result<Value, String> {
    let mut b = Vec::new();
    GzDecoder::new(raw)
        .read_to_end(&mut b)
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&b).map_err(|e| e.to_string())
}
impl Store {
    pub fn global_ships(&self) -> Result<Vec<i32>, String> {
        let mut statement = self.db.prepare("SELECT DISTINCT k.ship FROM killmails k JOIN global_membership g ON k.id=g.id ORDER BY k.ship")
            .map_err(|e| e.to_string())?;
        statement
            .query_map([], |row| row.get(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    pub fn invalidate_statistics_cache(&self) -> Result<(), String> {
        self.db.execute("DELETE FROM http_cache WHERE key LIKE 'https://zkillboard.com/api/losses/shipTypeID/%' OR key LIKE 'population:%'", [])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn prepare_archive_refresh(&self, start: &str, end: &str) -> Result<(), String> {
        let period = serde_json::json!([start, end]);
        if self.meta("archive_refresh_pending") != Some(period.clone()) {
            self.db
                .execute(
                    "DELETE FROM meta WHERE key >= ? AND key <= ?",
                    params![format!("archive:{start}"), format!("archive:{end}")],
                )
                .map_err(|e| e.to_string())?;
            self.set_meta("archive_refresh_pending", &period)?;
        }
        Ok(())
    }
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let db = Connection::open(dir.join("combat.sqlite3")).map_err(|e| e.to_string())?;
        db.busy_timeout(std::time::Duration::from_secs(15))
            .map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL;
          CREATE TABLE IF NOT EXISTS killmails(id INTEGER PRIMARY KEY,ship INTEGER NOT NULL,pilot INTEGER,
            happened TEXT NOT NULL,system INTEGER,corp INTEGER,raw BLOB NOT NULL);
          CREATE INDEX IF NOT EXISTS km_ship_time ON killmails(ship,happened);
          CREATE INDEX IF NOT EXISTS km_pilot_ship ON killmails(pilot,ship,happened);
          CREATE TABLE IF NOT EXISTS http_cache(key TEXT PRIMARY KEY,expires REAL,raw BLOB);
          CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT);
          CREATE TABLE IF NOT EXISTS global_membership(id INTEGER PRIMARY KEY);") .map_err(|e|e.to_string())?;
        let store = Self { db };
        if store.meta("global_membership_v1").is_none() {
            store
                .db
                .execute(
                    "INSERT OR IGNORE INTO global_membership SELECT id FROM killmails",
                    [],
                )
                .map_err(|e| e.to_string())?;
            store.set_meta("global_membership_v1", &serde_json::json!(true))?;
        }
        Ok(store)
    }
    pub fn ingest(&mut self, mails: &[Value]) -> Result<usize, String> {
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        let mut count = 0;
        for m in mails {
            if id(m, "killmail_id") == 0
                || id(&m["victim"], "ship_type_id") == 0
                || s(m, "killmail_time").is_empty()
            {
                continue;
            }
            let raw = compress(m)?;
            count += tx
                .execute(
                    "INSERT OR IGNORE INTO killmails VALUES(?,?,?,?,?,?,?)",
                    params![
                        id(m, "killmail_id"),
                        id(&m["victim"], "ship_type_id"),
                        id(&m["victim"], "character_id"),
                        s(m, "killmail_time"),
                        id(m, "solar_system_id"),
                        id(&m["victim"], "corporation_id"),
                        raw
                    ],
                )
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }
    // Персональные запросы пишут в killmails, но не в global_membership.
    // Это сохраняет проценты общей выборки во время игры.
    pub fn ingest_global(&mut self, mails: &[Value]) -> Result<usize, String> {
        let count = self.ingest(mails)?;
        let mut ships = std::collections::HashSet::new();
        for mail in mails {
            let km = id(mail, "killmail_id");
            if km == 0 {
                continue;
            }
            let changed = self
                .db
                .execute("INSERT OR IGNORE INTO global_membership VALUES(?)", [km])
                .map_err(|e| e.to_string())?;
            if changed > 0 {
                ships.insert(id(&mail["victim"], "ship_type_id"));
            }
        }
        for ship in ships {
            let key = format!("global_revision:{ship}");
            let revision = self.meta(&key).and_then(|v| v.as_i64()).unwrap_or(0) + 1;
            self.set_meta(&key, &serde_json::json!(revision))?;
        }
        Ok(count)
    }
    pub fn global_mails(&self, ship: i32) -> Result<Vec<Value>, String> {
        let sql = "SELECT k.raw FROM killmails k
            JOIN global_membership g ON g.id = k.id
            WHERE k.ship = ? ORDER BY k.happened DESC, k.id DESC";
        let mut stmt = self.db.prepare(sql).map_err(|error| error.to_string())?;
        let rows = stmt
            .query_map([ship], |r| r.get::<_, Vec<u8>>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| decompress(&r.map_err(|e| e.to_string())?))
            .collect()
    }
    pub fn pilot_mails(&self, ship: i32, pilot: i64) -> Result<Vec<Value>, String> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT raw FROM killmails WHERE ship=? AND pilot=? ORDER BY happened DESC,id DESC",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![ship, pilot], |r| r.get::<_, Vec<u8>>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| decompress(&r.map_err(|e| e.to_string())?))
            .collect()
    }
    pub fn cached(&self, key: &str) -> Option<Value> {
        let raw: Vec<u8> = self
            .db
            .query_row(
                "SELECT raw FROM http_cache WHERE key=? AND expires>?",
                params![key, timestamp()],
                |r| r.get(0),
            )
            .ok()?;
        decompress(&raw).ok()
    }
    pub fn cache(&self, key: &str, value: &Value, ttl: i64) -> Result<(), String> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO http_cache VALUES(?,?,?)",
                params![key, timestamp() + ttl, compress(value)?],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn meta(&self, key: &str) -> Option<Value> {
        let text: String = self
            .db
            .query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
            .ok()?;
        serde_json::from_str(&text).ok()
    }
    pub fn set_meta(&self, key: &str, value: &Value) -> Result<(), String> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO meta VALUES(?,?)",
                params![key, value.to_string()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}
