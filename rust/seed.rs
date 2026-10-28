//! Read-only starter data shipped next to the executable; never replaces user data.
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize)]
pub struct SeedInfo {
    pub id: String,
    pub period_start: String,
    pub period_end: String,
    pub records: u64,
    pub known_hulls: u64,
    pub hulls_with_records: u64,
    pub prepared: bool,
}
pub fn default_path() -> Option<PathBuf> {
    let path = std::env::current_exe()
        .ok()?
        .parent()?
        .join("seed/seed.sqlite3");
    path.is_file().then_some(path)
}
pub fn attach(db: &Connection, path: &Path) -> Result<SeedInfo, String> {
    let mut uri = url::Url::parse("file:///").map_err(|e| e.to_string())?;
    uri.set_path(&path.to_string_lossy().replace('\\', "/"));
    uri.query_pairs_mut().append_pair("mode", "ro");
    db.execute("ATTACH DATABASE ? AS bundled", [uri.as_str()])
        .map_err(|e| e.to_string())?;
    let get = |key: &str| -> Result<Value, String> {
        let raw: String = db
            .query_row("SELECT value FROM bundled.meta WHERE key=?", [key], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).map_err(|e| e.to_string())
    };
    if get("schema_version")?.as_u64() != Some(1) {
        return Err("Unsupported starter database schema".into());
    }
    let upstream: Value =
        serde_json::from_str(include_str!("../native/upstream.json")).map_err(|e| e.to_string())?;
    if get("sde_build")? != upstream["sde_build"] {
        return Err("Starter database uses another SDE".into());
    }
    if get("complete_days")?.as_u64().unwrap_or(0) == 0 {
        return Err("Starter database is incomplete".into());
    }
    let prepared: bool = db
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM bundled.sqlite_schema WHERE name='populations')",
            [],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let start = get("period_start")?
        .as_str()
        .unwrap_or_default()
        .to_string();
    let end = get("period_end")?.as_str().unwrap_or_default().to_string();
    let built = get("built_at")?.as_str().unwrap_or_default().to_string();
    let records = get("records")?.as_u64().unwrap_or(0);
    Ok(SeedInfo {
        id: format!("{}:{end}:{records}:{built}", upstream["sde_build"]),
        period_start: start,
        period_end: end,
        records,
        known_hulls: get("known_hulls")?.as_u64().unwrap_or(0),
        hulls_with_records: get("hulls_with_records")?.as_u64().unwrap_or(0),
        prepared,
    })
}

/// Release preparation only: precompute default populations without evaluating combat.
pub fn prepare(path: &Path, directory: &Path) -> Result<(), String> {
    let catalog = crate::engine::Catalog::load()?;
    let settings = crate::model::Settings::default();
    let store = crate::store::Store::open_with_seed(directory, Some(path))?;
    if let Some(error) = &store.seed_error {
        return Err(error.clone());
    }
    let mut writer = Connection::open(path).map_err(|e| e.to_string())?;
    writer
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS populations(key TEXT PRIMARY KEY,raw BLOB NOT NULL);",
        )
        .map_err(|e| e.to_string())?;
    let tx = writer.transaction().map_err(|e| e.to_string())?;
    let mut with_variants = 0;
    for (index, (ship, name)) in catalog.ships.iter().enumerate() {
        let population = crate::fits::population(&store.global_mails(*ship)?, &catalog, &settings);
        with_variants += usize::from(!population.variants.is_empty());
        let raw =
            crate::store::compress(&serde_json::to_value(&population).map_err(|e| e.to_string())?)?;
        tx.execute(
            "INSERT OR REPLACE INTO populations VALUES(?,?)",
            rusqlite::params![crate::fits::seed_population_key(*ship, &settings), raw],
        )
        .map_err(|e| e.to_string())?;
        println!(
            "{}",
            serde_json::json!({"position":index+1,"ship":name,"input":population.report.input,"eligible":population.total,"variants":population.variants.len()})
        );
    }
    tx.commit().map_err(|e| e.to_string())?;
    writer
        .execute_batch("PRAGMA journal_mode=DELETE;")
        .map_err(|e| e.to_string())?;
    println!(
        "{}",
        serde_json::json!({"event":"prepared","hulls":catalog.ships.len(),"hulls_with_variants":with_variants})
    );
    Ok(())
}
