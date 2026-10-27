//! Фиты из killmail: нормализация слотов, исключение флотов и подсчёт долей.

use crate::{
    engine::Catalog,
    model::{Settings, array, id, n, s},
};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

pub fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))[..24].into()
}
pub fn family(name: &str, group: i64) -> &'static str {
    let name = name.to_lowercase();
    if name.contains("autocannon") || name.contains("repeating cannon") {
        "Autocannon"
    } else if name.contains("artillery") || name.contains("howitzer") {
        "Artillery"
    } else if name.contains("blaster")
        || group == 74
            && name
                .split_whitespace()
                .any(|word| ["electron", "ion", "neutron"].contains(&word))
    {
        "Blaster"
    } else if name.contains("rail") || group == 74 {
        "Railgun"
    } else if name.contains("pulse") && (group == 53 || name.contains("laser")) {
        "Pulse"
    } else if group == 53 || name.contains("beam laser") || name.contains("beam maser") {
        "Beam"
    } else if name.contains("launcher")
        && ["missile", "rocket", "torpedo", "heavy assault"]
            .iter()
            .any(|w| name.contains(w))
    {
        "Missiles"
    } else if name.contains("disintegrator") {
        "Disintegrator"
    } else if name.contains("vorton") {
        "Vorton"
    } else if group == 55 {
        "Autocannon"
    } else {
        ""
    }
}
pub fn fingerprint(fit: &Value) -> String {
    let mut parts: Vec<_> = array(&fit["modules"])
        .iter()
        .map(|m| (s(&m["slot"], "type"), id(m, "type_id")))
        .collect();
    parts.sort();
    hash(&json!([fit["ship_type_id"], parts]).to_string())
}
pub fn normalize(mail: &Value, catalog: &Catalog) -> Option<Value> {
    let v = &mail["victim"];
    let ship = id(v, "ship_type_id") as i32;
    let hull = catalog.get(ship);
    if hull["category"] != 6 {
        return None;
    }
    let items = array(&v["items"]);
    let mut modules = Vec::new();
    let mut slots = HashSet::new();
    let mut drones = Vec::new();
    let mut bandwidth = n(&hull["attributes"], "droneBandwidth");
    let racks = [
        (11, 18, "Low"),
        (19, 26, "Medium"),
        (27, 34, "High"),
        (92, 99, "Rig"),
        (125, 132, "SubSystem"),
    ];
    for item in items {
        let tid = id(item, "item_type_id") as i32;
        let flag = id(item, "flag");
        let t = catalog.get(tid);
        if t["category"] == 18 && flag == 87 {
            let used = n(&t["attributes"], "droneBandwidthUsed");
            let count =
                (id(item, "quantity_dropped") + id(item, "quantity_destroyed")).clamp(0, 100);
            for _ in 0..count {
                if drones.len() < 5 && used > 0. && used <= bandwidth {
                    drones.push(json!({"type_id":tid,"state":"Active"}));
                    bandwidth -= used;
                }
            }
        }
        if ![7, 32, 66].contains(&id(t, "category")) {
            continue;
        }
        for (low, high, rack) in racks {
            if (low..=high).contains(&flag) {
                if !slots.insert((rack, flag - low)) {
                    return None;
                }
                let charge = items
                    .iter()
                    .find(|i| {
                        id(i, "flag") == flag
                            && catalog.get(id(i, "item_type_id") as i32)["category"] == 8
                    })
                    .map(|i| json!({"type_id":i["item_type_id"]}));
                modules.push(json!({"type_id":tid,"slot":{"type":rack,"index":flag-low},"state":"Active","charge":charge}));
                break;
            }
        }
    }
    if modules.is_empty() {
        return None;
    }
    Some(json!({"ship_type_id":ship,"modules":modules,"drones":drones}))
}
pub fn describe(fit: &Value, catalog: &Catalog) -> (String, String) {
    let types: Vec<_> = array(&fit["modules"])
        .iter()
        .map(|m| catalog.get(id(m, "type_id") as i32))
        .collect();
    let names: Vec<_> = types.iter().map(|t| s(t, "name").to_lowercase()).collect();
    let mut weapons: BTreeMap<String, usize> = BTreeMap::new();
    let mut families = HashSet::new();
    for t in &types {
        let f = family(s(t, "name"), id(t, "group"));
        if !f.is_empty() {
            families.insert(f);
            *weapons
                .entry(format!("{f}:{}", n(&t["attributes"], "chargeSize")))
                .or_default() += 1;
        }
    }
    if families.is_empty() && !array(&fit["drones"]).is_empty() {
        families.insert("Drones");
    }
    let prop = if names.iter().any(|x| x.contains("microwarpdrive")) {
        "MWD"
    } else if names.iter().any(|x| x.contains("afterburner")) {
        "AB"
    } else {
        "Без разгона"
    };
    let armor = names.iter().any(|x| {
        ["armor repairer", "ancillary armor", "plates"]
            .iter()
            .any(|w| x.contains(w))
    });
    let shield = names.iter().any(|x| {
        ["shield booster", "shield extender", "ancillary shield"]
            .iter()
            .any(|w| x.contains(w))
    });
    let tank = if armor {
        "Armor"
    } else if shield {
        "Shield"
    } else {
        "Hull / Speed"
    };
    let mut controls = Vec::new();
    for (label, tokens) in [
        ("web", vec!["webifier", "stasis web"]),
        ("scram", vec!["scrambler"]),
        ("point", vec!["warp disruptor"]),
        ("neut", vec!["neutralizer"]),
        ("nos", vec!["nosferatu"]),
    ] {
        let count = names
            .iter()
            .filter(|name| tokens.iter().any(|t| name.contains(t)))
            .count();
        if count > 0 {
            controls.push(format!("{label}×{count}"));
        }
    }
    let mut fs: Vec<_> = families.into_iter().collect();
    fs.sort();
    let weapon = if fs.is_empty() {
        "Без оружия".into()
    } else {
        fs.join(" + ")
    };
    let title = format!("{weapon} / {prop} / {tank}");
    let key = hash(&json!([weapons, prop, tank, controls]).to_string());
    let suffix = if controls.is_empty() {
        String::new()
    } else {
        format!(" · {}", controls.join(" / "))
    };
    (title + &suffix, key)
}
pub fn fleet_module(fit: &Value, catalog: &Catalog) -> bool {
    array(&fit["modules"]).iter().any(|m| {
        let name = catalog.name(id(m, "type_id") as i32).to_lowercase();
        [
            "command burst",
            "remote shield booster",
            "remote armor repairer",
            "remote capacitor transmitter",
            "siege module",
            "triage module",
        ]
        .iter()
        .any(|w| name.contains(w))
    })
}
pub fn time(mail: &Value) -> i64 {
    DateTime::parse_from_rfc3339(s(mail, "killmail_time"))
        .map(|t| t.timestamp())
        .unwrap_or(0)
}
pub fn fleet_ids(mails: &[Value]) -> HashSet<i64> {
    let mut groups: HashMap<(i64, i64, i64), Vec<(i64, i64)>> = HashMap::new();
    for m in mails {
        let corp = id(&m["victim"], "corporation_id");
        if corp < 98000000 {
            continue;
        }
        groups
            .entry((
                id(m, "solar_system_id"),
                corp,
                id(&m["victim"], "ship_type_id"),
            ))
            .or_default()
            .push((time(m), id(m, "killmail_id")));
    }
    let mut out = HashSet::new();
    for events in groups.values_mut() {
        events.sort();
        let mut left = 0;
        for right in 0..events.len() {
            while events[right].0 - events[left].0 > 900 {
                left += 1;
            }
            if right - left + 1 >= 3 {
                for e in &events[left..=right] {
                    out.insert(e.1);
                }
            }
        }
    }
    out
}
pub fn eligible(
    mail: &Value,
    fit: &Value,
    catalog: &Catalog,
    settings: &Settings,
    fleet: &HashSet<i64>,
) -> bool {
    if mail["zkb"]["npc"] == true
        || mail["zkb"]["awox"] == true
        || fleet.contains(&id(mail, "killmail_id"))
    {
        return false;
    }
    let attackers: HashSet<_> = array(&mail["attackers"])
        .iter()
        .map(|a| id(a, "character_id"))
        .filter(|id| *id > 0)
        .collect();
    !attackers.is_empty()
        && attackers.len() <= settings.max_attackers
        && !fleet_module(fit, catalog)
        && !settings.excluded.contains(&fingerprint(fit))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Variant {
    pub title: String,
    pub key: String,
    pub fit: Value,
    pub count: usize,
    pub percent: Option<f64>,
    pub personal: bool,
    pub examples: Vec<Value>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Population {
    pub variants: Vec<Variant>,
    pub total: usize,
    pub excluded: usize,
    pub unknown: usize,
    pub other: f64,
}
pub fn population(mails: &[Value], catalog: &Catalog, settings: &Settings) -> Population {
    let mut out = Population::default();
    let mut grouped: HashMap<String, Variant> = HashMap::new();
    let fleet = fleet_ids(mails);
    let cutoff = if settings.days > 0 {
        (Utc::now() - TimeDelta::days(settings.days as i64)).timestamp()
    } else {
        0
    };
    for mail in mails {
        if time(mail) < cutoff {
            continue;
        }
        let Some(fit) = normalize(mail, catalog) else {
            out.unknown += 1;
            continue;
        };
        if !eligible(mail, &fit, catalog, settings, &fleet) {
            out.excluded += 1;
            continue;
        }
        let (title, key) = describe(&fit, catalog);
        let v = grouped.entry(key.clone()).or_insert_with(|| Variant {
            title,
            key,
            fit: fit.clone(),
            count: 0,
            percent: None,
            personal: false,
            examples: Vec::new(),
        });
        v.count += 1;
        v.examples.push(fit);
        out.total += 1;
    }
    let mut ranked: Vec<_> = grouped.into_values().collect();
    ranked.sort_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
    let mut cumulative = 0.;
    for mut v in ranked {
        // Знаменатель — вся подходящая выборка, включая варианты за пределами топа.
        // Иначе короткий список создавал бы ложное впечатление полного покрытия.
        let percent = v.count as f64 * 100. / out.total as f64;
        if !out.variants.is_empty()
            && (percent < settings.min_percent || cumulative >= settings.coverage)
        {
            break;
        }
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for f in &v.examples {
            *counts.entry(fingerprint(f)).or_default() += 1;
        }
        if let Some((key, _)) = counts.into_iter().max_by_key(|(_, n)| *n) {
            v.fit = v
                .examples
                .iter()
                .find(|f| fingerprint(f) == key)
                .unwrap()
                .clone();
        }
        v.percent = Some(percent);
        v.examples.clear();
        cumulative += percent;
        out.variants.push(v);
        if out.variants.len() >= settings.max_variants {
            break;
        }
    }
    out.other = if out.total > 0 {
        (100. - cumulative).max(0.)
    } else {
        0.
    };
    out
}
pub fn pilot_variant(
    mails: &[Value],
    pilot: i64,
    ship: i32,
    catalog: &Catalog,
    settings: &Settings,
) -> Option<Variant> {
    let cutoff = (Utc::now() - TimeDelta::days(settings.pilot_days as i64)).timestamp();
    let fleet = fleet_ids(mails);
    for m in mails {
        if id(&m["victim"], "character_id") != pilot
            || id(&m["victim"], "ship_type_id") != ship as i64
            || time(m) < cutoff
        {
            continue;
        }
        if let Some(fit) = normalize(m, catalog)
            && eligible(m, &fit, catalog, settings, &fleet)
        {
            let (title, key) = describe(&fit, catalog);
            return Some(Variant {
                title,
                key,
                fit,
                count: 0,
                percent: None,
                personal: true,
                examples: Vec::new(),
            });
        }
    }
    None
}
pub fn eft(fit: &Value, catalog: &Catalog) -> String {
    let mut lines = vec![
        format!(
            "[{}, Combat Assistant]",
            catalog.name(id(fit, "ship_type_id") as i32)
        ),
        String::new(),
    ];
    for rack in ["Low", "Medium", "High", "Rig", "SubSystem"] {
        let mut modules: Vec<_> = array(&fit["modules"])
            .iter()
            .filter(|m| s(&m["slot"], "type") == rack)
            .collect();
        modules.sort_by_key(|m| id(&m["slot"], "index"));
        for m in modules {
            let mut name = catalog.name(id(m, "type_id") as i32).to_string();
            if !m["charge"].is_null() {
                name += &format!(", {}", catalog.name(id(&m["charge"], "type_id") as i32));
            }
            lines.push(name);
        }
        lines.push(String::new());
    }
    let mut drones: BTreeMap<i32, usize> = BTreeMap::new();
    for d in array(&fit["drones"]) {
        *drones.entry(id(d, "type_id") as i32).or_default() += 1;
    }
    for (tid, count) in drones {
        lines.push(format!("{} x{count}", catalog.name(tid)));
    }
    lines.join("\n").trim().into()
}
