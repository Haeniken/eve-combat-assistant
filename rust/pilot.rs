//! Solo evidence and group participation are separate; this is not opponent-based Elo.
use crate::{
    model::{id, n},
    store::Store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Deserialize)]
struct Reference {
    knots: Vec<[f64; 2]>,
}
fn reference() -> &'static Reference {
    static REFERENCE: OnceLock<Reference> = OnceLock::new();
    REFERENCE.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/pilot-rating-reference.json"))
            .expect("bundled solo calibration")
    })
}
fn solo_score(wins: u64, losses: u64) -> Option<u16> {
    let sample = wins.checked_add(losses)?;
    if sample < 20 || wins < 5 {
        return None;
    }
    // Smoothed outcome quality times experience. A loss cannot increase it.
    let strength = (wins as f64 + 5.) / (sample as f64 + 150.);
    let knots = &reference().knots;
    let index = knots.partition_point(|k| k[0] < strength);
    let percentile = if index == 0 {
        0.
    } else if index == knots.len() {
        1.
    } else {
        let [left, low] = knots[index - 1];
        let [right, high] = knots[index];
        low + (high - low) * (strength - left) / (right - left)
    };
    // 9000..9998 is deliberately reserved for a future model revision.
    Some((1000. + (7999. * percentile.clamp(0., 1.)).floor()).clamp(1000., 8999.) as u16)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Rating {
    pub character_id: i64,
    pub solo_kills: u64,
    pub solo_losses: u64,
    pub kills: u64,
    pub score: Option<u16>,
    pub group_fraction: Option<f64>,
    pub large_group_fraction: Option<f64>,
    pub average_gang: Option<f64>,
}
impl Rating {
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
        let score = solo_score(solo_kills, solo_losses);
        // #:10+ is cumulative: do not add #:25+, #:50+ etc. again.
        let large = stats["labels"]["#:10+"]
            .get("shipsDestroyed")
            .and_then(Value::as_u64);
        let average = n(stats, "avgGangSize");
        Ok(Self {
            character_id,
            solo_kills,
            solo_losses,
            kills,
            score,
            group_fraction: (kills > 0).then(|| 1. - solo_kills as f64 / kills as f64),
            large_group_fraction: large
                .filter(|n| *n <= kills && kills > 0)
                .map(|n| n as f64 / kills as f64),
            average_gang: (average.is_finite() && average > 0.).then_some(average),
        })
    }
    pub fn tier(&self) -> Option<usize> {
        self.score.map(|score| {
            [2500, 4000, 5500, 7000, 8000]
                .into_iter()
                .take_while(|limit| score >= *limit)
                .count()
        })
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
    pub fn hint(&self) -> String {
        let mut hint = if self.score.is_some() {
            "Условная оценка соло по zKillboard; не официальный ELO.\n"
        } else {
            "Недостаточно соло-данных для оценки.\n"
        }
        .to_string();
        hint += &format!(
            "Соло-убийства: {} · соло-потери: {}\n",
            self.solo_kills, self.solo_losses
        );
        if let Some(group) = self.group_fraction {
            hint += &format!("Убийства в группе: {:.0}%\n", group * 100.);
        }
        if let Some(large) = self.large_group_fraction {
            hint += &format!("С 10+ атакующими: {:.0}%\n", large * 100.);
        }
        if let Some(avg) = self.average_gang {
            hint += &format!("Средний размер группы по zKillboard: {avg:.1}\n");
        }
        hint += "Статистика за всё время. Соло-метка zKillboard не доказывает честную дуэль; помощь и состав флота полностью не видны. Групповые убийства не повышают соло-оценку.";
        hint
    }
}

pub fn cached(store: &Store, name: &str) -> Option<Rating> {
    let character = store
        .meta(&format!("character:{}", name.to_lowercase()))?
        .as_i64()?;
    let mut rating: Rating =
        serde_json::from_value(store.meta(&format!("pilot_rating:v1:{character}"))?).ok()?;
    rating.score = solo_score(rating.solo_kills, rating.solo_losses);
    (rating.character_id == character).then_some(rating)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn stats(kills: u64, wins: u64, losses: u64) -> Value {
        json!({"type":"characterID","id":123,"shipsDestroyed":kills,"shipsLost":losses,"soloKills":wins,"soloLosses":losses})
    }
    #[test]
    fn group_farming_cannot_inflate_solo_rating_and_absence_is_unknown() {
        let a = Rating::from_stats(&stats(100, 50, 20), 123).unwrap();
        let mut blob = stats(10000, 50, 20);
        blob["labels"] = json!({"#:10+":{"shipsDestroyed":9500},"#:25+":{"shipsDestroyed":9000}});
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
    fn solo_results_control_all_six_tiers_without_formula_or_age_in_hint() {
        for (score, tier) in [
            (1000, 0),
            (2500, 1),
            (4000, 2),
            (5500, 3),
            (7000, 4),
            (8000, 5),
            (8999, 5),
        ] {
            let mut r = Rating::from_stats(&stats(100, 100, 20), 123).unwrap();
            r.score = Some(score);
            assert_eq!(r.tier(), Some(tier));
            assert!(!r.hint().contains("log") && !r.hint().contains("обновлен"));
        }
        let small = Rating::from_stats(&stats(30, 30, 10), 123)
            .unwrap()
            .score
            .unwrap();
        let large = Rating::from_stats(&stats(300, 300, 100), 123)
            .unwrap()
            .score
            .unwrap();
        assert!(large > small);
        let perfect = Rating::from_stats(&stats(1_000_000_000, 1_000_000_000, 0), 123).unwrap();
        assert_eq!(perfect.score, Some(8999));
        for (wins, losses) in [(15, 5), (50, 20), (1000, 500)] {
            let original = Rating::from_stats(&stats(wins, wins, losses), 123)
                .unwrap()
                .score
                .unwrap();
            let win = Rating::from_stats(&stats(wins + 1, wins + 1, losses), 123)
                .unwrap()
                .score
                .unwrap();
            let loss = Rating::from_stats(&stats(wins, wins, losses + 1), 123)
                .unwrap()
                .score
                .unwrap();
            assert!(win >= original && loss <= original);
        }
    }
    #[test]
    fn real_reference_is_ordered_and_spreads_eligible_players_across_the_current_scale() {
        let knots = &reference().knots;
        assert!(knots.len() >= 30);
        assert_eq!(knots.first().unwrap()[1], 0.);
        assert_eq!(knots.last().unwrap()[1], 1.);
        for pair in knots.windows(2) {
            assert!(pair[0][0] < pair[1][0] && pair[0][1] <= pair[1][1]);
        }
        let data: Value =
            serde_json::from_str(include_str!("../resources/pilot-rating-reference.json")).unwrap();
        let mut tiers = [0; 6];
        for row in data["samples"].as_array().unwrap() {
            let wins = row["solo_kills"].as_u64().unwrap();
            let losses = row["solo_losses"].as_u64().unwrap();
            let r = Rating::from_stats(&stats(wins, wins, losses), 123).unwrap();
            let score = r.score.unwrap();
            assert!((1000..=8999).contains(&score));
            tiers[r.tier().unwrap()] += 1;
        }
        assert!(tiers.iter().all(|n| *n > 0));
    }
}
