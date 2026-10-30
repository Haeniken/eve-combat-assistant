//! Annual solo player strength and historical group participation are separate.
use crate::{
    model::{id, n},
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ACTIVITY_WINDOW: i64 = 3 * 86400;
pub const ACTIVITY_TTL: i64 = 3600;
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActivityAge {
    Day,
    ThreeDays,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecentActivity {
    #[serde(default)]
    pub solo_at: Option<i64>,
    pub group_at: Option<i64>,
    pub blob_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActivityCache {
    pub character_id: i64,
    pub checked_at: i64,
    pub activity: RecentActivity,
}
impl RecentActivity {
    pub fn from_kills(rows: &[Value], character: i64, now: i64) -> Self {
        let mut activity = Self::default();
        for row in rows.iter().take(200) {
            let Some(at) = row["killmail_time"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|v| v.timestamp())
            else {
                continue;
            };
            if !(0..ACTIVITY_WINDOW).contains(&now.saturating_sub(at))
                || row["zkb"]["npc"] == true
                || id(&row["victim"], "character_id") == character
            {
                continue;
            }
            let players: std::collections::HashSet<_> = crate::model::array(&row["attackers"])
                .iter()
                .map(|a| id(a, "character_id"))
                .filter(|id| *id > 0)
                .collect();
            if character <= 0 || !players.contains(&character) {
                continue;
            }
            let labels: Vec<_> = crate::model::array(&row["zkb"]["labels"])
                .iter()
                .filter_map(Value::as_str)
                .collect();
            if labels.contains(&"npc") || labels.contains(&"padding") {
                continue;
            }
            let solo = row["zkb"]["solo"] == true || labels.contains(&"solo");
            if solo {
                if players.len() == 1 && id(&row["victim"], "character_id") > 0 {
                    activity.solo_at = Some(activity.solo_at.unwrap_or(at).max(at));
                }
                continue;
            }
            // Current zKillboard labels are exclusive attacker-count buckets.
            let bucket = labels
                .iter()
                .filter_map(|s| {
                    s.strip_prefix("#:")?
                        .trim_end_matches('+')
                        .parse::<usize>()
                        .ok()
                })
                .max();
            let count = bucket.unwrap_or(players.len());
            let pvp = row["zkb"]["npc"] == false
                || labels.contains(&"pvp")
                || id(&row["victim"], "character_id") > 0;
            if pvp && count >= 2 {
                activity.group_at = Some(activity.group_at.unwrap_or(at).max(at));
            }
            if pvp && count >= 10 {
                activity.blob_at = Some(activity.blob_at.unwrap_or(at).max(at));
            }
        }
        activity
    }
    pub fn solo_within_day(&self, now: i64) -> bool {
        self.solo_at
            .and_then(|at| now.checked_sub(at))
            .is_some_and(|age| (0..86400).contains(&age))
    }
    pub fn merge(&mut self, other: Self) {
        self.solo_at = self.solo_at.into_iter().chain(other.solo_at).max();
        self.group_at = self.group_at.into_iter().chain(other.group_at).max();
        self.blob_at = self.blob_at.into_iter().chain(other.blob_at).max();
    }
}
pub fn cached_activity(store: &Store, character: i64) -> Option<ActivityCache> {
    let entry: ActivityCache =
        serde_json::from_value(store.meta(&format!("pilot_activity:v1:{character}"))?).ok()?;
    (entry.character_id == character).then_some(entry)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rating {
    #[serde(default = "stats_available_default")]
    pub stats_available: bool,
    pub character_id: i64,
    pub solo_kills: u64,
    pub solo_losses: u64,
    pub kills: u64,
    pub score: Option<u16>,
    pub group_fraction: Option<f64>,
    pub large_group_fraction: Option<f64>,
    pub average_gang: Option<f64>,
    #[serde(default)]
    pub recent: RecentActivity,
    #[serde(default)]
    pub annual: Option<crate::annual_rating::Estimate>,
}
fn stats_available_default() -> bool {
    true
}
pub fn profile_url(character: i64) -> Option<String> {
    (character > 0).then(|| format!("https://zkillboard.com/character/{character}/"))
}
impl Rating {
    pub fn unavailable(character_id: i64) -> Self {
        Self {
            character_id,
            stats_available: false,
            solo_kills: 0,
            solo_losses: 0,
            kills: 0,
            score: None,
            group_fraction: None,
            large_group_fraction: None,
            average_gang: None,
            recent: RecentActivity::default(),
            annual: None,
        }
    }
    pub fn from_stats(stats: &Value, character_id: i64) -> Result<Self, String> {
        if stats["type"] != "characterID" || id(stats, "id") != character_id || character_id <= 0 {
            return Err("zKillboard не вернул статистику выбранного пилота".into());
        }
        let count = |key: &str, alternate: &str| -> Result<u64, String> {
            let value = stats.get(key).or_else(|| stats.get(alternate));
            match value {
                None | Some(Value::Null) => Ok(0),
                Some(v) => v
                    .as_u64()
                    .filter(|n| *n <= 1_000_000_000)
                    .ok_or_else(|| "Некорректный счётчик статистики zKillboard".into()),
            }
        };
        let kills = count("shipsDestroyed", "shipsDestroyed")?;
        let losses = count("shipsLost", "shipsLost")?;
        let solo_kills = count("soloKills", "shipsDestroyedSolo")?;
        let solo_losses = count("soloLosses", "shipsLostSolo")?;
        if solo_kills > kills || solo_losses > losses {
            return Err("Несогласованные соло-счётчики zKillboard".into());
        }
        let large_keys = ["#:10+", "#:25+", "#:50+", "#:100+", "#:1000+"];
        let large = large_keys
            .iter()
            .any(|key| stats["labels"].get(key).is_some())
            .then(|| {
                large_keys.iter().try_fold(0_u64, |sum, key| {
                    let count = match stats["labels"][key].get("shipsDestroyed") {
                        None | Some(Value::Null) => 0,
                        Some(v) => v.as_u64()?,
                    };
                    sum.checked_add(count)
                })
            })
            .flatten();
        let average = n(stats, "avgGangSize");
        Ok(Self {
            stats_available: true,
            character_id,
            solo_kills,
            solo_losses,
            kills,
            score: None,
            group_fraction: (kills > 0).then(|| 1. - solo_kills as f64 / kills as f64),
            large_group_fraction: large
                .filter(|n| *n <= kills && kills > 0)
                .map(|n| n as f64 / kills as f64),
            average_gang: (average.is_finite() && average > 0.).then_some(average),
            recent: RecentActivity::default(),
            annual: None,
        })
    }
    pub fn tier(&self) -> Option<usize> {
        self.score.map(crate::rating_scale::solo_tier)
    }
    pub fn group_marker(&self) -> Option<&'static str> {
        if self.kills < 20 {
            return None;
        }
        if self.large_group_fraction.is_some_and(|v| v > 0.5) {
            Some("БЛОБ")
        } else if self.group_fraction.is_some_and(|v| v >= 0.9) {
            Some("ГРУППА")
        } else {
            None
        }
    }
    pub fn recent_marker(&self, now: i64) -> Option<(ActivityAge, i64)> {
        let at = match self.group_marker()? {
            "БЛОБ" => self.recent.blob_at?,
            _ => self.recent.group_at?,
        };
        let age = now.checked_sub(at)?;
        match age {
            0..86400 => Some((ActivityAge::Day, age)),
            86400..ACTIVITY_WINDOW => Some((ActivityAge::ThreeDays, age)),
            _ => None,
        }
    }
    pub fn recent_hint(&self, age: i64) -> String {
        let who = if self.group_marker() == Some("БЛОБ") {
            "В блобе"
        } else {
            "В группе"
        };
        let time = match age {
            0..60 => "только что".into(),
            60..3600 => format!("{} мин назад", age / 60),
            3600..86400 => format!("{} ч назад", age / 3600),
            _ => format!("{} дн назад", age / 86400),
        };
        format!("{who} · {time}")
    }
    pub fn hint(&self) -> String {
        if !self.stats_available && self.annual.is_none() {
            return "Статистика zKillboard для персонажа пока недоступна".into();
        }
        let mut hint = if self.score.is_some() {
            "Годовая соло-оценка по результатам против игроков с учётом корпусов.\n"
        } else {
            "Недостаточно соло-данных для оценки.\n"
        }
        .to_string();
        if let Some(annual) = &self.annual {
            hint += &format!(
                "За 365 дней: {} побед · {} потерь\nРазных соперников: {}\n",
                annual.wins, annual.losses, annual.opponents
            );
        }
        if let Some(group) = self.group_fraction {
            hint += &format!("Убийства в группе: {:.0}%\n", group * 100.);
        }
        if let Some(large) = self.large_group_fraction {
            hint += &format!("С 10+ атакующими: {:.0}%\n", large * 100.);
        }
        if let Some(avg) = self.average_gang {
            hint += &format!("Средний размер группы по zKillboard: {avg:.1}\n");
        }
        hint += "Групповые метки — за всё время. Не отражённая в killmail помощь и бои без уничтожения корабля не видны. Групповые убийства не повышают соло-оценку.";
        hint
    }
}

pub fn cached(store: &Store, name: &str) -> Option<Rating> {
    let character = store
        .meta(&format!("character:{}", name.to_lowercase()))?
        .as_i64()?;
    cached_character(store, character)
}
pub fn cached_character(store: &Store, character: i64) -> Option<Rating> {
    let mut rating: Rating = store
        .meta(&format!("pilot_rating:v1:{character}"))
        .and_then(|v| serde_json::from_value(v).ok())
        .or_else(|| {
            (store
                .meta(&format!("annual_rating:v1:{character}"))
                .is_some()
                || crate::annual_rating::prepared(character).is_some())
            .then(|| Rating::unavailable(character))
        })?;
    apply_annual(store, &mut rating);
    if let Some(cached) = cached_activity(store, character) {
        rating.recent = cached.activity;
    }
    (rating.character_id == character).then_some(rating)
}
pub fn apply_annual(store: &Store, rating: &mut Rating) {
    let now = crate::store::timestamp();
    rating.annual = store
        .meta(&format!("annual_rating:v1:{}", rating.character_id))
        .and_then(|v| serde_json::from_value::<crate::annual_rating::Estimate>(v).ok())
        .filter(|e| {
            e.character_id == rating.character_id
                && e.model == crate::annual_rating::version()
                && (0..crate::annual_rating::WINDOW).contains(&now.saturating_sub(e.checked_at))
        })
        .or_else(|| crate::annual_rating::prepared(rating.character_id));
    rating.score = rating.annual.as_ref().and_then(|e| e.score);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn solo_activity_requires_confirmed_player_kill_and_expires_after_a_day() {
        let now = 1_800_000_000;
        let solo = |age| json!({"killmail_time":chrono::DateTime::from_timestamp(now-age,0).unwrap().to_rfc3339(),"victim":{"character_id":456},"attackers":[{"character_id":123}],"zkb":{"solo":true,"npc":false}});
        let valid = solo(3600);
        let mut npc = solo(1);
        npc["zkb"]["npc"] = json!(true);
        let mut other = solo(2);
        other["attackers"] = json!([{"character_id":999}]);
        let mut group = solo(3);
        group["attackers"] = json!([{"character_id":123},{"character_id":999}]);
        let mut loss = solo(4);
        loss["victim"]["character_id"] = json!(123);
        let mut structure = solo(5);
        structure["victim"]["character_id"] = json!(0);
        let mut padding = solo(6);
        padding["zkb"]["labels"] = json!(["solo", "padding"]);
        let mut found = RecentActivity::from_kills(
            &[valid, npc, other, group, loss, structure, padding, solo(-1)],
            123,
            now,
        );
        assert_eq!(found.solo_at, Some(now - 3600));
        assert!(found.group_at.is_none() && found.blob_at.is_none());
        assert!(found.solo_within_day(now));
        assert!(found.solo_within_day(now + 82799));
        assert!(!found.solo_within_day(now + 82800));
        let mut labelled = solo(100);
        labelled["zkb"].as_object_mut().unwrap().remove("solo");
        labelled["zkb"]["labels"] = json!(["pvp", "solo"]);
        found.merge(RecentActivity::from_kills(&[labelled], 123, now));
        assert_eq!(found.solo_at, Some(now - 100));
        let legacy: RecentActivity =
            serde_json::from_value(json!({"group_at":now-200,"blob_at":null})).unwrap();
        assert!(legacy.solo_at.is_none());
        assert_eq!(legacy.group_at, Some(now - 200));
    }
    #[test]
    fn blob_counts_all_exclusive_large_fleet_buckets() {
        let mut value = stats(100, 1, 30);
        value["labels"] = json!({"#:25+":{"shipsDestroyed":40},"#:50+":{"shipsDestroyed":30},"#:100+":{"shipsDestroyed":10}});
        let rating = Rating::from_stats(&value, 123).unwrap();
        assert_eq!(rating.large_group_fraction, Some(0.8));
        assert_eq!(rating.group_marker(), Some("БЛОБ"));
        value["labels"]["#:1000+"] = json!({"shipsDestroyed":30});
        assert_eq!(
            Rating::from_stats(&value, 123)
                .unwrap()
                .large_group_fraction,
            None
        );
    }
    #[test]
    fn recent_activity_requires_matching_pvp_participation_and_ages_without_network() {
        let now = 1_800_000_000;
        let row = |age: i64, count: usize| json!({"killmail_time":chrono::DateTime::from_timestamp(now-age,0).unwrap().to_rfc3339(),"victim":{"character_id":456},"attackers":(0..count).map(|i|json!({"character_id":123+i})).collect::<Vec<_>>(),"zkb":{"npc":false,"solo":false}});
        let mut npc = row(10, 20);
        npc["zkb"]["npc"] = json!(true);
        let mut solo = row(20, 1);
        solo["zkb"]["solo"] = json!(true);
        let mut other = row(30, 10);
        other["attackers"] = json!([{"character_id":999},{"character_id":998}]);
        let mut victim = row(40, 10);
        victim["victim"]["character_id"] = json!(123);
        let mut missing = row(50, 20);
        missing["killmail_time"] = json!("bad timestamp");
        let activity = RecentActivity::from_kills(
            &[
                row(6 * 3600, 2),
                row(2 * 86400, 10),
                row(-10, 30),
                row(ACTIVITY_WINDOW, 30),
                npc,
                solo,
                other,
                victim,
                missing,
            ],
            123,
            now,
        );
        assert_eq!(activity.group_at, Some(now - 6 * 3600));
        assert_eq!(activity.blob_at, Some(now - 2 * 86400));
        let mut rating = Rating::from_stats(&stats(100, 1, 30), 123).unwrap();
        rating.recent = activity.clone();
        assert_eq!(
            rating.recent_marker(now),
            Some((ActivityAge::Day, 6 * 3600))
        );
        assert_eq!(rating.recent_hint(6 * 3600), "В группе · 6 ч назад");
        rating.large_group_fraction = Some(0.8);
        assert_eq!(
            rating.recent_marker(now),
            Some((ActivityAge::ThreeDays, 2 * 86400))
        );
        assert_eq!(rating.recent_hint(2 * 86400), "В блобе · 2 дн назад");
        for (age, expected) in [
            (-1, None),
            (86399, Some(ActivityAge::Day)),
            (86400, Some(ActivityAge::ThreeDays)),
            (259199, Some(ActivityAge::ThreeDays)),
            (259200, None),
        ] {
            rating.recent.blob_at = Some(now - age);
            assert_eq!(rating.recent_marker(now).map(|(p, _)| p), expected);
        }
        let mut old = serde_json::to_value(rating).unwrap();
        old.as_object_mut().unwrap().remove("recent");
        assert!(
            serde_json::from_value::<Rating>(old)
                .unwrap()
                .recent
                .group_at
                .is_none()
        );
    }
    fn stats(kills: u64, wins: u64, losses: u64) -> Value {
        json!({"type":"characterID","id":123,"shipsDestroyed":kills,"shipsLost":losses,"soloKills":wins,"soloLosses":losses})
    }
    #[test]
    fn group_farming_cannot_inflate_solo_rating_and_absence_is_unknown() {
        let a = Rating::from_stats(&stats(100, 50, 20), 123).unwrap();
        let mut blob = stats(10000, 50, 20);
        blob["labels"] = json!({"#:10+":{"shipsDestroyed":500},"#:25+":{"shipsDestroyed":9000}});
        let b = Rating::from_stats(&blob, 123).unwrap();
        assert_eq!(a.score, b.score);
        assert_eq!(b.group_marker(), Some("БЛОБ"));
        assert_eq!(b.large_group_fraction, Some(0.95));
        assert_eq!(
            Rating::from_stats(&stats(10000, 0, 100), 123)
                .unwrap()
                .score,
            None
        );
        assert_eq!(
            Rating::from_stats(&stats(3, 3, 0), 123).unwrap().score,
            None
        );
        assert!(Rating::from_stats(&blob, 124).is_err());
    }
    #[test]
    fn solo_results_control_all_seven_tiers_without_formula_or_age_in_hint() {
        for (score, tier) in [
            (1000, 0),
            (2000, 1),
            (3000, 2),
            (4000, 3),
            (6000, 4),
            (7000, 5),
            (8000, 6),
            (8999, 6),
        ] {
            let mut r = Rating::from_stats(&stats(100, 100, 20), 123).unwrap();
            r.score = Some(score);
            assert_eq!(r.tier(), Some(tier));
            assert!(!r.hint().contains("log") && !r.hint().contains("обновлен"));
        }
        // Lifetime aggregates cannot stand in for complete annual evidence.
        for (wins, losses) in [(30, 10), (300, 100), (1_000_000_000, 0)] {
            assert!(
                Rating::from_stats(&stats(wins, wins, losses), 123)
                    .unwrap()
                    .score
                    .is_none()
            );
        }
    }
}
