//! Приближённое сравнение двух фитов. Дистанции в метрах, время в секундах.

use crate::{
    engine::{Catalog, Engine},
    fits::family,
    model::{Settings, array, id, n, s},
};
use serde::Serialize;
use serde_json::{Value, json};
pub const DAMAGE: [(&str, &str, &str); 4] = [
    ("EM", "emDamage", "Em"),
    ("THERM", "thermalDamage", "Thermal"),
    ("KIN", "kineticDamage", "Kinetic"),
    ("EXP", "explosiveDamage", "Explosive"),
];
fn vector(v: &Value) -> [f64; 4] {
    DAMAGE.map(|(_, key, _)| n(&v["attributes"], key).max(0.))
}
fn sum(v: &[f64; 4]) -> f64 {
    v.iter().sum()
}
pub fn compatible(module: &Value, charge: &Value) -> bool {
    let a = &module["attributes"];
    let ca = &charge["attributes"];
    let group = id(charge, "group") as f64;
    let groups = a
        .as_object()
        .map(|a| {
            a.iter().any(|(k, v)| {
                k.to_lowercase().starts_with("chargegroup") && v.as_f64() == Some(group)
            })
        })
        .unwrap_or(false);
    groups
        && (n(a, "chargeSize") == 0. || n(a, "chargeSize") == n(ca, "chargeSize"))
        && sum(&vector(charge)) > 0.
}
pub fn fill_charges(fit: &Value, catalog: &Catalog) -> Value {
    let mut fit = fit.clone();
    if let Some(modules) = fit["modules"].as_array_mut() {
        for module in modules {
            if !module["charge"].is_null() {
                continue;
            }
            let t = catalog.get(id(module, "type_id") as i32);
            let prefix = match family(s(t, "name"), id(t, "group")) {
                "Autocannon" | "Artillery" => "fusion",
                "Blaster" | "Railgun" => "antimatter charge",
                "Beam" | "Pulse" => "multifrequency",
                "Missiles" => "scourge",
                "Disintegrator" => "tetryon",
                _ => continue,
            };
            if let Some(chosen) = catalog
                .types
                .values()
                .filter(|v| {
                    v["category"] == 8
                        && s(v, "name").to_lowercase().starts_with(prefix)
                        && compatible(t, v)
                })
                .min_by_key(|v| (s(v, "name").len(), id(v, "id")))
            {
                module["charge"] = json!({"type_id":chosen["id"]});
            }
        }
    }
    fit
}
#[derive(Clone)]
struct Weapon {
    dps: f64,
    damage: [f64; 4],
    kind: u8,
    optimal: f64,
    falloff: f64,
    tracking: f64,
    resolution: f64,
    radius: f64,
    velocity: f64,
    reduction: f64,
}
#[derive(Clone)]
pub struct Profile {
    result: Value,
    weapons: Vec<Weapon>,
    speed: f64,
    base_speed: f64,
    signature: f64,
    base_signature: f64,
    mwd: bool,
    scram: f64,
    web: f64,
    web_factor: f64,
    point: f64,
    neut: f64,
    neut_rate: f64,
    cap_peak: f64,
}
impl Profile {
    pub fn new(result: &Value) -> Self {
        let hull = &result["hull"]["attributes"];
        let mut profile = Self {
            result: result.clone(),
            weapons: Vec::new(),
            speed: n(hull, "maxVelocity"),
            base_speed: n(hull, "maxVelocity"),
            signature: n(hull, "signatureRadius"),
            base_signature: n(hull, "signatureRadius"),
            mwd: false,
            scram: 0.,
            web: 0.,
            web_factor: 1.,
            point: 0.,
            neut: 0.,
            neut_rate: 0.,
            cap_peak: n(hull, "capacitorCapacity") * 2.5
                / (n(hull, "rechargeRate") / 1000.).max(1.),
        };
        for module in array(&result["modules"]) {
            if !["active", "overload"].contains(&s(module, "state")) {
                continue;
            }
            let a = &module["attributes"];
            let ca = &module["charge"]["attributes"];
            let name = s(module, "name").to_lowercase();
            let f = family(s(module, "name"), id(module, "group"));
            let drone = s(module, "slot") == "dronebay";
            if !f.is_empty() || drone || n(a, "damagePerSecondWithoutReload") > 0. {
                let damage = vector(if module["charge"].is_null() {
                    module
                } else {
                    &module["charge"]
                });
                let mut dps = n(a, "damagePerSecondWithoutReload");
                if dps <= 0. {
                    dps = sum(&damage) * n(a, "damageMultiplier").max(1.)
                        / (n(a, "speed") / 1000.).max(0.1);
                }
                let kind = if drone {
                    2
                } else if f == "Missiles" {
                    1
                } else {
                    0
                };
                let optimal = if drone {
                    n(&result["summary"]["drones"], "range") * 1000.
                } else if kind == 1 {
                    n(ca, "maxVelocity") * n(ca, "explosionDelay") / 1000.
                } else {
                    n(a, "maxRange")
                };
                profile.weapons.push(Weapon {
                    dps,
                    damage,
                    kind,
                    optimal,
                    falloff: n(a, "falloff"),
                    tracking: n(a, "trackingSpeed"),
                    resolution: n(a, "optimalSigRadius").max(1.),
                    radius: n(ca, "aoeCloudSize"),
                    velocity: n(ca, "aoeVelocity"),
                    reduction: n(ca, "aoeDamageReductionFactor"),
                });
            }
            let reach = n(a, "maxRange") + n(a, "falloff") * 0.25;
            if name.contains("scrambler") {
                profile.scram = profile.scram.max(reach)
            }
            if name.contains("warp disruptor") {
                profile.point = profile.point.max(reach)
            }
            if name.contains("webifier") || name.contains("stasis web") {
                profile.web = profile.web.max(reach);
                profile.web_factor *= (1. + n(a, "speedFactor") / 100.).max(0.1);
            }
            if name.contains("neutralizer") {
                profile.neut = profile.neut.max(reach);
                profile.neut_rate +=
                    n(a, "energyDestabilizationAmount") / (n(a, "duration") / 1000.).max(0.1);
            }
            if name.contains("microwarpdrive") {
                profile.mwd = true
            }
        }
        let actual = n(&result["summary"]["offense"], "dps");
        let total = profile.dps();
        if total > 0. && actual > 0. {
            for weapon in &mut profile.weapons {
                weapon.dps *= actual / total;
            }
        }
        profile
    }
    fn dps(&self) -> f64 {
        self.weapons.iter().map(|weapon| weapon.dps).sum()
    }
    fn speed_against(&self, other: &Self, distance: f64) -> f64 {
        let mut speed = if self.mwd && other.scram >= distance {
            self.base_speed
        } else {
            self.speed
        };
        if other.web >= distance {
            speed *= other.web_factor;
        }
        speed
    }
    fn signature_against(&self, other: &Self, distance: f64) -> f64 {
        if self.mwd && other.scram >= distance {
            self.base_signature
        } else {
            self.signature
        }
    }
    fn ehp(&self, damage: &[f64; 4]) -> f64 {
        let total = sum(damage);
        if total <= 0. {
            return f64::INFINITY;
        }
        let a = &self.result["hull"]["attributes"];
        let mut ehp = 0.;
        for (prefix, hp) in [
            ("shield", "shieldCapacity"),
            ("armor", "armorHP"),
            ("", "hp"),
        ] {
            let mut resonance = 0.;
            for (i, (_, _, suffix)) in DAMAGE.iter().enumerate() {
                let key = if prefix.is_empty() {
                    format!(
                        "{}{}DamageResonance",
                        suffix[..1].to_lowercase(),
                        &suffix[1..]
                    )
                } else {
                    format!("{prefix}{suffix}DamageResonance")
                };
                resonance += damage[i] / total * a.get(&key).and_then(Value::as_f64).unwrap_or(1.);
            }
            ehp += n(a, hp) / resonance.max(0.01);
        }
        ehp.max(1.)
    }
    fn weak(&self) -> String {
        (0..4)
            .map(|i| {
                let mut d = [0.; 4];
                d[i] = 1.;
                (self.ehp(&d), DAMAGE[i].0)
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap()
            .1
            .into()
    }
    fn applied(&self, other: &Self, distance: f64, angular: f64) -> f64 {
        self.weapons
            .iter()
            .map(|weapon| {
                weapon.dps
                    * application(
                        weapon,
                        distance,
                        other.signature_against(self, distance),
                        other.speed_against(self, distance),
                        angular,
                    )
            })
            .sum()
    }
    fn pressure(&self, other: &Self, distance: f64, angular: f64) -> f64 {
        self.weapons
            .iter()
            .map(|weapon| {
                weapon.dps
                    * application(
                        weapon,
                        distance,
                        other.signature_against(self, distance),
                        other.speed_against(self, distance),
                        angular,
                    )
                    / other.ehp(&weapon.damage)
            })
            .sum()
    }
}
fn application(weapon: &Weapon, distance: f64, signature: f64, velocity: f64, angular: f64) -> f64 {
    match weapon.kind {
        2 => {
            if distance <= weapon.optimal {
                (signature / weapon.resolution.max(25.)).min(1.)
            } else {
                0.
            }
        }
        1 => {
            if distance > weapon.optimal {
                return 0.;
            }
            let size = signature / weapon.radius.max(1.);
            1_f64.min(size).min(
                (size * weapon.velocity / velocity.max(1.)).powf(weapon.reduction.clamp(0.1, 1.)),
            )
        }
        _ => {
            let range = (distance - weapon.optimal).max(0.) / weapon.falloff.max(1.);
            let tracking = angular * weapon.resolution / (weapon.tracking * signature).max(1e-9);
            let chance = 0.5_f64.powf((range * range + tracking * tracking).min(100.));
            if chance < 0.01 {
                3. * chance
            } else {
                0.5 * chance * chance + 0.49 * chance + 0.01
            }
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Advice {
    pub action: String,
    pub low: f64,
    pub high: f64,
    pub ammo: String,
    pub damage: String,
    pub weak: String,
    pub neut: String,
    pub speed: String,
    pub reason: String,
    pub own_dps: f64,
    pub enemy_dps: f64,
}
pub fn range_text(low: f64, high: f64) -> String {
    format!("{:.1}–{:.1} км", low / 1000., high / 1000.).replace('.', ",")
}
pub fn advise(
    own: &Value,
    enemy: &Value,
    engine: &mut Engine,
    catalog: &Catalog,
    settings: &Settings,
    distance: Option<f64>,
    angular: f64,
) -> Result<Advice, String> {
    let mut own_profile = Profile::new(own);
    let mut enemy_profile = Profile::new(enemy);
    for (profile, skills, level) in [
        (
            &mut own_profile,
            settings.skills.clone(),
            settings.skill_level,
        ),
        (&mut enemy_profile, None, 5),
    ] {
        if profile.mwd {
            let mut fit = profile.result["fit"].clone();
            for module in fit["modules"].as_array_mut().unwrap() {
                if catalog
                    .name(id(module, "type_id") as i32)
                    .to_lowercase()
                    .contains("microwarpdrive")
                {
                    module["state"] = json!("Online");
                }
            }
            let mut request = json!({"fit":fit,"skill_level":level});
            if let Some(skills) = skills {
                request["skills"] = skills;
            }
            let disabled = engine.evaluate(request)?;
            profile.base_speed = n(&disabled["hull"]["attributes"], "maxVelocity");
            profile.base_signature = n(&disabled["hull"]["attributes"], "signatureRadius");
        }
    }
    if own_profile.dps() <= 0. || enemy_profile.dps() <= 0. {
        return Err("Нужен фит с оружием и зарядами".into());
    }
    let mut candidates = vec![(String::new(), own_profile.clone())];
    let mut ammo = HashSet::new();
    for module in array(&own["modules"]) {
        if !module["charge"].is_null() {
            ammo.insert(id(&module["charge"], "id") as i32);
        }
    }
    for item in array(&own["cargo"]) {
        if id(item, "quantity") > 0 {
            ammo.insert(id(item, "type_id") as i32);
        }
    }
    let mut ammo: Vec<_> = ammo
        .into_iter()
        .filter(|type_id| {
            catalog.get(*type_id)["category"] == 8 && sum(&vector(catalog.get(*type_id))) > 0.
        })
        .collect();
    ammo.sort();
    for type_id in ammo {
        let mut fit = own["fit"].clone();
        let mut changed = false;
        for module in fit["modules"].as_array_mut().unwrap() {
            if compatible(
                catalog.get(id(module, "type_id") as i32),
                catalog.get(type_id),
            ) {
                module["charge"] = json!({"type_id":type_id});
                changed = true;
            }
        }
        if changed {
            let mut request = json!({"fit":fit,"skill_level":settings.skill_level});
            if let Some(skills) = &settings.skills {
                request["skills"] = skills.clone();
            }
            let mut profile = Profile::new(&engine.evaluate(request)?);
            profile.base_speed = own_profile.base_speed;
            profile.base_signature = own_profile.base_signature;
            candidates.push((catalog.name(type_id).into(), profile));
        }
    }
    let grid: Vec<f64> = [
        500., 1000., 2000., 3000., 4500., 6000., 7500., 9000., 11000., 13000., 16000., 20000.,
        24000., 30000., 40000., 50000., 65000., 80000., 100000.,
    ]
    .into_iter()
    .filter(|distance| *distance <= n(&own["hull"]["attributes"], "maxTargetRange"))
    .collect();
    let mut rows = Vec::new();
    for (index, (_, candidate)) in candidates.iter().enumerate() {
        for distance in &grid {
            let outgoing = candidate.pressure(&enemy_profile, *distance, angular);
            if outgoing <= 0. {
                continue;
            }
            let incoming = enemy_profile.pressure(candidate, *distance, angular);
            // Сравниваем долю EHP, снимаемую за секунду. Нижняя граница входящего
            // урона не даёт практически недостижимой дальней точке получить бесконечный балл.
            let mut score = outgoing / (incoming + 0.0015);
            score *= (candidate.applied(&enemy_profile, *distance, angular)
                / (candidate.dps() * 0.45).max(1.))
            .min(1.);
            if enemy_profile.neut >= *distance {
                score /= 1. + enemy_profile.neut_rate / candidate.cap_peak.max(1.);
            }
            if *distance > enemy_profile.scram
                && candidate.speed_against(&enemy_profile, *distance)
                    < enemy_profile.speed_against(candidate, *distance)
            {
                score *= 0.7;
            }
            let tackle = candidate.scram.max(candidate.point);
            if tackle > 0. && *distance > tackle {
                score *= 0.65;
            }
            rows.push((score, *distance, index));
        }
    }
    let Some(best) = rows
        .iter()
        .max_by(|a, b| a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1)))
        .copied()
    else {
        return Err("Нет применения урона на рабочей дистанции".into());
    };
    // Показываем непрерывный диапазон в пределах 90% от лучшей оценки.
    let good: Vec<_> = rows
        .iter()
        .filter(|(score, _, i)| *i == best.2 && *score >= best.0 * 0.9)
        .map(|distance| distance.1)
        .collect();
    let index = grid
        .iter()
        .position(|distance| *distance == best.1)
        .unwrap();
    let (mut left, mut right) = (index, index);
    while left > 0 && good.contains(&grid[left - 1]) {
        left -= 1;
    }
    while right + 1 < grid.len() && good.contains(&grid[right + 1]) {
        right += 1;
    }
    let (mut low, mut high) = (grid[left], grid[right]);
    if low == high {
        low = (low * 0.9).max(500.);
        high *= 1.1;
    }
    let (ammo, profile) = &candidates[best.2];
    high = high.min(n(&own["hull"]["attributes"], "maxTargetRange"));
    let tackle = profile.scram.max(profile.point);
    if tackle > 0. && best.1 <= tackle {
        high = high.min(tackle);
    }
    let my_speed = profile.speed_against(&enemy_profile, best.1);
    let their_speed = enemy_profile.speed_against(profile, best.1);
    let speed = if my_speed > their_speed * 1.08 {
        "ТЫ БЫСТРЕЕ"
    } else if their_speed > my_speed * 1.08 {
        "ОН БЫСТРЕЕ"
    } else {
        "СОПОСТАВИМО"
    };
    let action = if let Some(d) = distance {
        if d > high {
            "СБЛИЖАТЬСЯ"
        } else if d < low {
            "УВЕЛИЧИВАТЬ ДИСТАНЦИЮ"
        } else {
            "УДЕРЖИВАТЬ ДИСТАНЦИЮ"
        }
    } else if best.1 <= 6000. {
        "БЛИЖНИЙ БОЙ"
    } else {
        "ДЕРЖАТЬ ДИСТАНЦИЮ"
    };
    let mut damage_mix = [0.; 4];
    for weapon in &enemy_profile.weapons {
        for (i, value) in damage_mix.iter_mut().enumerate() {
            *value += weapon.damage[i] * weapon.dps / sum(&weapon.damage).max(1.);
        }
    }
    let mut indexes = [0, 1, 2, 3];
    indexes.sort_by(|a, b| damage_mix[*b].total_cmp(&damage_mix[*a]));
    let damage = indexes
        .iter()
        .filter(|i| damage_mix[**i] > sum(&damage_mix) * 0.1)
        .map(|i| DAMAGE[*i].0)
        .collect::<Vec<_>>()
        .join(" / ");
    let ammo = if ammo.is_empty() {
        let names: HashSet<_> = array(&own["modules"])
            .iter()
            .filter(|module| !module["charge"].is_null())
            .map(|module| s(&module["charge"], "name"))
            .collect();
        if names.is_empty() {
            "Дроны".into()
        } else {
            let mut names: Vec<_> = names.into_iter().collect();
            names.sort();
            names.join(", ")
        }
    } else {
        ammo.clone()
    };
    let reason = if enemy_profile.neut >= best.1 {
        "В радиусе нейтриков: контролируй капаситор"
    } else if speed == "ОН БЫСТРЕЕ" && best.1 > enemy_profile.scram {
        "Противник быстрее: удержание дистанции зависит от контроля"
    } else {
        "Рабочая дистанция твоего оружия и меньше входящего урона"
    };
    Ok(Advice {
        action: action.into(),
        low,
        high,
        ammo,
        damage,
        weak: enemy_profile.weak(),
        neut: if enemy_profile.neut > 0. {
            format!("ДО {:.1} КМ", enemy_profile.neut / 1000.).replace('.', ",")
        } else {
            "НЕТ В ФИТЕ".into()
        },
        speed: speed.into(),
        reason: reason.into(),
        own_dps: profile.applied(&enemy_profile, best.1, angular),
        enemy_dps: enemy_profile.applied(profile, best.1, angular),
    })
}
use std::collections::HashSet;
