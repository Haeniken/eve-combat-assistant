//! Локальная проверка фита и сравнение небольшого набора PvP-расходников.
use crate::{
    engine::{Catalog, Engine},
    model::{Settings, array, id, n},
};
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Suggestion {
    pub type_id: i32,
    pub booster: bool,
    pub benefit: String,
    pub score: f64,
}
#[derive(Clone)]
pub struct Plan {
    pub result: Value,
    pub cargo_boosters: Vec<i32>,
    pub suggestions: Vec<Suggestion>,
    pub warnings: Vec<String>,
}

pub fn slot(catalog: &Catalog, type_id: i32, booster: bool) -> i32 {
    n(
        &catalog.get(type_id)["attributes"],
        if booster {
            "boosterness"
        } else {
            "implantness"
        },
    ) as i32
}
pub fn select(types: &mut Vec<i32>, type_id: i32, booster: bool, catalog: &Catalog) {
    let target = slot(catalog, type_id, booster);
    types.retain(|other| slot(catalog, *other, booster) != target);
    types.push(type_id);
    types.sort_by_key(|id| slot(catalog, *id, booster));
}

fn metrics(result: &Value) -> Vec<(&'static str, f64, bool)> {
    let hull = &result["hull"]["attributes"];
    let mut range: f64 = 0.;
    let mut radius: f64 = 0.;
    for module in array(&result["modules"]) {
        let attrs = &module["attributes"];
        let charge = &module["charge"]["attributes"];
        range = range
            .max(n(attrs, "maxRange"))
            .max(n(charge, "maxVelocity") * n(charge, "explosionDelay") / 1000.);
        radius = radius.max(n(charge, "aoeCloudSize"));
    }
    vec![
        ("DPS", n(&result["summary"]["offense"], "dps"), true),
        ("скорость", n(hull, "maxVelocity"), true),
        ("ёмкость капы", n(hull, "capacitorCapacity"), true),
        ("дальность", range, true),
        ("радиус взрыва", radius, false),
        ("сигнатура", n(hull, "signatureRadius"), false),
    ]
}

pub fn calculate(settings: &Settings, catalog: &Catalog) -> Result<Plan, String> {
    if settings.own_fit.trim().is_empty() {
        return Err("Сначала вставьте свой фит".into());
    }
    let mut engine = Engine::default();
    let result = engine.evaluate(settings.own_request(json!(settings.own_fit), true))?;
    let cargo_boosters: Vec<_> = array(&result["cargo"])
        .iter()
        .filter(|item| id(item, "quantity") > 0)
        .map(|item| id(item, "type_id") as i32)
        .filter(|tid| slot(catalog, *tid, true) > 0)
        .collect();
    let mut warnings = Vec::new();
    for (resource, label) in [("cpu", "CPU"), ("power", "Powergrid")] {
        let free = n(&result["summary"][resource], "free");
        if free < -0.01 {
            warnings.push(format!("Не хватает {label}: {:.1}", -free));
        }
    }
    if n(&result["summary"]["offense"], "dps") <= 0. {
        warnings.push("Не загружены заряды или в фите нет оружия".into());
    }
    let candidates = [
        "Synth Crash Booster",
        "Synth Drop Booster",
        "Synth Mindflood Booster",
        "Synth X-Instinct Booster",
        "Agency 'Overclocker' SB3 Dose I",
        "Zainou 'Deadeye' Rapid Launch RL-1003",
        "Zainou 'Deadeye' Light Missiles LM-903",
        "Zainou 'Deadeye' Heavy Missiles HM-703",
        "Zainou 'Deadeye' Heavy Assault Missiles AM-703",
        "Eifyr and Co. 'Gunslinger' Surgical Strike SS-903",
        "Eifyr and Co. 'Gunslinger' Rapid Firing RF-903",
        "Eifyr and Co. 'Rogue' Navigation NN-603",
        "Zainou 'Gypsy' CPU Management EE-603",
        "Inherent Implants 'Squire' Power Grid Management EG-603",
    ];
    let base = metrics(&result);
    let mut suggestions = Vec::new();
    for name in candidates {
        let Some(type_id) = catalog.names.get(&name.to_lowercase()).copied() else {
            continue;
        };
        let booster = slot(catalog, type_id, true) > 0;
        let mut candidate = settings.clone();
        let types = if booster {
            &mut candidate.boosters
        } else {
            &mut candidate.implants
        };
        if types.contains(&type_id) {
            continue;
        }
        select(types, type_id, booster, catalog);
        let evaluated = engine.evaluate(candidate.own_request(result["fit"].clone(), false))?;
        let changed = metrics(&evaluated);
        let mut benefits = Vec::new();
        let mut score = 0.;
        for ((label, before, up), (_, after, _)) in base.iter().zip(&changed) {
            if *before <= 0. {
                continue;
            }
            let percent = (after / before - 1.) * 100.;
            let gain = if *up { percent } else { -percent };
            score += gain;
            if percent.abs() >= 0.1 {
                benefits.push(format!("{label} {percent:+.1}%"));
            }
        }
        for (key, label) in [("cpu", "CPU"), ("power", "Powergrid")] {
            let before = n(&result["summary"][key], "free");
            let after = n(&evaluated["summary"][key], "free");
            if before < 0. && after > before + 0.1 {
                benefits.push(format!("{label} +{:.1}", after - before));
                score += 5.;
            }
        }
        if score > 0.05 && !benefits.is_empty() {
            suggestions.push(Suggestion {
                type_id,
                booster,
                benefit: benefits.join(" · "),
                score,
            });
        }
    }
    suggestions.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(Plan {
        result,
        cargo_boosters,
        suggestions,
        warnings,
    })
}
