//! Характеристики фитов через EVEShipFit. Справочники встроены в EXE.
use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::{Arc, OnceLock};

use esf_dogma_engine::info::InfoName;
use esf_dogma_engine::{calculate, data_types, eft, sde};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    op: String,
    raw_fit: Option<String>,
    fit: Option<data_types::EsfFit>,
    #[serde(default = "five")]
    skill_level: i32,
    skills: Option<BTreeMap<i32, i32>>,
    #[serde(default)]
    implants: Vec<i32>,
    #[serde(default)]
    boosters: Vec<i32>,
    #[serde(default)]
    booster_effects: Vec<i32>,
}
fn five() -> i32 {
    5
}

fn named(
    sde: &sde::Sde,
    attrs: &BTreeMap<i32, calculate::item::Attribute>,
) -> Result<BTreeMap<String, f64>, String> {
    attrs
        .iter()
        .map(|(id, attr)| {
            let n = sde
                .get_dogma_attribute(*id)
                .map(|a| a.name().to_owned())
                .unwrap_or(id.to_string());
            let v = attr.value.get().ok_or(format!("uncalculated {n}"))?;
            if !v.is_finite() {
                return Err(format!("nonfinite {n}"));
            }
            Ok((n, v))
        })
        .collect()
}

fn type_json(sde: &sde::Sde, id: i32) -> Result<Value, String> {
    let t = sde.get_type(id).ok_or(format!("unknown type {id}"))?;
    let attrs: BTreeMap<String, f64> = t
        .dogma_attributes()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            sde.get_dogma_attribute(a.attribute_id())
                .map(|d| (d.name().to_string(), a.value() as f64))
        })
        .collect();
    let effects: Vec<_> = t
        .dogma_effects()
        .into_iter()
        .flatten()
        .map(|a| a.effect_id())
        .collect();
    let side_effects: Vec<_> = effects.iter().filter_map(|id| sde.get_dogma_effect(*id))
        .filter(|effect| effect.fitting_usage_chance_attribute_id() > 0)
        .map(|effect| json!({"id":effect.id(),"name":effect.display_name().unwrap_or(effect.name())})).collect();
    Ok(
        json!({"id": id, "name": t.name(), "category": t.category_id(), "group": t.group_id(),
        "attributes": attrs, "effects": effects, "side_effects":side_effects, "published": t.published()}),
    )
}

fn item_json(sde: &sde::Sde, item: &calculate::item::Item) -> Result<Value, String> {
    let mut out = type_json(sde, item.type_id)?;
    out["attributes"] =
        serde_json::to_value(named(sde, &item.attributes)?).map_err(|e| e.to_string())?;
    out["state"] = json!(format!("{:?}", item.state).to_lowercase());
    out["slot"] = json!(format!("{:?}", item.slot.r#type).to_lowercase());
    out["slot_index"] = json!(item.slot.index);
    out["charge"] = item
        .charge
        .as_ref()
        .map(|c| item_json(sde, c))
        .transpose()?
        .unwrap_or(Value::Null);
    Ok(out)
}

struct StaticData {
    sde: sde::Sde<'static>,
    names: sde::Names<'static>,
}

fn static_data() -> Result<&'static StaticData, String> {
    // FlatBuffer и индексы неизменяемы. Проверяем их один раз за процесс,
    // иначе каждый новый вариант боеприпасов заново читает весь справочник.
    static DATA: OnceLock<Result<StaticData, String>> = OnceLock::new();
    DATA.get_or_init(|| {
        Ok(StaticData {
            sde: sde::Sde::new(include_bytes!("../native/data/sde.dat"))?,
            names: sde::Names::new(include_bytes!("../native/data/names.dat"))?,
        })
    })
    .as_ref()
    .map_err(Clone::clone)
}

fn run(req: Request) -> Result<Value, String> {
    let data = static_data()?;
    let sde = &data.sde;
    if req.op == "catalog" {
        let types: Vec<_> = sde
            .types()
            .filter(|t| t.published() && [6, 7, 8, 16, 18, 20, 32, 66].contains(&t.category_id()))
            .map(|t| type_json(sde, t.id()))
            .collect::<Result<_, _>>()?;
        return Ok(json!({"sde_build": sde.build_number(), "types": types}));
    }
    if !(0..=5).contains(&req.skill_level) {
        return Err("invalid skill level".into());
    }
    let name_info = sde::InfoNameSde::new(sde, Some(&data.names))?;
    let (mut fit, cargo) = if let Some(raw) = req.raw_fit {
        let raw = raw.trim().trim_start_matches('\u{feff}').trim();
        if raw.len() > 131072 || raw.lines().count() > 2048 {
            return Err("fit too large".into());
        }
        // Upstream expands drone quantities. Bound before entering its parser.
        let mut drones = 0i64;
        for line in raw.lines().skip(1) {
            if let Some((name, count)) = line.trim().rsplit_once(' ')
                && let Some(q) = count.strip_prefix('x')
            {
                let q = q.parse::<i64>().map_err(|_| "invalid quantity")?;
                if !(1..=1_000_000_000).contains(&q) {
                    return Err("invalid quantity".into());
                }
                let id = name_info.type_name_to_id(name);
                if sde.get_type(id).is_some_and(|t| t.category_id() == 18) {
                    drones += q;
                    if drones > 100 {
                        return Err("too many drones".into());
                    }
                }
            }
        }
        let parsed = eft::load_eft(&name_info, &raw.replace('\r', ""))?;
        let cargo: Vec<_> = parsed
            .cargo
            .iter()
            .map(|c| json!({"type_id": c.type_id, "quantity": c.quantity}))
            .collect();
        (parsed.esf_fit, cargo)
    } else {
        (req.fit.ok_or("fit required")?, Vec::new())
    };
    fit.implants = req.implants;
    fit.boosters = req.boosters;
    fit.booster_effects = req.booster_effects;
    let mut implant_slots = BTreeSet::new();
    let mut booster_slots = BTreeSet::new();
    for (types, attribute, slots) in [
        (&fit.implants, "implantness", &mut implant_slots),
        (&fit.boosters, "boosterness", &mut booster_slots),
    ] {
        for type_id in types {
            let t = type_json(sde, *type_id)?;
            let slot = crate::model::n(&t["attributes"], attribute) as i32;
            if t["category"] != 20 || slot <= 0 || !slots.insert(slot) {
                return Err(format!("Неверный или повторяющийся слот: {}", t["name"]));
            }
        }
    }
    // EFT перечисляет запас боеприпасов отдельно. Если заряд не указан у модуля,
    // берём первый совместимый из трюма: сам импорт не должен давать нулевой DPS.
    for module in &mut fit.modules {
        if module.charge.is_none() {
            let module_type = type_json(sde, module.type_id)?;
            for item in &cargo {
                let tid = crate::model::id(item, "type_id") as i32;
                let charge_type = type_json(sde, tid)?;
                if crate::tactics::accepts_charge(&module_type, &charge_type) {
                    module.charge = Some(data_types::EsfCharge { type_id: tid });
                    break;
                }
            }
        }
    }
    if fit.modules.len() > 64 || fit.drones.len() > 100 {
        return Err("fit too large".into());
    }
    if !sde
        .get_type(fit.ship_type_id)
        .is_some_and(|t| t.category_id() == 6 && t.published())
    {
        return Err("unknown hull".into());
    }
    let mut slots = BTreeSet::new();
    for m in &fit.modules {
        if !slots.insert((format!("{:?}", m.slot.r#type), m.slot.index))
            || !(0..=7).contains(&m.slot.index)
        {
            return Err("invalid or duplicate slot".into());
        }
        if !sde
            .get_type(m.type_id)
            .is_some_and(|t| [7, 32, 66].contains(&t.category_id()))
        {
            return Err("unknown module".into());
        }
        if let Some(c) = &m.charge
            && !sde
                .get_type(c.type_id)
                .is_some_and(|t| t.category_id() == 8)
        {
            return Err("unknown charge".into());
        }
    }
    for d in &fit.drones {
        if !sde
            .get_type(d.type_id)
            .is_some_and(|t| t.category_id() == 18)
        {
            return Err("unknown drone".into());
        }
    }
    let mut skills: BTreeMap<i32, i32> = sde
        .types()
        .filter(|t| t.category_id() == 16)
        .map(|t| {
            (
                t.id(),
                if req.skills.is_some() {
                    0
                } else {
                    req.skill_level
                },
            )
        })
        .collect();
    if let Some(levels) = req.skills {
        for (id, level) in levels {
            if !skills.contains_key(&id) || !(0..=5).contains(&level) {
                return Err("invalid skill".into());
            }
            skills.insert(id, level);
        }
    }
    let fit_json = json!({"ship_type_id": fit.ship_type_id,
        "modules": fit.modules.iter().map(|m| json!({"type_id":m.type_id,
            "slot":{"type":format!("{:?}",m.slot.r#type),"index":m.slot.index},
            "state":format!("{:?}",m.state), "charge":m.charge.as_ref().map(|c|json!({"type_id":c.type_id}))})).collect::<Vec<_>>(),
        "drones":fit.drones.iter().map(|d|json!({"type_id":d.type_id,"state":format!("{:?}",d.state)})).collect::<Vec<_>>(),
        "implants":fit.implants,"boosters":fit.boosters,"booster_effects":fit.booster_effects});
    let info = sde::InfoSde::new(fit, skills, sde);
    let stats = calculate::calculate(&info);
    let modules = stats
        .items
        .iter()
        .map(|i| item_json(sde, i))
        .collect::<Result<Vec<_>, _>>()?;
    let mut hull = type_json(sde, stats.hull.type_id)?;
    hull["attributes"] =
        serde_json::to_value(named(sde, &stats.hull.attributes)?).map_err(|e| e.to_string())?;
    let uncertain_modules: Vec<_> = modules
        .iter()
        .filter(|m| {
            sde.get_type(crate::model::id(m, "id") as i32)
                .is_some_and(|t| {
                    t.dogma_effects()
                        .into_iter()
                        .flatten()
                        .any(|e| e.effect_id() == 6556)
                })
        })
        .map(|m| m["name"].clone())
        .collect();
    let warnings = if uncertain_modules.is_empty() {
        Vec::<String>::new()
    } else {
        vec!["Параметры Abyssal-модулей отсутствуют в публичной потере; расчёт по справочным значениям может быть неточным".into()]
    };
    Ok(
        json!({"sde_build": sde.build_number(), "engine": "EVEShipFit v8.1.0", "hull": hull,
        "summary": esf_dogma_engine::rust::Output::new(&info, &stats), "modules": modules,
        "cargo": cargo, "fit": fit_json,"warnings":warnings,"uncertain_modules":uncertain_modules}),
    )
}

#[derive(Clone)]
pub struct Catalog {
    pub types: Arc<HashMap<i32, Value>>,
    pub ships: Arc<Vec<(i32, String)>>,
    pub names: Arc<HashMap<String, i32>>,
}
impl Catalog {
    pub fn load() -> Result<Self, String> {
        let mut decoder =
            flate2::read::GzDecoder::new(&include_bytes!("../resources/catalog.json.gz")[..]);
        let mut raw = Vec::new();
        decoder.read_to_end(&mut raw).map_err(|e| e.to_string())?;
        let value: Value = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
        let mut types: HashMap<i32, Value> = value["types"]
            .as_array()
            .ok_or("bad catalog")?
            .iter()
            .map(|t| (t["id"].as_i64().unwrap_or(0) as i32, t.clone()))
            .collect();
        // Импланты и бустеры берём из того же SDE, что использует расчётчик.
        let data = static_data()?;
        for t in data
            .sde
            .types()
            .filter(|t| t.published() && t.category_id() == 20)
        {
            types.insert(t.id(), type_json(&data.sde, t.id())?);
        }
        let names = types
            .iter()
            .map(|(id, t)| (t["name"].as_str().unwrap_or("").to_lowercase(), *id))
            .collect();
        let mut ships: Vec<_> = types
            .iter()
            .filter(|(_, t)| t["category"] == 6)
            .map(|(id, t)| (*id, t["name"].as_str().unwrap_or("").to_string()))
            .collect();
        ships.sort_by(|a, b| a.1.cmp(&b.1));
        Ok(Self {
            types: Arc::new(types),
            ships: Arc::new(ships),
            names: Arc::new(names),
        })
    }
    pub fn get(&self, id: i32) -> &Value {
        self.types.get(&id).unwrap_or(&Value::Null)
    }
    pub fn ship_id(&self, name: &str) -> Option<i32> {
        self.names
            .get(&name.trim().to_lowercase())
            .copied()
            .filter(|id| self.get(*id)["category"] == 6)
    }
    pub fn name(&self, id: i32) -> &str {
        self.get(id)["name"]
            .as_str()
            .unwrap_or("Неизвестный модуль")
    }
}
#[derive(Default)]
pub struct Engine {
    cache: HashMap<String, Value>,
    #[cfg(test)]
    pub calculations: usize,
}
impl Engine {
    pub fn evaluate(&mut self, request: Value) -> Result<Value, String> {
        let key = request.to_string();
        if let Some(v) = self.cache.get(&key) {
            return Ok(v.clone());
        }
        let req: Request = serde_json::from_value(request.clone()).map_err(|e| e.to_string())?;
        #[cfg(test)]
        {
            self.calculations += 1;
        }
        let result = std::panic::catch_unwind(|| run(req))
            .unwrap_or_else(|_| Err("Не удалось рассчитать фит".into()))?;
        if self.cache.len() > 256 {
            self.cache.clear();
        }
        self.cache.insert(key, result.clone());
        Ok(result)
    }
}

#[cfg(test)]
mod resistance_regressions {
    use super::*;
    fn fixture(raw: &str) -> Value {
        Engine::default()
            .evaluate(serde_json::from_str(raw).unwrap())
            .unwrap()
    }
    #[test]
    fn polarized_resonances_are_fractions_and_override_resistance_modules() {
        let result = fixture(include_str!("../resources/fixtures/marshal-0.json"));
        for layer in ["shield", "armor", "structure"] {
            for key in ["em", "therm", "kin", "expl"] {
                assert_eq!(
                    result["summary"]["defense"][layer]["resist"][key].as_f64(),
                    Some(0.)
                );
            }
        }
        assert!(!crate::model::array(&result["warnings"]).is_empty());
        let mut req: Value =
            serde_json::from_str(include_str!("../resources/fixtures/marshal-1.json")).unwrap();
        let polarized: Value =
            serde_json::from_str(include_str!("../resources/fixtures/marshal-0.json")).unwrap();
        let c = Catalog::load().unwrap();
        req["fit"]["modules"] = json!(
            crate::model::array(&req["fit"]["modules"])
                .iter()
                .map(|m| {
                    if c.name(crate::model::id(m, "type_id") as i32)
                        .contains("Rapid Heavy")
                    {
                        let mut m = m.clone();
                        m["type_id"] = polarized["fit"]["modules"][6]["type_id"].clone();
                        m["charge"] = polarized["fit"]["modules"][6]["charge"].clone();
                        m
                    } else {
                        m.clone()
                    }
                })
                .collect::<Vec<_>>()
        );
        let result = Engine::default().evaluate(req).unwrap();
        assert!(crate::model::n(&result["summary"]["offense"], "dps") > 0.);
        for key in ["em", "therm", "kin", "expl"] {
            assert_eq!(result["summary"]["defense"]["armor"]["resist"][key], 0.);
        }
    }
    #[test]
    fn marshal_and_gnosis_omni_resists_match_their_real_fits_and_bare_hulls() {
        for (raw, ship, layer, expected, base) in [
            (
                include_str!("../resources/fixtures/marshal-1.json"),
                44996,
                "armor",
                79.31208054560844,
                40.,
            ),
            (
                include_str!("../resources/fixtures/gnosis-1.json"),
                3756,
                "shield",
                64.8855805398874,
                27.5,
            ),
        ] {
            let result = fixture(raw);
            let bare = Engine::default()
                .evaluate(
                    json!({"fit":{"ship_type_id":ship,"modules":[],"drones":[]},"skill_level":5}),
                )
                .unwrap();
            for key in ["em", "therm", "kin", "expl"] {
                assert!(
                    (result["summary"]["defense"][layer]["resist"][key]
                        .as_f64()
                        .unwrap()
                        - expected)
                        .abs()
                        < 0.001
                );
                assert!(
                    (bare["summary"]["defense"][layer]["resist"][key]
                        .as_f64()
                        .unwrap()
                        - base)
                        .abs()
                        < 0.001
                );
            }
        }
        let rifter = Engine::default()
            .evaluate(json!({"fit":{"ship_type_id":587,"modules":[],"drones":[]}}))
            .unwrap();
        assert_ne!(
            rifter["summary"]["defense"]["shield"]["resist"]["em"],
            rifter["summary"]["defense"]["shield"]["resist"]["expl"]
        );
    }
    #[test]
    fn incoming_damage_exposes_minor_damage_and_assumed_ammunition() {
        let c = Catalog::load().unwrap();
        let mut engine = Engine::default();
        let req: Value =
            serde_json::from_str(include_str!("../resources/fixtures/marshal-1.json")).unwrap();
        let result = engine.evaluate(req.clone()).unwrap();
        let mix = crate::tactics::incoming_summary(&result, &req["fit"], &c);
        let kin = mix["percentages"][2].as_f64().unwrap();
        let explosive = mix["percentages"][3].as_f64().unwrap();
        assert!((kin - 92.72).abs() < 0.02 && (explosive - 7.28).abs() < 0.02);
        assert!(
            crate::model::array(&mix["sources"])
                .iter()
                .any(|s| s["charge"] == "Scourge Fury Heavy Missile")
        );
        assert!(
            crate::model::array(&mix["sources"])
                .iter()
                .all(|s| s["assumed_charge"] == false)
        );
        let mut empty = req["fit"].clone();
        for m in empty["modules"].as_array_mut().unwrap() {
            m["charge"] = Value::Null;
        }
        let result = engine
            .evaluate(json!({"fit":crate::tactics::fill_charges(&empty,&c)}))
            .unwrap();
        let mix = crate::tactics::incoming_summary(&result, &empty, &c);
        assert!(
            crate::model::array(&mix["sources"])
                .iter()
                .any(|s| s["assumed_charge"] == true)
        );
        assert_eq!(crate::tactics::resistance_note(&[79.312; 4]), "РАВНОМЕРНО");
        assert_eq!(
            crate::tactics::resistance_note(&[20., 40., 50., 60.]),
            "СЛАБЕЕ EM"
        );
    }
}
