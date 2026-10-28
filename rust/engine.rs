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

/// Killmails identify the Abyssal type, not the original item or its rolled values.
/// Use an explicit, repeatable estimate instead of the empty dynamic SDE attributes.
fn abyssal_surrogate(sde: &sde::Sde, type_id: i32) -> Result<Option<i32>, String> {
    let Some(module) = sde.get_type(type_id) else {
        return Ok(None);
    };
    if module.category_id() != 7 || !module.name().contains("Abyssal ") {
        return Ok(None);
    }
    let plain = module.name().replace("Abyssal ", "");
    let special = match plain.as_str() {
        "Small Armor Plates" => Some("200mm Steel Plates II"),
        "Medium Armor Plates" => Some("800mm Steel Plates II"),
        "Large Armor Plates" => Some("1600mm Steel Plates II"),
        "Mining Laser" => Some("Miner II"),
        // These families have no T2 version in the bundled SDE.
        "Deep Core Mining Laser" => Some("Deep Core Mining Laser I"),
        "Deep Core Strip Miner" => Some("ORE Deep Core Strip Miner"),
        _ => None,
    };
    let candidates = if let Some(name) = special {
        vec![name.to_owned()]
    } else {
        vec![format!("{plain} II"), format!("{plain} I"), plain]
    };
    for name in candidates {
        if let Some(base) = sde.type_name_to_id(&name).and_then(|id| sde.get_type(id))
            && base.published()
            && base.category_id() == module.category_id()
            && base.group_id() == module.group_id()
            && !base.name().contains("Abyssal")
        {
            return Ok(Some(base.id()));
        }
    }
    Err(format!("Не найден обычный аналог для {}", module.name()))
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

/// Tactical destroyer modes are SDE dogma items, not a fixed speed multiplier.
pub fn propulsion_mode(ship: i32) -> Result<Option<Value>, String> {
    let data = static_data()?;
    let hull = data.sde.get_type(ship).ok_or("Неизвестный корпус")?;
    data.sde
        .type_name_to_id(&format!("{} Propulsion Mode", hull.name()))
        .map(|mode| type_json(&data.sde, mode))
        .transpose()
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
        if raw
            .lines()
            .skip(1)
            .any(|line| line.trim().starts_with("Неизвестный модуль"))
        {
            return Err("В снимке фита ESI потеряны названия предметов. Обновите персонажа и выберите фит заново".into());
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
    let mut approximations = Vec::new();
    for (index, module) in fit.modules.iter_mut().enumerate() {
        if let Some(estimated) = abyssal_surrogate(sde, module.type_id)? {
            let original = type_json(sde, module.type_id)?;
            let surrogate = type_json(sde, estimated)?;
            approximations.push(json!({"module_index":index,"original_type_id":module.type_id,
                "original_name":original["name"],"estimated_type_id":estimated,"estimated_name":surrogate["name"],
                "slot":format!("{:?}",module.slot.r#type).to_lowercase(),"slot_index":module.slot.index,
                "basis":"original_unknown_standard_analogue","tech_level":surrogate["attributes"]["techLevel"]}));
            module.type_id = estimated;
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
    if let Some(mode_id) = fit.ship_mode {
        let hull = sde.get_type(fit.ship_type_id).unwrap();
        let valid = ["Defense Mode", "Propulsion Mode", "Sharpshooter Mode"]
            .iter()
            .any(|mode| sde.type_name_to_id(&format!("{} {mode}", hull.name())) == Some(mode_id));
        if !valid {
            return Err("Режим не соответствует корпусу".into());
        }
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
    let mut fit_json = json!({"ship_type_id": fit.ship_type_id,"ship_mode":fit.ship_mode,
        "modules": fit.modules.iter().map(|m| json!({"type_id":m.type_id,
            "slot":{"type":format!("{:?}",m.slot.r#type),"index":m.slot.index},
            "state":format!("{:?}",m.state), "charge":m.charge.as_ref().map(|c|json!({"type_id":c.type_id}))})).collect::<Vec<_>>(),
        "drones":fit.drones.iter().map(|d|json!({"type_id":d.type_id,"state":format!("{:?}",d.state)})).collect::<Vec<_>>(),
        "implants":fit.implants,"boosters":fit.boosters,"booster_effects":fit.booster_effects});
    // Keep the import intact for subsequent ammo/MWD recalculations and diagnostics.
    for estimate in &approximations {
        fit_json["modules"][estimate["module_index"].as_u64().unwrap() as usize]["type_id"] =
            estimate["original_type_id"].clone();
    }
    let info = sde::InfoSde::new(fit, skills, sde);
    let stats = calculate::calculate(&info);
    let mut modules = stats
        .items
        .iter()
        .map(|i| item_json(sde, i))
        .collect::<Result<Vec<_>, _>>()?;
    let mut hull = type_json(sde, stats.hull.type_id)?;
    hull["attributes"] =
        serde_json::to_value(named(sde, &stats.hull.attributes)?).map_err(|e| e.to_string())?;
    for module in &mut modules {
        if let Some(estimate) = approximations
            .iter()
            .find(|a| a["slot"] == module["slot"] && a["slot_index"] == module["slot_index"])
        {
            module["approximation"] = estimate.clone();
        }
    }
    let uncertain_modules: Vec<_> = approximations
        .iter()
        .map(|a| a["original_name"].clone())
        .collect();
    let warnings = if uncertain_modules.is_empty() {
        Vec::<String>::new()
    } else {
        vec!["Присутствуют Abyssal модули, оценка не точна".into()]
    };
    Ok(
        json!({"sde_build": sde.build_number(), "engine": "EVEShipFit v8.1.0", "hull": hull,
        "summary": esf_dogma_engine::rust::Output::new(&info, &stats), "modules": modules,
        "cargo": cargo, "fit": fit_json,"warnings":warnings,"uncertain_modules":uncertain_modules,"approximations":approximations}),
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
    /// ESI cargo includes filaments, deployables and other non-fitting items
    /// omitted from the lightweight catalog. Resolve them without expanding it.
    pub fn esi_item_name(&self, id: i32) -> Result<String, String> {
        if let Some(name) = self.get(id)["name"].as_str() {
            return Ok(name.to_string());
        }
        static_data()?
            .sde
            .get_type(id)
            .map(|item| item.name().to_string())
            .ok_or_else(|| format!("Неизвестный предмет ESI: type_id={id}"))
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
    fn every_published_abyssal_module_has_a_same_family_standard_analogue() {
        let sde = &static_data().unwrap().sde;
        let mut count = 0;
        for original in sde
            .types()
            .filter(|t| t.published() && t.category_id() == 7 && t.name().contains("Abyssal "))
        {
            let base = sde
                .get_type(abyssal_surrogate(sde, original.id()).unwrap().unwrap())
                .unwrap();
            assert_eq!(base.group_id(), original.group_id());
            assert!(!base.name().contains("Abyssal"));
            assert!(base.dogma_attributes().is_some_and(|a| a.len() > 5));
            count += 1;
        }
        assert!(count > 50);
    }
    #[test]
    fn abyssal_fit_calculates_like_its_standard_analogues_and_keeps_original_import() {
        let req: Value =
            serde_json::from_str(include_str!("../resources/fixtures/marshal-0.json")).unwrap();
        let mut engine = Engine::default();
        let estimated = engine.evaluate(req.clone()).unwrap();
        assert!(crate::model::n(&estimated["summary"]["offense"], "dps") > 0.);
        assert_eq!(
            estimated["warnings"][0],
            "Присутствуют Abyssal модули, оценка не точна"
        );
        assert_eq!(estimated["fit"]["modules"], req["fit"]["modules"]);
        let mut standard = req;
        for estimate in crate::model::array(&estimated["approximations"]) {
            let index = estimate["module_index"].as_u64().unwrap() as usize;
            standard["fit"]["modules"][index]["type_id"] = estimate["estimated_type_id"].clone();
        }
        let standard = engine.evaluate(standard).unwrap();
        assert_eq!(estimated["summary"], standard["summary"]);
        assert!(crate::model::array(&standard["warnings"]).is_empty());
        let catalog = Catalog::load().unwrap();
        let incoming = crate::tactics::incoming_summary(&estimated, &estimated["fit"], &catalog);
        assert!(incoming["percentages"].is_array() && incoming["uncertain"] == true);
        let own = engine
            .evaluate(json!({"raw_fit": include_str!("../resources/example-tristan.eft")}))
            .unwrap();
        let advice = crate::tactics::advise(
            &own,
            &estimated,
            &mut engine,
            &catalog,
            &crate::model::Settings::default(),
            None,
            0.,
        )
        .unwrap();
        assert!(advice.own_dps > 0. && advice.low.is_finite() && advice.high >= advice.low);
    }
    #[test]
    fn abyssal_detection_includes_propulsion_and_damage_control_without_effect_6556() {
        let result = Engine::default().evaluate(json!({"raw_fit":"[Rifter, approximate]\nAbyssal Damage Control\n\n1MN Abyssal Afterburner\nAbyssal Stasis Webifier\n"})).unwrap();
        let replacements = crate::model::array(&result["approximations"]);
        for expected in [
            "Damage Control II",
            "1MN Afterburner II",
            "Stasis Webifier II",
        ] {
            assert!(
                replacements.iter().any(|a| a["estimated_name"] == expected),
                "{expected}"
            );
        }
        assert!(crate::model::n(&result["hull"]["attributes"], "maxVelocity") > 500.);
        assert!(crate::model::n(&result["summary"]["defense"]["shield"]["resist"], "em") > 0.);
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
