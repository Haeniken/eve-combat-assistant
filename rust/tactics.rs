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
pub fn accepts_charge(module: &Value, charge: &Value) -> bool {
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
    charge["category"] == 8
        && groups
        && (n(a, "chargeSize") == 0. || n(a, "chargeSize") == n(ca, "chargeSize"))
}
pub fn compatible(module: &Value, charge: &Value) -> bool {
    accepts_charge(module, charge) && sum(&vector(charge)) > 0.
}
fn charged_weapon(module: &Value) -> bool {
    s(module, "slot") != "dronebay"
        && !family(s(module, "name"), id(module, "group")).is_empty()
        && ["active", "overload"].contains(&s(module, "state"))
}
pub fn own_weapon_summary(own: &Value, catalog: &Catalog) -> Value {
    let weapons: Vec<_> = array(&own["modules"])
        .iter()
        .filter(|m| charged_weapon(m))
        .collect();
    let charges: Vec<_> = array(&own["cargo"])
        .iter()
        .filter(|item| id(item, "quantity") > 0)
        .map(|item| id(item, "type_id") as i32)
        .filter(|charge| {
            weapons.iter().any(|module| {
                compatible(catalog.get(id(module, "id") as i32), catalog.get(*charge))
            })
        })
        .collect();
    json!({"weapon_type_ids":weapons.iter().map(|m|id(m,"id")).collect::<Vec<_>>(),"loaded_weapon_charge_ids":weapons.iter().filter(|m|!m["charge"].is_null() && sum(&vector(&m["charge"]))>0.).map(|m|id(&m["charge"],"id")).collect::<Vec<_>>(),"compatible_cargo_charge_ids":charges})
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
            let smartbomb = id(module, "group") == 72;
            if !f.is_empty() || drone || smartbomb || n(a, "damagePerSecondWithoutReload") > 0. {
                let damage = vector(if module["charge"].is_null() {
                    module
                } else {
                    &module["charge"]
                });
                let mut dps = n(a, "damagePerSecondWithoutReload");
                if dps <= 0. {
                    dps = sum(&damage) * n(a, "damageMultiplier").max(1.)
                        / (n(a, if smartbomb { "duration" } else { "speed" }) / 1000.).max(0.1);
                }
                let kind = if smartbomb {
                    3
                } else if drone {
                    2
                } else if f == "Missiles" {
                    1
                } else {
                    0
                };
                let optimal = if drone {
                    n(&result["summary"]["drones"], "range") * 1000.
                } else if smartbomb {
                    n(a, "empFieldRange")
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
        3 => f64::from(distance <= weapon.optimal),
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
pub fn base_hole_preference(damage: &[f64; 4], total: &[f64; 4], base: &[f64; 4]) -> f64 {
    let low = total.iter().copied().fold(f64::INFINITY, f64::min);
    let high = total.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if high - low >= 0.01 || sum(damage) <= 0. {
        return 0.;
    }
    (0..4)
        .map(|i| damage[i] / sum(damage) * (1. - base[i] / 100.))
        .sum()
}
fn profile_base_preference(profile: &Profile, enemy: &Value, catalog: &Catalog) -> f64 {
    let (title, _) = crate::fits::describe(&enemy["fit"], catalog);
    let layer = if title.contains("Armor") {
        "armor"
    } else if title.contains("Shield") {
        "shield"
    } else {
        "structure"
    };
    let fields = ["em", "therm", "kin", "expl"];
    let total = fields.map(|key| n(&enemy["summary"]["defense"][layer]["resist"], key));
    let base = if enemy["base_defense"][layer]["resist"].is_object() {
        fields.map(|key| n(&enemy["base_defense"][layer]["resist"], key))
    } else {
        DAMAGE.map(|(_, _, suffix)| {
            let prefix = if layer == "structure" { "" } else { layer };
            let key = if prefix.is_empty() {
                format!(
                    "{}{}DamageResonance",
                    suffix[..1].to_lowercase(),
                    &suffix[1..]
                )
            } else {
                format!("{prefix}{suffix}DamageResonance")
            };
            (1. - catalog.get(id(&enemy["hull"], "id") as i32)["attributes"][key]
                .as_f64()
                .unwrap_or(1.))
                * 100.
        })
    };
    let mut damage = [0.; 4];
    for weapon in &profile.weapons {
        for (i, value) in damage.iter_mut().enumerate() {
            *value += weapon.dps * weapon.damage[i] / sum(&weapon.damage).max(1.);
        }
    }
    base_hole_preference(&damage, &total, &base)
}

pub fn resistance_note(values: &[f64; 4]) -> String {
    let low = values.iter().copied().fold(f64::INFINITY, f64::min);
    let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if high - low < 0.01 {
        return "РАВНОМЕРНО".into();
    }
    format!(
        "СЛАБЕЕ {}",
        (0..4)
            .filter(|i| (values[*i] - low).abs() < 0.01)
            .map(|i| DAMAGE[i].0)
            .collect::<Vec<_>>()
            .join(" / ")
    )
}

/// Damage before target resists/application, for the chosen historical example.
pub fn incoming_summary(result: &Value, original_fit: &Value, catalog: &Catalog) -> Value {
    let profile = Profile::new(result);
    let uncertain = !array(&result["warnings"]).is_empty();
    let mut mix = [0.; 4];
    for weapon in &profile.weapons {
        for (i, value) in mix.iter_mut().enumerate() {
            *value += weapon.damage[i] * weapon.dps / sum(&weapon.damage).max(1.);
        }
    }
    let mut sources = Vec::new();
    let mut known_types = [false; 4];
    for module in array(&result["modules"]) {
        if !["active", "overload"].contains(&s(module, "state")) {
            continue;
        }
        let charge = &module["charge"];
        let damage = if charge.is_null() {
            vector(catalog.get(id(module, "id") as i32))
        } else {
            vector(catalog.get(id(charge, "id") as i32))
        };
        if sum(&damage) <= 0. {
            continue;
        }
        for (i, value) in damage.iter().enumerate() {
            known_types[i] |= *value > 0.;
        }
        let original_type = module["approximation"]
            .get("original_type_id")
            .unwrap_or(&module["id"]);
        let assumed = !charge.is_null()
            && !array(&original_fit["modules"]).iter().any(|m| {
                &m["type_id"] == original_type
                    && m["slot"]["index"] == module["slot_index"]
                    && !m["charge"].is_null()
            });
        sources.push(json!({"weapon":module["name"],"approximation":module.get("approximation"),"charge":charge.get("name"),"assumed_charge":assumed,"damage":damage,"max_range_m":if id(module,"group")==72 {Some(n(&module["attributes"],"empFieldRange"))}else{None}}));
    }
    let total = sum(&mix);
    let percentages = if total > 0. {
        Some(mix.map(|v| v / total * 100.))
    } else {
        None
    };
    let mut order = [0, 1, 2, 3];
    order.sort_by(|a, b| mix[*b].total_cmp(&mix[*a]));
    let label = if let Some(percentages) = percentages {
        order
            .iter()
            .filter(|i| percentages[**i] > 0.001)
            .map(|i| format!("{} {:.0}%", DAMAGE[*i].0, percentages[*i]))
            .collect::<Vec<_>>()
            .join(" / ")
    } else {
        (0..4)
            .filter(|i| known_types[*i])
            .map(|i| DAMAGE[i].0)
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let mut hint="По оружию, дронам и зарядам выбранного примера потери, до резистов и применения урона. Текущий заряд противника неизвестен; он может его сменить.".to_string();
    for source in &sources {
        hint += &format!(
            "\n{}{}{}",
            s(source, "weapon"),
            if source["charge"].is_string() {
                ": "
            } else {
                ""
            },
            s(source, "charge")
        );
        if let Some(range) = source["max_range_m"].as_f64() {
            hint += &format!(" (до {:.1} км)", range / 1000.);
        }
        if source["assumed_charge"] == true {
            hint += " (подставлен стандартный совместимый заряд)";
        }
        if source["approximation"].is_object() {
            hint += &format!(
                " (аналог для {})",
                s(&source["approximation"], "original_name")
            );
        }
    }
    if uncertain {
        hint += "\nПрисутствуют Abyssal модули, оценка не точна. Доли рассчитаны по обычным аналогам; исходный модуль и результат мутации неизвестны.";
    }
    json!({"label":if label.is_empty(){"—"}else{&label},"percentages":percentages,"sources":sources,"uncertain":uncertain,"hint":hint})
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
    for (profile, own_pilot) in [(&mut own_profile, true), (&mut enemy_profile, false)] {
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
            let request = if own_pilot {
                settings.own_request(fit, false)
            } else {
                json!({"fit":fit,"skill_level":5})
            };
            let disabled = engine.evaluate(request)?;
            profile.base_speed = n(&disabled["hull"]["attributes"], "maxVelocity");
            profile.base_signature = n(&disabled["hull"]["attributes"], "signatureRadius");
        }
    }
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
            catalog.get(*type_id)["category"] == 8
                && sum(&vector(catalog.get(*type_id))) > 0.
                && array(&own["modules"]).iter().any(|module| {
                    charged_weapon(module)
                        && compatible(catalog.get(id(module, "id") as i32), catalog.get(*type_id))
                })
        })
        .collect();
    ammo.sort();
    let unloaded = array(&own["modules"]).iter().any(|module| {
        charged_weapon(module)
            && sum(&vector(&module["charge"])) <= 0.
            && ammo.iter().any(|charge| {
                compatible(catalog.get(id(module, "id") as i32), catalog.get(*charge))
            })
    });
    // An imported empty gun must be evaluated with available ammunition rather
    // than competing as a drone-only setup or failing before charge trials.
    let mut candidates = Vec::new();
    if !unloaded {
        candidates.push((String::new(), own_profile.clone()));
    }
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
            let request = settings.own_request(fit, false);
            let mut profile = Profile::new(&engine.evaluate(request)?);
            profile.base_speed = own_profile.base_speed;
            profile.base_signature = own_profile.base_signature;
            candidates.push((catalog.name(type_id).into(), profile));
        }
    }
    if !candidates.iter().any(|(_, p)| p.dps() > 0.) {
        return Err("В своём фите нет рассчитываемого оружия с зарядами или дронов. Проверьте полный EFT корабля".into());
    }
    if enemy_profile.dps() <= 0. {
        return Err(
            "В примере потери противника не определён исходящий урон. Резисты и скорость доступны"
                .into(),
        );
    }
    let grid: Vec<f64> = [
        500., 1000., 2000., 3000., 4500., 6000., 7500., 9000., 11000., 13000., 16000., 20000.,
        24000., 30000., 40000., 50000., 65000., 80000., 100000.,
    ]
    .into_iter()
    .filter(|distance| *distance <= n(&own["hull"]["attributes"], "maxTargetRange"))
    .collect();
    let base_preferences: Vec<_> = candidates
        .iter()
        .map(|(_, profile)| profile_base_preference(profile, enemy, catalog))
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
        .max_by(|a, b| {
            let equal_effectiveness = (a.0 - b.0).abs() <= a.0.abs().max(b.0.abs()).max(1.) * 1e-9;
            if equal_effectiveness && a.1 == b.1 {
                base_preferences[a.2]
                    .total_cmp(&base_preferences[b.2])
                    .then(a.0.total_cmp(&b.0))
            } else {
                a.0.total_cmp(&b.0).then(b.1.total_cmp(&a.1))
            }
        })
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
            .filter(|module| {
                charged_weapon(module)
                    && !module["charge"].is_null()
                    && sum(&vector(&module["charge"])) > 0.
            })
            .map(|module| s(&module["charge"], "name"))
            .collect();
        if names.is_empty() {
            if profile.weapons.iter().any(|w| w.kind == 2 && w.dps > 0.) {
                "Дроны".into()
            } else {
                "Не требуются".into()
            }
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
