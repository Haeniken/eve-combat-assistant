//! Static fitting information. No target motion, external tackle or drone reach.
use crate::{
    engine::{Catalog, Engine},
    fits::family,
    model::{Settings, array, id, n, s},
    tactics,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn active(module: &Value) -> bool {
    ["active", "overload"].contains(&s(module, "state"))
}
fn damage(item: &Value) -> f64 {
    tactics::DAMAGE
        .iter()
        .map(|(_, key, _)| n(&item["attributes"], key))
        .sum()
}
fn weapon(module: &Value) -> bool {
    s(module, "slot") == "high"
        && active(module)
        && (!family(s(module, "name"), id(module, "group")).is_empty()
            || id(module, "group") == 72
            || damage(module) > 0.
            || damage(&module["charge"]) > 0.)
}

/// Stationary missile upper reach: acceleration and server-second lifetime.
/// Flight starts at the ship centre; distances in the HUD are surface-to-surface.
fn missile_range(charge: &Value, hull_radius: f64) -> Option<f64> {
    let a = &charge["attributes"];
    let v = n(a, "maxVelocity");
    let lifetime = n(a, "explosionDelay") / 1000.;
    if v <= 0. || lifetime <= 0. {
        return None;
    }
    let time = lifetime + hull_radius / v;
    let time = if (time - time.round()).abs() < 1e-8 {
        time.round()
    } else {
        time.ceil()
    };
    let acceleration = (n(a, "mass") * n(a, "agility") / 1_000_000.).clamp(0., time);
    let mut range = (v * (time - acceleration / 2.) - hull_radius).max(0.);
    if n(a, "maxFOFTargetRange") > 0. {
        range = range.min(n(a, "maxFOFTargetRange"));
    }
    Some(range)
}

fn reach(module: &Value, hull_radius: f64) -> Option<Value> {
    if !weapon(module) {
        return None;
    }
    let a = &module["attributes"];
    let charge = &module["charge"];
    let f = family(s(module, "name"), id(module, "group"));
    // Generic dogma defaults are not usable falloff for these weapon families.
    let falloff =
        if ["Disintegrator", "Vorton", "Missiles"].contains(&f) || id(module, "group") == 72 {
            0.
        } else {
            n(a, "falloff").max(0.)
        };
    let (range, approximate, method) = if f == "Missiles" {
        (missile_range(charge, hull_radius)?, true, "missile_upper")
    } else if id(module, "group") == 72 {
        (n(a, "empFieldRange"), false, "fixed")
    } else {
        // An unloaded charged gun has no usable range until an ammo trial succeeds.
        if !f.is_empty() && damage(charge) <= 0. {
            return None;
        }
        (
            n(a, "maxRange") + 2. * falloff,
            falloff > 0.,
            if falloff > 0. {
                "optimal_plus_2_falloff"
            } else {
                "optimal"
            },
        )
    };
    if !range.is_finite() || range <= 0. {
        return None;
    }
    Some(
        json!({"weapon":module["name"],"weapon_type_id":module["id"],"slot_index":module["slot_index"],
        "charge":charge.get("name"),"charge_type_id":charge.get("id"),"max_range_m":range,
        "optimal_m":n(a,"maxRange"),"falloff_m":falloff,"approximate":approximate,"method":method}),
    )
}

/// Evaluate every published compatible damaging charge with the complete fit.
/// Identical fitted guns share each ammo trial; cargo is deliberately not a limit.
pub fn weapon_range(result: &Value, engine: &mut Engine, catalog: &Catalog) -> Value {
    let radius = n(&result["hull"]["attributes"], "radius");
    let mut best: BTreeMap<i64, Value> = BTreeMap::new();
    let mut types: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for module in array(&result["modules"]).iter().filter(|m| weapon(m)) {
        if let Some(source) = reach(module, radius) {
            best.insert(id(module, "slot_index"), source);
        }
        types
            .entry(id(module, "id"))
            .or_default()
            .push(id(module, "slot_index"));
    }
    let mut errors = Vec::new();
    let mut trials = 0;
    for (type_id, slots) in types {
        let mut charges: Vec<_> = catalog
            .types
            .values()
            .filter(|c| {
                c["published"] != false && tactics::compatible(catalog.get(type_id as i32), c)
            })
            .map(|c| id(c, "id"))
            .collect();
        charges.sort_unstable();
        for charge in charges {
            let mut fit = result["fit"].clone();
            for m in fit["modules"].as_array_mut().unwrap() {
                if s(&m["slot"], "type") == "High" && slots.contains(&id(&m["slot"], "index")) {
                    m["charge"] = json!({"type_id":charge});
                }
            }
            trials += 1;
            match engine.evaluate(json!({"fit":fit,"skill_level":5})) {
                Ok(trial) => {
                    for m in array(&trial["modules"])
                        .iter()
                        .filter(|m| s(m, "slot") == "high" && slots.contains(&id(m, "slot_index")))
                    {
                        if let Some(source) = reach(m, radius) {
                            let slot = id(m, "slot_index");
                            if best.get(&slot).is_none_or(|old| {
                                n(&source, "max_range_m") > n(old, "max_range_m") + 0.001
                            }) {
                                best.insert(slot, source);
                            }
                        }
                    }
                }
                Err(error) => errors
                    .push(json!({"weapon_type_id":type_id,"charge_type_id":charge,"error":error})),
            }
        }
    }
    let sources: Vec<_> = best.into_values().collect();
    let maximum = sources
        .iter()
        .max_by(|a, b| n(a, "max_range_m").total_cmp(&n(b, "max_range_m")));
    let label = maximum.map(|v| {
        format!(
            "до {}{:.1} км",
            if v["approximate"] == true { "≈" } else { "" },
            n(v, "max_range_m") / 1000.
        )
        .replace('.', ",")
    });
    let mut hint = "Дальность установленного боевого оружия выбранного фита, навыки V. Проверены совместимые дальние заряды, включая отсутствующие в трюме примера. Дроны не учитываются. Текущий заряд и его тип урона могут отличаться от примера в «ЕГО УРОН». Без погони и внешних эффектов.".to_string();
    if sources
        .iter()
        .any(|v| v["method"] == "optimal_plus_2_falloff")
    {
        hint += "\nДля пушек с falloff: приблизительный внешний рубеж optimal + 2×falloff; урон за ним не обрывается. Это не гарантированно безопасная дистанция.";
    }
    if sources.iter().any(|v| v["method"] == "missile_upper") {
        hint += "\nДля ракет: верхняя оценка до неподвижной цели, с разгоном и округлением времени полёта. У самого края часть ракет может не долететь.";
    }
    for source in &sources {
        hint += &format!(
            "\n{}{}{}: {:.1} км",
            s(source, "weapon"),
            if source["charge"].is_string() {
                " / "
            } else {
                ""
            },
            s(source, "charge"),
            n(source, "max_range_m") / 1000.
        );
    }
    if !errors.is_empty() {
        hint += "\nЧасть зарядов не удалось рассчитать; дальность может быть неполной.";
    }
    if !array(&result["approximations"]).is_empty() {
        hint += "\nAbyssal-модули заменены обычными аналогами; оценка приблизительная.";
    }
    json!({"label":label.unwrap_or_else(||"—".into()),"max_range_m":maximum.map(|m|n(m,"max_range_m")),
        "approximate":maximum.map(|m|m["approximate"].clone()),"sources":sources,"ammo_trials":trials,"errors":errors,"hint":hint})
}

/// Base and one unheated propulsion module at a time; all other fitted effects stay.
/// Own requests keep the exact character skills/implants/boosters; enemy uses V.
pub fn fitted_speed(
    result: &Value,
    engine: &mut Engine,
    catalog: &Catalog,
    own: Option<&Settings>,
) -> Result<Value, String> {
    let mut fit = result["fit"].clone();
    let ship_mode = crate::engine::propulsion_mode(id(&fit, "ship_type_id") as i32)?;
    if let Some(mode) = &ship_mode {
        fit["ship_mode"] = mode["id"].clone();
    }
    let mut props = Vec::new();
    let mut inactive_movement_modules = Vec::new();
    for (i, m) in fit["modules"]
        .as_array_mut()
        .ok_or("В фите нет модулей")?
        .iter_mut()
        .enumerate()
    {
        let t = catalog.get(id(m, "type_id") as i32);
        // Killmail imports mark modules active without knowing their actual state.
        // Compare combat movement capability with cloak/immobilising modes off.
        if [330, 515, 658, 588, 815, 590, 4127, 4184].contains(&id(t, "group")) {
            m["state"] = json!("Online");
            inactive_movement_modules.push(t["name"].clone());
            continue;
        }
        let name = s(t, "name").to_lowercase();
        let kind = if name.contains("microwarpdrive") {
            "mwd"
        } else if name.contains("afterburner") {
            "ab"
        } else {
            continue;
        };
        props.push((i, kind));
        m["state"] = json!("Online");
    }
    let evaluate = |fit: Value, engine: &mut Engine| {
        engine.evaluate(match own {
            Some(settings) => settings.own_request(fit, false),
            None => json!({"fit":fit,"skill_level":5}),
        })
    };
    let base = evaluate(fit.clone(), engine)?;
    let mut speeds = json!({"base_mps":n(&base["hull"]["attributes"],"maxVelocity"),"skill_basis":if own.is_some(){"character"}else{"all_v"},"inactive_movement_modules":inactive_movement_modules,"ship_mode":ship_mode});
    for (i, kind) in props {
        let mut trial = fit.clone();
        trial["modules"][i]["state"] = json!("Active");
        let trial = evaluate(trial, engine)?;
        let v = n(&trial["hull"]["attributes"], "maxVelocity");
        let key = format!("{kind}_mps");
        speeds[&key] = json!(v.max(n(&speeds, &key)));
    }
    Ok(speeds)
}

pub struct SpeedRow {
    pub label: String,
    pub enemy: Option<f64>,
    pub own: Option<f64>,
}
pub fn speed_rows(enemy: &Value, own: &Value) -> Vec<SpeedRow> {
    fn value(v: &Value, key: &str) -> Option<f64> {
        v[key].as_f64().filter(|v| v.is_finite() && *v > 0.)
    }
    fn mode<'a>(v: &Value, preferred: &'a str) -> &'a str {
        if v[format!("{preferred}_mps")].as_f64().is_some() {
            return preferred;
        }
        ["ab", "mwd"]
            .into_iter()
            .filter(|m| v[format!("{m}_mps")].as_f64().is_some())
            .max_by(|a, b| n(v, &format!("{a}_mps")).total_cmp(&n(v, &format!("{b}_mps"))))
            .unwrap_or("base")
    }
    let mut rows = vec![SpeedRow {
        label: "База".into(),
        enemy: value(enemy, "base_mps"),
        own: value(own, "base_mps"),
    }];
    let mut pairs = Vec::new();
    for kind in ["ab", "mwd"] {
        if value(enemy, &format!("{kind}_mps")).is_none()
            && value(own, &format!("{kind}_mps")).is_none()
        {
            continue;
        }
        let e = mode(enemy, kind);
        let o = mode(own, kind);
        if pairs.contains(&(e, o)) {
            continue;
        }
        pairs.push((e, o));
        let title = |v: &str| match v {
            "ab" => "AB",
            "mwd" => "MWD",
            _ => "база",
        };
        let label = if e == o || value(own, "base_mps").is_none() {
            title(e).into()
        } else if value(enemy, "base_mps").is_none() {
            title(o).into()
        } else {
            format!("{}/{}", title(e), title(o))
        };
        rows.push(SpeedRow {
            label,
            enemy: value(enemy, &format!("{e}_mps")),
            own: value(own, &format!("{o}_mps")),
        });
    }
    rows
}

/// Group by actual size and fitted optimal/falloff rather than imply all neuts
/// reach the longest one. The displayed bound follows the existing +.25 falloff convention.
pub fn neutralizers(result: &Value) -> Value {
    let mut groups: BTreeMap<(String, i64), Value> = BTreeMap::new();
    for m in array(&result["modules"])
        .iter()
        .filter(|m| active(m) && id(m, "group") == 71)
    {
        let name = s(m, "name").to_lowercase();
        let size = if name.contains("capital") {
            "Capital"
        } else if name.contains("heavy") || name.contains("large") {
            "Heavy"
        } else if name.contains("medium") {
            "Medium"
        } else {
            "Light"
        };
        let optimal = n(&m["attributes"], "maxRange");
        let falloff = n(&m["attributes"], "falloff");
        let range = optimal + 0.25 * falloff;
        let key = (size.to_string(), (range * 1000.).round() as i64);
        let g=groups.entry(key).or_insert_with(||json!({"size":size,"count":0,"range_m":range,"optimal_m":optimal,"falloff_m":falloff,"modules":[]}));
        g["count"] = json!(id(g, "count") + 1);
        g["modules"].as_array_mut().unwrap().push(m["name"].clone());
    }
    let mut groups: Vec<_> = groups.into_values().collect();
    groups.sort_by(|a, b| n(b, "range_m").total_cmp(&n(a, "range_m")));
    let label = groups
        .iter()
        .map(|g| {
            format!(
                "{}× {} · до {:.1} км",
                id(g, "count"),
                s(g, "size"),
                n(g, "range_m") / 1000.
            )
            .replace('.', ",")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut hint="Установленные активные нейтрализаторы выбранного фита, навыки V. Количество и размер сгруппированы по дальности. В зоне falloff эффект постепенно снижается; показан optimal + ¼ falloff. Это не жёсткий предел действия.".to_string();
    for g in &groups {
        hint += &format!(
            "\n{}× {}: optimal {:.1} км, falloff {:.1} км",
            id(g, "count"),
            s(g, "size"),
            n(g, "optimal_m") / 1000.,
            n(g, "falloff_m") / 1000.
        );
    }
    json!({"label":if label.is_empty(){"НЕТ В ФИТЕ"}else{&label},"groups":groups,"hint":hint})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missile_upper_range_includes_acceleration_tick_rounding_and_fof_limit() {
        let mut c = json!({"attributes":{"maxVelocity":1000.,"explosionDelay":10200.,"mass":1000.,"agility":100.}});
        assert!((missile_range(&c, 0.).unwrap() - 10950.).abs() < 1e-8);
        // An integer lifetime must not spuriously gain an extra second.
        c["attributes"]["explosionDelay"] = json!(10000.);
        assert!((missile_range(&c, 0.).unwrap() - 9950.).abs() < 1e-8);
        c["attributes"]["maxFOFTargetRange"] = json!(8000.);
        assert_eq!(missile_range(&c, 100.), Some(8000.));
    }
}
