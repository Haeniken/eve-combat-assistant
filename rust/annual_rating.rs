//! Annual player strength with jointly learned hull context and bounded evidence.
use crate::{
    engine::Catalog,
    model::{array, id, n},
};
use chrono::{TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    sync::{Arc, OnceLock, RwLock},
};

pub fn window_days() -> i64 {
    model().window_days
}
pub fn window() -> i64 {
    window_days() * 86400
}
fn default_window_days() -> i64 {
    365
}
type Player = (i64, f64, u64, u64, u32, i64);

#[derive(Clone, Serialize, Deserialize)]
struct Specification {
    player_penalty: f64,
    half_life_days: f64,
    hull_penalty: f64,
    pair_penalty: f64,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Model {
    schema: u32,
    version: String,
    policy: String,
    period_start: String,
    period_end: String,
    anchor: i64,
    #[serde(default = "default_window_days")]
    window_days: i64,
    specification: Specification,
    minimum_battles: u64,
    minimum_opponents: u32,
    repeat_pair_day_cap: u32,
    calibration_knots: Vec<[f64; 2]>,
    players: Vec<Player>,
    hulls: Vec<(i32, f64)>,
    pairs: Vec<(i32, i32, f64)>,
    combat_items: Vec<i32>,
    #[serde(default)]
    excluded_hulls: Vec<i32>,
    #[serde(flatten)]
    extra: serde_json::Map<String, Value>,
}
#[path = "pilot_model_fit.rs"]
mod fit;
static MODEL: OnceLock<RwLock<Arc<Model>>> = OnceLock::new();
fn validate(m: &Model) -> Result<(), String> {
    if m.schema != 1
        || m.policy != "pilot-evidence-v3"
        || !(1..=3660).contains(&m.window_days)
        || !m.specification.player_penalty.is_finite()
        || m.specification.player_penalty <= 0.
        || !m.specification.hull_penalty.is_finite()
        || m.specification.hull_penalty <= 0.
        || !m.specification.pair_penalty.is_finite()
        || m.specification.pair_penalty <= 0.
        || m.specification.half_life_days != 0.
        || m.calibration_knots.len() < 100
        || m.minimum_battles == 0
        || m.minimum_opponents == 0
        || !m.players.windows(2).all(|w| w[0].0 < w[1].0)
        || !m.hulls.windows(2).all(|w| w[0].0 < w[1].0)
        || !m
            .pairs
            .windows(2)
            .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1))
        || !m
            .calibration_knots
            .windows(2)
            .all(|w| w[0][0] < w[1][0] && w[0][1] <= w[1][1])
        || m.players.iter().any(|p| !p.1.is_finite())
    {
        return Err("Некорректная модель SOLO".into());
    }
    Ok(())
}
fn decode(bytes: &[u8]) -> Result<Model, String> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .take(64 * 1024 * 1024)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    let model: Model = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    validate(&model)?;
    Ok(model)
}
fn state() -> &'static RwLock<Arc<Model>> {
    MODEL.get_or_init(|| {
        RwLock::new(Arc::new(
            decode(include_bytes!("../resources/pilot-rating-model.json.gz"))
                .expect("bundled SOLO model"),
        ))
    })
}
fn model() -> Arc<Model> {
    state().read().unwrap_or_else(|e| e.into_inner()).clone()
}
pub fn version() -> String {
    model().version.clone()
}
pub fn load_local(directory: &std::path::Path) -> Result<(), String> {
    let path = directory.join("pilot-rating-model.json.gz");
    if !path.exists() {
        return Ok(());
    }
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("Слишком большой файл модели SOLO".into());
    }
    let local = decode(&std::fs::read(path).map_err(|e| e.to_string())?)?;
    if local.anchor >= model().anchor {
        *state().write().map_err(|e| e.to_string())? = Arc::new(local);
    }
    Ok(())
}
pub fn rebuild(
    rows: Vec<Duel>,
    directory: &std::path::Path,
    end: chrono::NaiveDate,
    cancel: &std::sync::atomic::AtomicBool,
    cold: bool,
    progress: &mut dyn FnMut(usize),
) -> Result<usize, String> {
    use std::io::Write;
    let template = if cold {
        let mut m = (*model()).clone();
        for p in &mut m.players {
            p.1 = 0.;
        }
        for h in &mut m.hulls {
            h.1 = 0.;
        }
        for p in &mut m.pairs {
            p.2 = 0.;
        }
        Arc::new(m)
    } else {
        model()
    };
    let updated = fit::train(rows, &template, end, cancel, progress)?;
    validate(&updated)?;
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("Пересчёт SOLO остановлен".into());
    }
    let mut zipped = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    serde_json::to_writer(&mut zipped, &updated).map_err(|e| e.to_string())?;
    let bytes = zipped.finish().map_err(|e| e.to_string())?;
    let temporary = directory.join("pilot-rating-model.json.gz.new");
    let mut file = std::fs::File::create(&temporary).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| e.to_string())?;
    drop(file);
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        let _ = std::fs::remove_file(temporary);
        return Err("Пересчёт SOLO остановлен".into());
    }
    std::fs::rename(temporary, directory.join("pilot-rating-model.json.gz"))
        .map_err(|e| e.to_string())?;
    let count = updated
        .players
        .iter()
        .filter(|p| p.2 + p.3 >= updated.minimum_battles && p.4 >= updated.minimum_opponents)
        .count();
    *state().write().map_err(|e| e.to_string())? = Arc::new(updated);
    Ok(count)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Estimate {
    pub character_id: i64,
    pub model: String,
    pub checked_at: i64,
    pub period_start: String,
    pub period_end: String,
    pub source: String,
    pub strength: f64,
    pub wins: u64,
    pub losses: u64,
    pub opponents: u32,
    pub score: Option<u16>,
}
impl Model {
    fn player(&self, character: i64) -> Option<&Player> {
        self.players
            .binary_search_by_key(&character, |p| p.0)
            .ok()
            .map(|i| &self.players[i])
    }
    fn hull(&self, ship: i32) -> f64 {
        self.hulls
            .binary_search_by_key(&ship, |p| p.0)
            .ok()
            .map(|i| self.hulls[i].1)
            .unwrap_or(0.)
    }
    fn offset(&self, own: i32, other: i32, opponent: i64) -> f64 {
        let mut context =
            self.hull(own) - self.hull(other) - self.player(opponent).map(|p| p.1).unwrap_or(0.);
        if own != other {
            let pair = (own.min(other), own.max(other));
            if let Ok(i) = self.pairs.binary_search_by_key(&pair, |p| (p.0, p.1)) {
                context += self.pairs[i].2 * if own < other { 1. } else { -1. };
            }
        }
        context
    }
    fn score(&self, strength: f64, wins: u64, losses: u64, opponents: u32) -> Option<u16> {
        if !strength.is_finite()
            || wins.checked_add(losses)? < self.minimum_battles
            || opponents < self.minimum_opponents
        {
            return None;
        }
        let knots = &self.calibration_knots;
        let i = knots.partition_point(|p| p[0] < strength);
        let percentile = if i == knots.len() {
            1.
        } else if knots[i][0] == strength {
            knots[i][1]
        } else if i == 0 {
            0.
        } else {
            let [left, low] = knots[i - 1];
            let [right, high] = knots[i];
            low + (high - low) * (strength - left) / (right - left)
        };
        Some((1000. + (7999. * percentile.clamp(0., 1.)).floor()).clamp(1000., 8999.) as u16)
    }
}
pub fn prepared(character: i64) -> Option<Estimate> {
    let m = model();
    let p = m.player(character)?;
    Some(Estimate {
        character_id: character,
        model: m.version.clone(),
        checked_at: m.anchor,
        period_start: m.period_start.clone(),
        period_end: m.period_end.clone(),
        source: "prepared".into(),
        strength: p.1,
        wins: p.2,
        losses: p.3,
        opponents: p.4,
        score: m.score(p.1, p.2, p.3, p.4),
    })
}

#[derive(Clone, Debug)]
pub struct Duel {
    pub id: i64,
    pub at: i64,
    pub winner: i64,
    pub loser: i64,
    pub winner_hull: i32,
    pub loser_hull: i32,
}
pub fn collect(rows: &[Value], catalog: &Catalog) -> Vec<Duel> {
    let m = model();
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for mail in rows {
        let identity = id(mail, "killmail_id");
        if identity <= 0 || !seen.insert(identity) {
            continue;
        }
        let Some(at) = mail["killmail_time"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.timestamp())
        else {
            continue;
        };
        let Some((winning_hull, losing_hull)) = crate::matchups::duel(mail, catalog) else {
            continue;
        };
        if m.excluded_hulls.binary_search(&winning_hull).is_ok()
            || m.excluded_hulls.binary_search(&losing_hull).is_ok()
        {
            continue;
        }
        let victim = &mail["victim"];
        let players: Vec<_> = array(&mail["attackers"])
            .iter()
            .filter(|a| id(a, "character_id") > 0)
            .collect();
        let attacker = players[0];
        let winner = id(attacker, "character_id");
        let loser = id(victim, "character_id");
        let corp = id(attacker, "corporation_id");
        if corp > 0 && corp == id(victim, "corporation_id") {
            continue;
        }
        let player_damage: f64 = players.iter().map(|a| n(a, "damage_done").max(0.)).sum();
        let npc_damage: f64 = array(&mail["attackers"])
            .iter()
            .filter(|a| id(a, "character_id") <= 0)
            .map(|a| n(a, "damage_done").max(0.))
            .sum();
        if player_damage <= 0. || npc_damage * 9. > player_damage {
            continue;
        }
        let armed = array(&victim["items"]).iter().any(|item| {
            let flag = id(item, "flag");
            ((27..=34).contains(&flag) || flag == 87)
                && i32::try_from(id(item, "item_type_id"))
                    .ok()
                    .is_some_and(|t| m.combat_items.binary_search(&t).is_ok())
        });
        if !armed {
            continue;
        }
        entries.push(Duel {
            id: identity,
            at,
            winner,
            loser,
            winner_hull: winning_hull,
            loser_hull: losing_hull,
        });
    }
    entries
}

struct Observation {
    id: i64,
    opponent: i64,
    at: i64,
    won: bool,
    offset: f64,
}
pub struct Delta {
    pub rows: Vec<Duel>,
    pub excluded: Vec<(i64, i64)>,
}
pub struct Evidence {
    character: i64,
    model: Arc<Model>,
    now: i64,
    seen: HashSet<i64>,
    observations: Vec<Observation>,
}
impl Evidence {
    pub fn new(character: i64, now: i64) -> Self {
        Self {
            character,
            model: model(),
            now,
            seen: HashSet::new(),
            observations: Vec::new(),
        }
    }
    pub fn len(&self) -> usize {
        self.observations.len()
    }
    pub fn extend(&mut self, rows: &[Value], catalog: &Catalog) -> Delta {
        let entries: Vec<_> = collect(rows, catalog)
            .into_iter()
            .filter(|e| e.at >= self.now - window() && e.at <= self.now)
            .collect();
        let accepted: HashSet<_> = entries.iter().map(|e| e.id).collect();
        let observed: HashMap<_, _> = rows
            .iter()
            .filter_map(|r| {
                let identity = id(r, "killmail_id");
                let at = r["killmail_time"]
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())?
                    .timestamp();
                (identity > 0 && at >= self.now - window() && at <= self.now)
                    .then_some((identity, at))
            })
            .collect();
        self.observations.retain(|e| !observed.contains_key(&e.id));
        self.seen
            .retain(|identity| !observed.contains_key(identity));
        self.extend_duels(&entries);
        Delta {
            rows: entries,
            excluded: observed
                .into_iter()
                .filter(|(identity, _)| !accepted.contains(identity))
                .collect(),
        }
    }
    pub fn extend_duels(&mut self, entries: &[Duel]) {
        let m = &self.model;
        for row in entries {
            if row.at < self.now - window()
                || row.at > self.now
                || !self.seen.insert(row.id)
                || (row.winner != self.character && row.loser != self.character)
                || m.excluded_hulls.binary_search(&row.winner_hull).is_ok()
                || m.excluded_hulls.binary_search(&row.loser_hull).is_ok()
            {
                continue;
            }
            let won = row.winner == self.character;
            let opponent = if won { row.loser } else { row.winner };
            let (own, other) = if won {
                (row.winner_hull, row.loser_hull)
            } else {
                (row.loser_hull, row.winner_hull)
            };
            self.observations.push(Observation {
                id: row.id,
                opponent,
                at: row.at,
                won,
                offset: m.offset(own, other, opponent),
            });
        }
    }
    pub fn finish(self) -> Estimate {
        let m = &self.model;
        let mut repeats = HashMap::new();
        let mut opponents = HashSet::new();
        let mut wins = 0;
        for row in &self.observations {
            *repeats
                .entry((row.opponent, row.at.div_euclid(86400)))
                .or_insert(0_u32) += 1;
            opponents.insert(row.opponent);
            wins += u64::from(row.won);
        }
        let rows: Vec<_> = self
            .observations
            .iter()
            .map(|row| {
                let count = repeats[&(row.opponent, row.at.div_euclid(86400))];
                let mut weight = (m.repeat_pair_day_cap as f64 / count as f64).min(1.);
                if m.specification.half_life_days > 0. {
                    weight *= 2_f64.powf(
                        -((self.now - row.at) as f64) / (m.specification.half_life_days * 86400.),
                    );
                }
                (row.won, row.offset, weight)
            })
            .collect();
        let strength = posterior(&rows, m.specification.player_penalty);
        let losses = self.observations.len() as u64 - wins;
        let opponents = opponents.len() as u32;
        let date = chrono::DateTime::from_timestamp(self.now, 0)
            .unwrap_or_else(Utc::now)
            .date_naive();
        Estimate {
            character_id: self.character,
            model: m.version.clone(),
            checked_at: self.now,
            period_start: (date - TimeDelta::days(window_days())).to_string(),
            period_end: date.to_string(),
            source: "complete_personal_period".into(),
            strength,
            wins,
            losses,
            opponents,
            score: m.score(strength, wins, losses, opponents),
        }
    }
}
fn sigmoid(value: f64) -> f64 {
    1. / (1. + (-value.clamp(-40., 40.)).exp())
}
fn posterior(rows: &[(bool, f64, f64)], penalty: f64) -> f64 {
    let mut value = 0.;
    for _ in 0..80 {
        let (mut gradient, mut hessian) = (penalty * value, penalty);
        for (won, offset, weight) in rows {
            let p = sigmoid(value + offset);
            gradient += weight * (p - u8::from(*won) as f64);
            hessian += weight * p * (1. - p);
        }
        let step = (gradient / hessian).clamp(-2., 2.);
        value = (value - step).clamp(-12., 12.);
        if step.abs() < 1e-9 {
            break;
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regularized_strength_uses_opponents_and_does_not_reward_volume_at_fifty_percent() {
        let balanced: Vec<_> = (0..100).map(|i| (i % 2 == 0, 0., 1.)).collect();
        assert!(posterior(&balanced, 4.).abs() < 1e-9);
        assert!(posterior(&balanced.repeat(10), 4.).abs() < 1e-9);
        let weak: Vec<_> = (0..30).map(|i| (i < 20, 1., 1.)).collect();
        let strong: Vec<_> = (0..30).map(|i| (i < 20, -1., 1.)).collect();
        assert!(posterior(&strong, 4.) > posterior(&weak, 4.));
        let few = vec![(true, 0., 1.); 5];
        assert!(posterior(&few, 4.) < posterior(&few.repeat(10), 4.));
    }
    #[test]
    fn bundled_model_has_the_configured_window_and_separates_insufficient_evidence() {
        let m = model();
        let start = chrono::NaiveDate::parse_from_str(&m.period_start, "%Y-%m-%d").unwrap();
        let end = chrono::NaiveDate::parse_from_str(&m.period_end, "%Y-%m-%d").unwrap();
        assert_eq!((end - start).num_days() + 1, m.window_days);
        assert_eq!((m.minimum_battles, m.minimum_opponents), (5, 3));
        assert!(m.players.len() > 10000);
        assert_eq!(m.score(0., 1, 1, 2), None);
        assert_eq!(m.score(0., 2, 2, 3), None);
        assert_eq!(m.score(0., 3, 2, 2), None);
        assert!(m.score(0., 3, 2, 3).is_some());
        assert_eq!(m.score(0., 100, 0, 1), None);
        assert!(m.score(-100., 100, 100, 10).is_some_and(|r| r == 1000));
        assert!(m.score(100., 100, 100, 10).is_some_and(|r| r == 8999));
        assert!(!m.excluded_hulls.is_empty());
        assert!(
            m.calibration_knots
                .windows(2)
                .all(|w| w[0][0] < w[1][0] && w[0][1] <= w[1][1])
        );
        let mut tiers = [0; 7];
        for p in &m.players {
            if let Some(score) = m.score(p.1, p.2, p.3, p.4) {
                tiers[crate::rating_scale::solo_tier(score)] += 1;
            }
        }
        assert!(tiers.into_iter().all(|n| n > 0));
    }
    #[test]
    fn visible_weapons_and_refreshed_exclusions_match_evidence_rules() {
        let c = Catalog::load().unwrap();
        let m = model();
        let now = Utc::now().timestamp();
        let gun = *c.names.get("125mm gatling autocannon ii").unwrap();
        let raw = serde_json::json!({"killmail_id":1,"killmail_time":chrono::DateTime::from_timestamp(now-10,0).unwrap().to_rfc3339(),
            "victim":{"character_id":200,"corporation_id":2,"ship_type_id":587,"items":[{"item_type_id":gun,"flag":27}]},
            "attackers":[{"character_id":100,"corporation_id":1,"ship_type_id":593,"damage_done":100}]});
        assert_eq!(collect(std::slice::from_ref(&raw), &c).len(), 1);
        for flag in [5, 11, 19] {
            let mut row = raw.clone();
            row["victim"]["items"][0]["flag"] = flag.into();
            assert!(collect(&[row], &c).is_empty());
        }
        for winner in [true, false] {
            let mut row = raw.clone();
            if winner {
                row["attackers"][0]["ship_type_id"] = m.excluded_hulls[0].into();
            } else {
                row["victim"]["ship_type_id"] = m.excluded_hulls[0].into();
            }
            assert!(collect(&[row], &c).is_empty());
        }
        let mut evidence = Evidence::new(100, now);
        evidence.extend_duels(&[Duel {
            id: 1,
            at: now - 10,
            winner: 100,
            loser: 200,
            winner_hull: 593,
            loser_hull: 587,
        }]);
        let mut excluded = raw.clone();
        excluded["victim"]["items"] = serde_json::json!([]);
        assert_eq!(evidence.extend(&[excluded], &c).excluded.len(), 1);
        assert_eq!(evidence.len(), 0);
        evidence.extend(std::slice::from_ref(&raw), &c);
        evidence.extend(&[raw], &c);
        assert_eq!(evidence.len(), 1);
    }
}
