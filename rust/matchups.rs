//! Hull-only observed solo outcomes; daily replacement prevents double counting.
use crate::{
    engine::Catalog,
    model::{array, id},
    rating_scale,
};
use chrono::{NaiveDate, TimeDelta};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const POLICY: &str = "hull-solo-v1";
pub const DAYS: i64 = 365;
pub type Counts = BTreeMap<(i32, i32), (u64, u64)>;

pub fn setup(db: &Connection) -> Result<(), String> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS hull_matchup_days(day TEXT PRIMARY KEY,source_count INTEGER NOT NULL,duels INTEGER NOT NULL,policy TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS hull_matchups_daily(day TEXT NOT NULL,low INTEGER NOT NULL,high INTEGER NOT NULL,low_wins INTEGER NOT NULL,high_wins INTEGER NOT NULL,PRIMARY KEY(day,low,high));
        CREATE INDEX IF NOT EXISTS hull_matchup_pair ON hull_matchups_daily(low,high,day);")
        .map_err(|e| e.to_string())
}
fn bundled(db: &Connection) -> bool {
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM bundled.sqlite_schema WHERE name='hull_matchup_days')",
        [],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}
pub fn covered(db: &Connection, day: &str) -> Option<usize> {
    let local = db
        .query_row(
            "SELECT source_count FROM main.hull_matchup_days WHERE day=?1 AND policy=?2",
            params![day, POLICY],
            |r| r.get::<_, i64>(0).map(|v| v as usize),
        )
        .ok();
    local.or_else(|| {
        bundled(db)
            .then(|| {
                db.query_row(
                    "SELECT source_count FROM bundled.hull_matchup_days WHERE day=?1 AND policy=?2",
                    params![day, POLICY],
                    |r| r.get::<_, i64>(0).map(|v| v as usize),
                )
                .ok()
            })
            .flatten()
    })
}
fn valid_hull(catalog: &Catalog, ship: i32) -> bool {
    let hull = catalog.get(ship);
    id(hull, "category") == 6 && ![29, 31, 237].contains(&id(hull, "group"))
}
pub fn duel(mail: &Value, catalog: &Catalog) -> Option<(i32, i32)> {
    let victim = &mail["victim"];
    let loser = i32::try_from(id(victim, "ship_type_id")).ok()?;
    let victim_id = id(victim, "character_id");
    if victim_id <= 0
        || !valid_hull(catalog, loser)
        || mail["zkb"]["npc"] == true
        || mail["zkb"]["awox"] == true
    {
        return None;
    }
    if array(&mail["zkb"]["labels"])
        .iter()
        .filter_map(Value::as_str)
        .any(|v| ["npc", "awox", "padding", "ganked"].contains(&v))
    {
        return None;
    }
    let mut players = HashMap::new();
    for attacker in array(&mail["attackers"]) {
        let player = id(attacker, "character_id");
        if player > 0 {
            let ship = i32::try_from(id(attacker, "ship_type_id")).ok()?;
            if players
                .insert(player, ship)
                .is_some_and(|previous| previous != ship)
            {
                return None;
            }
        } else if attacker["final_blow"] == true && id(attacker, "corporation_id") == 1000125 {
            return None;
        }
    }
    if players.len() != 1 {
        return None;
    }
    let (attacker, winner) = players.into_iter().next()?;
    (attacker != victim_id && valid_hull(catalog, winner)).then_some((winner, loser))
}
pub fn aggregate(rows: &[Value], catalog: &Catalog) -> Counts {
    let mut counts = Counts::new();
    let mut seen = HashSet::new();
    for row in rows {
        let km = id(row, "killmail_id");
        if km <= 0 || !seen.insert(km) {
            continue;
        }
        if let Some((winner, loser)) = duel(row, catalog) {
            let pair = counts
                .entry((winner.min(loser), winner.max(loser)))
                .or_default();
            if winner <= loser {
                pair.0 += 1;
            } else {
                pair.1 += 1;
            }
        }
    }
    counts
}
pub fn replace_day(
    db: &Connection,
    day: &str,
    source_count: usize,
    counts: &Counts,
) -> Result<(), String> {
    let tx = db.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM hull_matchups_daily WHERE day=?", [day])
        .map_err(|e| e.to_string())?;
    {
        let mut insert = tx
            .prepare("INSERT INTO hull_matchups_daily VALUES(?,?,?,?,?)")
            .map_err(|e| e.to_string())?;
        for ((low, high), (a, b)) in counts {
            insert
                .execute(params![day, low, high, *a as i64, *b as i64])
                .map_err(|e| e.to_string())?;
        }
    }
    let duels: u64 = counts.values().map(|(a, b)| a + b).sum();
    tx.execute(
        "INSERT OR REPLACE INTO hull_matchup_days VALUES(?,?,?,?)",
        params![day, source_count as i64, duels as i64, POLICY],
    )
    .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())
}
fn union(db: &Connection, pair: bool) -> String {
    let filter = if pair { " AND low=?4 AND high=?5" } else { "" };
    let mut sql = format!(
        "SELECT d.day,d.low,d.high,d.low_wins,d.high_wins FROM main.hull_matchups_daily d JOIN main.hull_matchup_days c ON c.day=d.day WHERE d.day>=?1 AND d.day<=?2 AND c.policy=?3{filter}"
    );
    if bundled(db) {
        sql += &format!(
            " UNION ALL SELECT d.day,d.low,d.high,d.low_wins,d.high_wins FROM bundled.hull_matchups_daily d JOIN bundled.hull_matchup_days c ON c.day=d.day WHERE d.day>=?1 AND d.day<=?2 AND c.policy=?3{filter} AND NOT EXISTS(SELECT 1 FROM main.hull_matchup_days m WHERE m.day=d.day AND m.policy=?3)"
        );
    }
    sql
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rules {
    pub boundaries: [u8; 6],
    pub minimum_battles: u64,
    pub reference_pairs: usize,
    pub period_start: String,
    pub period_end: String,
}
impl Rules {
    fn fallback() -> Self {
        let r = &rating_scale::reference().winrate;
        Self {
            boundaries: r.boundaries,
            minimum_battles: r.minimum_battles,
            reference_pairs: 0,
            period_start: String::new(),
            period_end: String::new(),
        }
    }
    pub fn tier(&self, percentage: u8) -> usize {
        self.boundaries.partition_point(|cut| percentage >= *cut)
    }
}
pub fn rules(db: &Connection) -> Rules {
    let local: Option<String> = db
        .query_row(
            "SELECT value FROM main.meta WHERE key='hull_matchup_calibration'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok();
    let bundled: Option<String> = if bundled(db) {
        db.query_row(
            "SELECT value FROM bundled.meta WHERE key='hull_matchup_calibration'",
            [],
            |r| r.get::<_, String>(0),
        )
        .ok()
    } else {
        None
    };
    local
        .into_iter()
        .chain(bundled)
        .filter_map(|s| serde_json::from_str::<Rules>(&s).ok())
        .find(|r| {
            r.minimum_battles >= 50
                && r.boundaries.windows(2).all(|p| p[0] < p[1])
                && r.tier(50) == 3
        })
        .unwrap_or_else(Rules::fallback)
}
fn period(end: NaiveDate) -> (String, String) {
    (
        (end - TimeDelta::days(DAYS - 1))
            .format("%Y%m%d")
            .to_string(),
        end.format("%Y%m%d").to_string(),
    )
}
#[derive(Clone, Debug, Serialize)]
pub struct Statistic {
    pub wins: u64,
    pub losses: u64,
    pub battles: u64,
    pub percent: Option<u8>,
    pub tier: Option<usize>,
    pub mirror: bool,
    pub own_ship: String,
    pub enemy_ship: String,
    pub minimum_battles: u64,
}
impl Statistic {
    pub fn hint(&self) -> String {
        let mut hint = format!(
            "{} против {} · последние 365 полных дней\n",
            self.own_ship, self.enemy_ship
        );
        if self.mirror {
            hint += &format!("Одинаковые корпуса · {} боёв", self.battles);
        } else {
            hint += &format!(
                "{} побед / {} поражений · {} боёв",
                self.wins, self.losses, self.battles
            );
        }
        if self.percent.is_none() {
            hint += &format!(
                "\nМало данных: нужно хотя бы {} исходов",
                self.minimum_battles
            );
        }
        hint + "\nОдин атакующий игрок. Сборки не учитываются. Бои без потери корабля не входят."
    }
}
pub fn lookup(
    db: &Connection,
    own: i32,
    enemy: i32,
    catalog: &Catalog,
    end: NaiveDate,
) -> Result<Statistic, String> {
    let (start, end) = period(end);
    let sql = format!(
        "SELECT coalesce(sum(low_wins),0),coalesce(sum(high_wins),0) FROM ({})",
        union(db, true)
    );
    let (low_wins, high_wins): (u64, u64) = db
        .query_row(
            &sql,
            params![start, end, POLICY, own.min(enemy), own.max(enemy)],
            |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
        )
        .map_err(|e| e.to_string())?;
    let mirror = own == enemy;
    let (wins, losses) = if own <= enemy {
        (low_wins, high_wins)
    } else {
        (high_wins, low_wins)
    };
    let battles = wins + losses;
    let rules = rules(db);
    let percent = (battles >= rules.minimum_battles).then(|| {
        if mirror {
            50
        } else {
            (100. * wins as f64 / battles as f64).round() as u8
        }
    });
    Ok(Statistic {
        wins,
        losses,
        battles,
        percent,
        tier: percent.map(|p| rules.tier(p)),
        mirror,
        own_ship: catalog.name(own).into(),
        enemy_ship: catalog.name(enemy).into(),
        minimum_battles: rules.minimum_battles,
    })
}
fn quantile(values: &[f64], p: f64) -> f64 {
    let position = (values.len() - 1) as f64 * p;
    let lo = position.floor() as usize;
    let hi = (lo + 1).min(values.len() - 1);
    values[lo] + (values[hi] - values[lo]) * (position - lo as f64)
}
pub fn recalibrate(db: &Connection, end: NaiveDate) -> Result<Rules, String> {
    let (start, finish) = period(end);
    let sql = format!(
        "SELECT low,high,sum(low_wins),sum(high_wins) FROM ({}) GROUP BY low,high",
        union(db, false)
    );
    let mut statement = db.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = statement
        .query_map(params![start, finish, POLICY], |r| {
            Ok((
                r.get::<_, i32>(0)?,
                r.get::<_, i32>(1)?,
                r.get::<_, i64>(2)? as u64,
                r.get::<_, i64>(3)? as u64,
            ))
        })
        .map_err(|e| e.to_string())?;
    let minimum_battles = rating_scale::reference().winrate.minimum_battles;
    let mut deviations = Vec::new();
    for row in rows {
        let (a, b, w, l) = row.map_err(|e| e.to_string())?;
        if a != b && w + l >= minimum_battles && w.min(l) >= 5 {
            deviations.push((100. * w as f64 / (w + l) as f64 - 50.).abs());
        }
    }
    // A sparse or partial update must not destroy a usable color reference.
    if deviations.len() < 30 {
        return Ok(rules(db));
    }
    deviations.sort_by(f64::total_cmp);
    let r = [
        quantile(&deviations, 0.25),
        quantile(&deviations, 0.5),
        quantile(&deviations, 0.75),
    ];
    let boundaries = [
        50. - r[2],
        50. - r[1],
        50. - r[0],
        50. + r[0],
        50. + r[1],
        50. + r[2],
    ]
    .map(|v| v.round().clamp(0., 100.) as u8);
    let updated = Rules {
        boundaries,
        minimum_battles,
        reference_pairs: deviations.len(),
        period_start: (end - TimeDelta::days(DAYS - 1)).to_string(),
        period_end: end.to_string(),
    };
    if updated.boundaries.windows(2).all(|p| p[0] < p[1]) && updated.tier(50) == 3 {
        db.execute(
            "INSERT OR REPLACE INTO main.meta VALUES('hull_matchup_calibration',?)",
            [json!(updated).to_string()],
        )
        .map_err(|e| e.to_string())?;
        Ok(updated)
    } else {
        Ok(rules(db))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn database() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT)")
            .unwrap();
        setup(&db).unwrap();
        db
    }
    #[test]
    fn solo_filter_checks_both_hulls_and_deduplicates_records() {
        let c = Catalog::load().unwrap();
        let a = c.ship_id("Harpy").unwrap();
        let b = c.ship_id("Stabber").unwrap();
        let row = json!({"killmail_id":1,"victim":{"character_id":10,"ship_type_id":b},"attackers":[{"character_id":20,"ship_type_id":a},{"corporation_id":123}]});
        assert_eq!(duel(&row, &c), Some((a, b)));
        assert_eq!(
            aggregate(&[row.clone(), row.clone()], &c)
                .values()
                .map(|(a, b)| a + b)
                .sum::<u64>(),
            1
        );
        let mut fleet = row.clone();
        fleet["attackers"]
            .as_array_mut()
            .unwrap()
            .push(json!({"character_id":30,"ship_type_id":a}));
        assert!(duel(&fleet, &c).is_none());
        let mut pod = row.clone();
        pod["attackers"][0]["ship_type_id"] = json!(670);
        assert!(duel(&pod, &c).is_none());
        let mut missing = row;
        missing["attackers"][0]["ship_type_id"] = Value::Null;
        assert!(duel(&missing, &c).is_none());
    }
    #[test]
    fn replacing_a_day_is_idempotent_and_rates_use_own_hull_perspective() {
        let db = database();
        let c = Catalog::load().unwrap();
        let a = c.ship_id("Harpy").unwrap();
        let b = c.ship_id("Stabber").unwrap();
        let key = (a.min(b), a.max(b));
        let counts = Counts::from([(key, (30, 20))]);
        replace_day(&db, "20261029", 200, &counts).unwrap();
        replace_day(&db, "20261029", 200, &counts).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let low = lookup(&db, key.0, key.1, &c, end).unwrap();
        let high = lookup(&db, key.1, key.0, &c, end).unwrap();
        assert_eq!(
            (low.battles, low.percent, high.percent),
            (50, Some(60), Some(40))
        );
        replace_day(&db, "20261029", 200, &Counts::new()).unwrap();
        assert_eq!(lookup(&db, key.0, key.1, &c, end).unwrap().percent, None);
        assert_eq!(covered(&db, "20261029"), Some(200));
    }
    #[test]
    fn insufficient_sample_is_unknown_and_mirror_pair_is_fifty() {
        let db = database();
        let c = Catalog::load().unwrap();
        let a = c.ship_id("Harpy").unwrap();
        let b = c.ship_id("Stabber").unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        replace_day(
            &db,
            "20261029",
            200,
            &Counts::from([((a.min(b), a.max(b)), (49, 0)), ((a, a), (51, 0))]),
        )
        .unwrap();
        assert_eq!(lookup(&db, a, b, &c, end).unwrap().percent, None);
        let mirror = lookup(&db, a, a, &c, end).unwrap();
        assert_eq!((mirror.battles, mirror.percent), (51, Some(50)));
        assert_eq!(
            lookup(&db, a, b, &c, end + TimeDelta::days(365))
                .unwrap()
                .battles,
            0
        );
    }
    #[test]
    fn complete_local_day_replaces_all_seed_pairs_including_removed_pairs() {
        let db = database();
        db.execute_batch("ATTACH DATABASE ':memory:' AS bundled;
            CREATE TABLE bundled.hull_matchup_days(day TEXT PRIMARY KEY,source_count INTEGER,duels INTEGER,policy TEXT);
            CREATE TABLE bundled.hull_matchups_daily(day TEXT,low INTEGER,high INTEGER,low_wins INTEGER,high_wins INTEGER);
            CREATE TABLE bundled.meta(key TEXT,value TEXT);").unwrap();
        let c = Catalog::load().unwrap();
        let a = c.ship_id("Harpy").unwrap();
        let b = c.ship_id("Stabber").unwrap();
        db.execute(
            "INSERT INTO bundled.hull_matchup_days VALUES('20261029',100,50,?)",
            [POLICY],
        )
        .unwrap();
        db.execute(
            "INSERT INTO bundled.hull_matchups_daily VALUES('20261029',?,?,30,20)",
            params![a.min(b), a.max(b)],
        )
        .unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        assert_eq!(lookup(&db, a, b, &c, end).unwrap().battles, 50);
        replace_day(&db, "20261029", 101, &Counts::new()).unwrap();
        assert_eq!(lookup(&db, a, b, &c, end).unwrap().battles, 0);
        assert_eq!(covered(&db, "20261029"), Some(101));
    }
    #[test]
    fn a_database_update_recalibrates_colors_with_fifty_always_green() {
        let db = database();
        let end = NaiveDate::from_ymd_opt(2026, 10, 29).unwrap();
        let first: Counts = (0..40)
            .map(|i| ((1000 + i, 2000 + i), (10 + i as u64, 90 - i as u64)))
            .collect();
        replace_day(&db, "20261029", 1000, &first).unwrap();
        let before = recalibrate(&db, end).unwrap();
        assert_eq!(before.reference_pairs, 40);
        assert_eq!(before.tier(50), 3);
        let second: Counts = (0..40)
            .map(|i| ((1000 + i, 2000 + i), (25 + i as u64, 75 - i as u64)))
            .collect();
        replace_day(&db, "20261029", 1000, &second).unwrap();
        let after = recalibrate(&db, end).unwrap();
        assert_ne!(before.boundaries, after.boundaries);
        assert_eq!(rules(&db).boundaries, after.boundaries);
        assert_eq!(after.tier(50), 3);
    }
}
