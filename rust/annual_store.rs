//! Compact annual observations; personal lookups never certify global day coverage.
use crate::{
    annual_rating::{Duel, window, window_days},
    store::Store,
};
use chrono::{TimeDelta, Utc};
use rusqlite::{Connection, params};
use std::collections::HashSet;

pub const POLICY: &str = "pilot-evidence-v3";
pub fn setup(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS annual_rating_duels(id INTEGER PRIMARY KEY,day TEXT NOT NULL,at INTEGER NOT NULL,winner INTEGER NOT NULL,loser INTEGER NOT NULL,winner_hull INTEGER NOT NULL,loser_hull INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS annual_rating_winner ON annual_rating_duels(winner,at);
        CREATE INDEX IF NOT EXISTS annual_rating_loser ON annual_rating_duels(loser,at);
        CREATE INDEX IF NOT EXISTS annual_rating_day ON annual_rating_duels(day);
        CREATE INDEX IF NOT EXISTS annual_rating_time ON annual_rating_duels(at);
        CREATE TABLE IF NOT EXISTS annual_rating_exclusions(id INTEGER PRIMARY KEY,at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS annual_rating_exclusion_time ON annual_rating_exclusions(at);
        CREATE TABLE IF NOT EXISTS annual_rating_days(day TEXT PRIMARY KEY,source_count INTEGER NOT NULL,policy TEXT NOT NULL);")
        .map_err(|e|e.to_string())
}
fn bundled(db: &Connection) -> bool {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM bundled.sqlite_schema WHERE name='annual_rating_duels')",
        [],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}
fn day(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .unwrap_or_else(Utc::now)
        .format("%Y%m%d")
        .to_string()
}
fn insert(db: &Connection, entries: &[Duel]) -> Result<(), String> {
    let mut statement = db
        .prepare("INSERT OR REPLACE INTO annual_rating_duels VALUES(?,?,?,?,?,?,?)")
        .map_err(|e| e.to_string())?;
    for e in entries {
        statement
            .execute(params![
                e.id,
                day(e.at),
                e.at,
                e.winner,
                e.loser,
                e.winner_hull,
                e.loser_hull
            ])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
impl Store {
    pub fn prune_annual(&self, now: i64) -> Result<(), String> {
        let tx = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM annual_rating_duels WHERE at<?",
            [now - window()],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM annual_rating_exclusions WHERE at<?",
            [now - window()],
        )
        .map_err(|e| e.to_string())?;
        tx.execute(
            "DELETE FROM annual_rating_days WHERE day<?",
            [day(now - window())],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn annual_day_covered(&self, stamp: &str) -> Option<usize> {
        self.db.query_row("SELECT source_count FROM main.annual_rating_days WHERE day=? AND policy=?",params![stamp,POLICY],|r|r.get::<_,i64>(0).map(|v|v as usize)).ok()
            .or_else(||bundled(&self.db).then(||self.db.query_row("SELECT source_count FROM bundled.annual_rating_days WHERE day=? AND policy=?",params![stamp,POLICY],|r|r.get::<_,i64>(0).map(|v|v as usize)).ok()).flatten())
    }
    #[cfg(test)]
    pub fn add_annual_duels(&self, entries: &[Duel]) -> Result<(), String> {
        let tx = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        insert(&tx, entries)?;
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn apply_annual_delta(&self, delta: &crate::annual_rating::Delta) -> Result<(), String> {
        let tx = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        for e in &delta.rows {
            tx.execute("DELETE FROM annual_rating_exclusions WHERE id=?", [e.id])
                .map_err(|e| e.to_string())?;
        }
        insert(&tx, &delta.rows)?;
        for (identity, at) in &delta.excluded {
            tx.execute("DELETE FROM annual_rating_duels WHERE id=?", [identity])
                .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT OR REPLACE INTO annual_rating_exclusions VALUES(?,?)",
                params![identity, at],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn replace_annual_day(
        &self,
        stamp: &str,
        source_count: usize,
        entries: &[Duel],
    ) -> Result<(), String> {
        if entries.iter().any(|e| day(e.at) != stamp) {
            return Err("Дата исхода SOLO отличается от даты архива".into());
        }
        let tx = self.db.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM annual_rating_duels WHERE day=?", [stamp])
            .map_err(|e| e.to_string())?;
        insert(&tx, entries)?;
        tx.execute(
            "INSERT OR REPLACE INTO annual_rating_days VALUES(?,?,?)",
            params![stamp, source_count as i64, POLICY],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    }
    fn annual_rows(
        &self,
        character: Option<i64>,
        first: i64,
        last: i64,
    ) -> Result<Vec<Duel>, String> {
        let mut sql="SELECT d.id,d.at,d.winner,d.loser,d.winner_hull,d.loser_hull FROM main.annual_rating_duels d WHERE (?1 IS NULL OR d.winner=?1 OR d.loser=?1) AND d.at>=?2 AND d.at<=?3 AND NOT EXISTS(SELECT 1 FROM main.annual_rating_exclusions e WHERE e.id=d.id)".to_string();
        if bundled(&self.db) {
            sql += " UNION ALL SELECT d.id,d.at,d.winner,d.loser,d.winner_hull,d.loser_hull FROM bundled.annual_rating_duels d WHERE (?1 IS NULL OR d.winner=?1 OR d.loser=?1) AND d.at>=?2 AND d.at<=?3 AND NOT EXISTS(SELECT 1 FROM main.annual_rating_days c WHERE c.day=d.day AND c.policy='pilot-evidence-v3') AND NOT EXISTS(SELECT 1 FROM main.annual_rating_duels m WHERE m.id=d.id) AND NOT EXISTS(SELECT 1 FROM main.annual_rating_exclusions e WHERE e.id=d.id)";
        }
        sql = sql.replace(
            "(?1 IS NULL OR d.winner=?1 OR d.loser=?1)",
            if character.is_some() {
                "(d.winner=?1 OR d.loser=?1)"
            } else {
                "1=1"
            },
        );
        sql += " ORDER BY 2,1";
        let mut statement = self.db.prepare(&sql).map_err(|e| e.to_string())?;
        statement
            .query_map(params![character, first, last], |r| {
                Ok(Duel {
                    id: r.get(0)?,
                    at: r.get(1)?,
                    winner: r.get(2)?,
                    loser: r.get(3)?,
                    winner_hull: r.get(4)?,
                    loser_hull: r.get(5)?,
                })
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }
    pub fn annual_duels(&self, character: i64, now: i64) -> Result<Vec<Duel>, String> {
        self.annual_rows(Some(character), now - window(), now)
    }
    pub fn all_annual_duels(&self, first: i64, last: i64) -> Result<Vec<Duel>, String> {
        self.annual_rows(None, first, last)
    }
    pub fn annual_gap(&self, now: i64) -> Result<i64, String> {
        let now =
            chrono::DateTime::from_timestamp(now, 0).ok_or("Некорректное время истории SOLO")?;
        let first = (now - TimeDelta::days(window_days())).date_naive();
        let end = now.date_naive();
        let mut sql="SELECT day FROM main.annual_rating_days WHERE day>=?1 AND day<=?2 AND policy='pilot-evidence-v3'".to_string();
        if bundled(&self.db) {
            sql += " UNION SELECT day FROM bundled.annual_rating_days WHERE day>=?1 AND day<=?2 AND policy='pilot-evidence-v3'";
        }
        let mut statement = self.db.prepare(&sql).map_err(|e| e.to_string())?;
        let covered: HashSet<String> = statement
            .query_map(
                params![
                    first.format("%Y%m%d").to_string(),
                    end.format("%Y%m%d").to_string()
                ],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        let mut current = first;
        while current < end && covered.contains(&current.format("%Y%m%d").to_string()) {
            current += TimeDelta::days(1);
        }
        Ok(current.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> (Store, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "eve-annual-store-{}-{}",
            std::process::id(),
            crate::diagnostics::trace_id("test")
        ));
        let store = Store::open_with_seed(&dir, None).unwrap();
        (store, dir)
    }
    #[test]
    fn personal_rows_do_not_replace_a_day_and_verified_empty_days_do() {
        let (store, dir) = store();
        store
            .db
            .execute("ATTACH DATABASE ':memory:' AS bundled", [])
            .unwrap();
        store.db.execute_batch("CREATE TABLE bundled.annual_rating_duels(id INTEGER PRIMARY KEY,day TEXT,at INTEGER,winner INTEGER,loser INTEGER,winner_hull INTEGER,loser_hull INTEGER);CREATE TABLE bundled.annual_rating_days(day TEXT PRIMARY KEY,source_count INTEGER,policy TEXT);").unwrap();
        let at = chrono::NaiveDate::from_ymd_opt(2026, 10, 29)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        store
            .db
            .execute(
                "INSERT INTO bundled.annual_rating_duels VALUES(1,'20261029',?1,100,200,587,593)",
                [at],
            )
            .unwrap();
        store
            .db
            .execute(
                "INSERT INTO bundled.annual_rating_duels VALUES(2,'20261029',?1,100,300,587,593)",
                [at],
            )
            .unwrap();
        store
            .db
            .execute(
                "INSERT INTO bundled.annual_rating_days VALUES('20261029',2,?1)",
                [POLICY],
            )
            .unwrap();
        store
            .add_annual_duels(&[Duel {
                id: 3,
                at,
                winner: 100,
                loser: 400,
                winner_hull: 587,
                loser_hull: 593,
            }])
            .unwrap();
        assert_eq!(store.annual_duels(100, at + 1).unwrap().len(), 3);
        assert_eq!(store.annual_day_covered("20261029"), Some(2));
        store
            .add_annual_duels(&[Duel {
                id: 1,
                at,
                winner: 100,
                loser: 200,
                winner_hull: 333,
                loser_hull: 593,
            }])
            .unwrap();
        let rows = store.annual_duels(100, at + 1).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().find(|r| r.id == 1).unwrap().winner_hull, 333);
        store
            .apply_annual_delta(&crate::annual_rating::Delta {
                rows: vec![],
                excluded: vec![(2, at)],
            })
            .unwrap();
        assert_eq!(store.annual_duels(100, at + 1).unwrap().len(), 2);
        // A raw day without zKill flags cannot resurrect an explicit API exclusion.
        store
            .replace_annual_day(
                "20261029",
                1,
                &[Duel {
                    id: 2,
                    at,
                    winner: 100,
                    loser: 300,
                    winner_hull: 587,
                    loser_hull: 593,
                }],
            )
            .unwrap();
        assert!(store.annual_duels(100, at + 1).unwrap().is_empty());
        store.replace_annual_day("20261029", 0, &[]).unwrap();
        assert!(store.annual_duels(100, at + 1).unwrap().is_empty());
        assert_eq!(store.annual_day_covered("20261029"), Some(0));
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn zero_outcome_coverage_counts_and_expired_observations_are_removed() {
        let (store, dir) = store();
        let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap();
        let now = date.and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp();
        let tx = store.db.unchecked_transaction().unwrap();
        for delta in 1..=window_days() {
            tx.execute(
                "INSERT INTO annual_rating_days VALUES(?,0,?)",
                params![
                    (date - TimeDelta::days(delta)).format("%Y%m%d").to_string(),
                    POLICY
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        assert_eq!(
            store.annual_gap(now).unwrap(),
            date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()
        );
        let stale = Duel {
            id: 1,
            at: now - window() - 1,
            winner: 100,
            loser: 200,
            winner_hull: 587,
            loser_hull: 593,
        };
        store.add_annual_duels(&[stale]).unwrap();
        store.prune_annual(now).unwrap();
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM annual_rating_duels", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
