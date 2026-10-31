//! SQLite WAL: исходные killmail и общая выборка хранятся раздельно.

use crate::model::{id, s};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;
use std::{
    io::Read,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
pub struct Store {
    pub(crate) db: Connection,
    pub seed: Option<crate::seed::SeedInfo>,
    pub seed_error: Option<String>,
    trace: Option<crate::diagnostics::Trace>,
}

pub fn timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
pub fn compress(v: &Value) -> Result<Vec<u8>, String> {
    let mut z = GzEncoder::new(Vec::new(), Compression::fast());
    serde_json::to_writer(&mut z, v).map_err(|e| e.to_string())?;
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
    #[cfg(test)]
    pub fn global_ships(&self) -> Result<Vec<i32>, String> {
        let sql = if self.seed.is_some() {
            "SELECT k.ship FROM main.killmails k JOIN main.global_membership g ON k.id=g.id UNION SELECT ship FROM bundled.killmails ORDER BY ship"
        } else {
            "SELECT DISTINCT k.ship FROM killmails k JOIN global_membership g ON k.id=g.id ORDER BY k.ship"
        };
        let mut statement = self.db.prepare(sql).map_err(|e| e.to_string())?;
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
        Self::open_with_seed(dir, crate::seed::default_path().as_deref())
    }
    pub fn open_with_seed(dir: &Path, seed_path: Option<&Path>) -> Result<Self, String> {
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
        crate::matchups::setup(&db)?;
        crate::annual_store::setup(&db)?;
        let mut store = Self {
            db,
            seed: None,
            seed_error: None,
            trace: None,
        };
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
        if let Some(path) = seed_path {
            match crate::seed::attach(&store.db, path) {
                Ok(info) => store.seed = Some(info),
                Err(error) => {
                    let _ = store.db.execute("DETACH DATABASE bundled", []);
                    store.seed_error = Some(error);
                }
            }
        }
        Ok(store)
    }
    pub fn set_trace(&mut self, trace: crate::diagnostics::Trace) {
        self.trace = Some(trace);
    }
    fn record(&self, kind: &str, fields: Value) {
        if let Some(trace) = &self.trace {
            trace.event(kind, fields);
        }
    }
    pub fn local_global_count(&self, ship: i32) -> Result<usize, String> {
        self.db.query_row("SELECT count(*) FROM main.killmails k JOIN main.global_membership g ON k.id=g.id WHERE ship=?",[ship],|r|r.get::<_, i64>(0)).map(|n| n as usize).map_err(|e|e.to_string())
    }
    pub fn archive_count(&self, day: &str) -> usize {
        let local = self
            .meta(&format!("archive_count:{day}"))
            .and_then(|v| v.as_u64());
        let seed = if self.seed.is_some() {
            self.db
                .query_row(
                    "SELECT coalesce(index_total,source_count) FROM bundled.days WHERE day=?",
                    [day],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap_or(0)
        } else {
            0
        };
        local.map(|n| n as usize).unwrap_or(seed as usize)
    }
    pub fn archive_matches(&self, day: &str, expected: usize) -> bool {
        self.archive_count(day) == expected
            || self.seed.is_some()
                && self
                    .db
                    .query_row(
                        "SELECT coalesce(index_total,source_count) FROM bundled.days WHERE day=?",
                        [day],
                        |r| r.get::<_, i64>(0),
                    )
                    .ok()
                    .is_some_and(|count| count as usize == expected)
    }
    pub fn seed_population(&self, key: &str) -> Option<Value> {
        if !self.seed.as_ref().is_some_and(|s| s.prepared) {
            return None;
        }
        let raw: Vec<u8> = self
            .db
            .query_row(
                "SELECT raw FROM bundled.populations WHERE key=?",
                [key],
                |r| r.get(0),
            )
            .ok()?;
        decompress(&raw).ok()
    }
    fn ingest_rows<'a>(
        &mut self,
        mails: impl IntoIterator<Item = &'a Value>,
        global: bool,
    ) -> Result<usize, String> {
        let tx = self.db.transaction().map_err(|e| e.to_string())?;
        let mut count = 0;
        let mut ships = std::collections::HashSet::new();
        {
            let mut insert = tx
                .prepare("INSERT OR IGNORE INTO killmails VALUES(?,?,?,?,?,?,?)")
                .map_err(|e| e.to_string())?;
            let mut membership = if global {
                Some(
                    tx.prepare("INSERT OR IGNORE INTO global_membership VALUES(?)")
                        .map_err(|e| e.to_string())?,
                )
            } else {
                None
            };
            for mail in mails {
                let km = id(mail, "killmail_id");
                let ship = id(&mail["victim"], "ship_type_id");
                if km == 0 || ship == 0 || s(mail, "killmail_time").is_empty() {
                    continue;
                }
                count += insert
                    .execute(params![
                        km,
                        ship,
                        id(&mail["victim"], "character_id"),
                        s(mail, "killmail_time"),
                        id(mail, "solar_system_id"),
                        id(&mail["victim"], "corporation_id"),
                        compress(mail)?
                    ])
                    .map_err(|e| e.to_string())?;
                if let Some(membership) = &mut membership
                    && membership.execute([km]).map_err(|e| e.to_string())? > 0
                {
                    ships.insert(ship);
                }
            }
        }
        for ship in ships {
            let key = format!("global_revision:{ship}");
            let previous: Option<String> = tx
                .query_row("SELECT value FROM meta WHERE key=?", [&key], |r| r.get(0))
                .optional()
                .map_err(|e| e.to_string())?;
            let revision = previous
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
                + 1;
            tx.execute(
                "INSERT OR REPLACE INTO meta VALUES(?,?)",
                params![key, revision.to_string()],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(count)
    }
    pub fn ingest(&mut self, mails: &[Value]) -> Result<usize, String> {
        self.ingest_rows(mails, false)
    }
    // Персональные запросы пишут в killmails, но не в global_membership.
    // Это сохраняет проценты общей выборки во время игры.
    pub fn ingest_global(&mut self, mails: &[Value]) -> Result<usize, String> {
        self.ingest_rows(mails, true)
    }
    /// Normal archive updates only add records absent from the read-only starter snapshot.
    pub fn ingest_archive(&mut self, mails: &[Value], force: bool) -> Result<usize, String> {
        if force || self.seed.is_none() {
            return self.ingest_global(mails);
        }
        let fresh = {
            let mut present = self
                .db
                .prepare("SELECT 1 FROM bundled.killmails WHERE id=?")
                .map_err(|error| error.to_string())?;
            let mut fresh = Vec::new();
            for mail in mails {
                if !present
                    .exists([id(mail, "killmail_id")])
                    .map_err(|error| error.to_string())?
                {
                    fresh.push(mail);
                }
            }
            fresh
        };
        self.ingest_rows(fresh, true)
    }
    pub fn global_mails(&self, ship: i32) -> Result<Vec<Value>, String> {
        let sql = if self.seed.is_some() {
            "SELECT raw,compressed FROM (SELECT k.id,k.happened,k.raw,1 AS compressed FROM main.killmails k JOIN main.global_membership g ON g.id=k.id WHERE k.ship=?1 UNION ALL SELECT k.id,k.happened,k.raw,0 AS compressed FROM bundled.killmails k WHERE k.ship=?1 AND NOT EXISTS(SELECT 1 FROM main.global_membership g JOIN main.killmails m ON m.id=g.id WHERE g.id=k.id)) ORDER BY happened DESC,id DESC"
        } else {
            "SELECT raw,1 AS compressed FROM main.killmails WHERE ship=?1 AND id IN (SELECT id FROM main.global_membership) ORDER BY happened DESC,id DESC"
        };
        self.mails(sql, ship, None, "popular_killmails")
    }
    pub fn pilot_mails(&self, ship: i32, pilot: i64) -> Result<Vec<Value>, String> {
        let sql = if self.seed.is_some() {
            "SELECT raw,compressed FROM (SELECT id,happened,raw,1 AS compressed FROM main.killmails WHERE ship=?1 AND pilot=?2 UNION ALL SELECT k.id,k.happened,k.raw,0 AS compressed FROM bundled.killmails k WHERE k.ship=?1 AND k.pilot=?2 AND NOT EXISTS(SELECT 1 FROM main.killmails m WHERE m.id=k.id)) ORDER BY happened DESC,id DESC"
        } else {
            "SELECT raw,1 AS compressed FROM main.killmails WHERE ship=?1 AND pilot=?2 ORDER BY happened DESC,id DESC"
        };
        self.mails(sql, ship, Some(pilot), "pilot_killmails")
    }
    fn mails(
        &self,
        sql: &str,
        ship: i32,
        pilot: Option<i64>,
        label: &str,
    ) -> Result<Vec<Value>, String> {
        let started = std::time::Instant::now();
        let result = (|| {
            let mut stmt = self.db.prepare(sql).map_err(|e| e.to_string())?;
            let decode = |row: &rusqlite::Row<'_>| -> rusqlite::Result<(Vec<u8>, bool)> {
                let raw = match row.get_ref(0)? {
                    rusqlite::types::ValueRef::Blob(raw) | rusqlite::types::ValueRef::Text(raw) => {
                        raw.to_vec()
                    }
                    _ => Vec::new(),
                };
                Ok((raw, row.get::<_, i32>(1)? != 0))
            };
            let mut values = Vec::new();
            let mut rows = if let Some(pilot) = pilot {
                stmt.query(params![ship, pilot])
            } else {
                stmt.query([ship])
            }
            .map_err(|e| e.to_string())?;
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let (raw, compressed) = decode(row).map_err(|e| e.to_string())?;
                values.push(if compressed {
                    decompress(&raw)?
                } else {
                    serde_json::from_slice(&raw).map_err(|e| e.to_string())?
                });
            }
            Ok::<_, String>(values)
        })();
        self.record("sql",serde_json::json!({"query":label,"statement":sql,"ship_id":ship,"pilot_id":pilot,"rows":result.as_ref().ok().map(Vec::len),"error":result.as_ref().err(),"elapsed_ms":started.elapsed().as_millis(),"bundled":self.seed}));
        result
    }
    pub fn cached(&self, key: &str) -> Option<Value> {
        let started = std::time::Instant::now();
        let row = self.db.query_row(
            "SELECT raw FROM http_cache WHERE key=? AND expires>?",
            params![key, timestamp()],
            |r| r.get::<_, Vec<u8>>(0),
        );
        let (value, error) = match row {
            Ok(raw) => match decompress(&raw) {
                Ok(value) => (Some(value), None),
                Err(e) => (None, Some(e)),
            },
            Err(rusqlite::Error::QueryReturnedNoRows) => (None, None),
            Err(e) => (None, Some(e.to_string())),
        };
        self.record("sql_cache",serde_json::json!({"query":"http_cache_get","key_hash":crate::fits::hash(key),"hit":value.is_some(),"error":error,"elapsed_ms":started.elapsed().as_millis()}));
        value
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
    pub fn cache_nonblocking(&self, key: &str, value: &Value, ttl: i64) -> Result<(), String> {
        self.db
            .busy_timeout(std::time::Duration::ZERO)
            .map_err(|e| e.to_string())?;
        let result = self.cache(key, value, ttl);
        let restored = self
            .db
            .busy_timeout(std::time::Duration::from_secs(15))
            .map_err(|e| e.to_string());
        result.and(restored)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failed_global_import_preserves_mails_membership_and_revision() {
        let directory =
            std::env::temp_dir().join(crate::diagnostics::trace_id("eve-global-import-rollback"));
        let mut store = Store::open_with_seed(&directory, None).unwrap();
        let mail = |km| {
            json!({"killmail_id":km,"killmail_time":"2026-10-29T12:00:00Z",
                "victim":{"ship_type_id":587,"character_id":123}})
        };
        store.ingest_global(&[mail(1)]).unwrap();
        let previous_revision = store.meta("global_revision:587");
        store.db.execute_batch("CREATE TRIGGER reject_import BEFORE INSERT ON global_membership WHEN NEW.id=3 BEGIN SELECT RAISE(ABORT,'simulated membership failure'); END;").unwrap();

        let error = store.ingest_global(&[mail(2), mail(3)]).unwrap_err();
        assert!(error.contains("simulated membership failure"));
        // A failed archive must not become visible as personal or popular fits,
        // nor advance the revision used to invalidate the population cache.
        assert_eq!(store.pilot_mails(587, 123).unwrap().len(), 1);
        assert_eq!(store.local_global_count(587).unwrap(), 1);
        assert_eq!(store.meta("global_revision:587"), previous_revision);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
