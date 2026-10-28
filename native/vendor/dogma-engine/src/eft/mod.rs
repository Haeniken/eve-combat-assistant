use std::collections::HashMap;

use crate::data_types;
use crate::info::InfoName;

pub struct EftCargo {
    pub type_id: i32,
    pub quantity: i32,
}

pub struct EftFit {
    pub name: String,
    pub esf_fit: data_types::EsfFit,
    pub cargo: Vec<EftCargo>,
}

fn find_slot_type_index(
    info: &impl InfoName,
    type_id: i32,
    module_slots: &mut HashMap<data_types::EsfSlotType, i32>,
) -> Option<(data_types::EsfSlotType, i32)> {
    for effect in info.get_dogma_effects(type_id).into_iter().flatten() {
        match effect.effect_id() {
            11 => {
                let index = module_slots
                    .entry(data_types::EsfSlotType::Low)
                    .or_insert(0);
                *index += 1;
                return Some((data_types::EsfSlotType::Low, *index - 1));
            }
            12 => {
                let index = module_slots
                    .entry(data_types::EsfSlotType::High)
                    .or_insert(0);
                *index += 1;
                return Some((data_types::EsfSlotType::High, *index - 1));
            }
            13 => {
                let index = module_slots
                    .entry(data_types::EsfSlotType::Medium)
                    .or_insert(0);
                *index += 1;
                return Some((data_types::EsfSlotType::Medium, *index - 1));
            }
            2663 => {
                let index = module_slots
                    .entry(data_types::EsfSlotType::Rig)
                    .or_insert(0);
                *index += 1;
                return Some((data_types::EsfSlotType::Rig, *index - 1));
            }
            3772 => {
                let index = module_slots
                    .entry(data_types::EsfSlotType::SubSystem)
                    .or_insert(0);
                *index += 1;
                return Some((data_types::EsfSlotType::SubSystem, *index - 1));
            }
            6306 => {
                let index = module_slots
                    .entry(data_types::EsfSlotType::Service)
                    .or_insert(0);
                *index += 1;
                return Some((data_types::EsfSlotType::Service, *index - 1));
            }
            _ => {}
        }
    }

    None
}

/* Split "<Type Name> x<Quantity>" on its last token, as type names can contain an "x" too. */
fn parse_quantity(line: &str) -> Option<(&str, i32)> {
    let (type_name, quantity) = line.trim().rsplit_once(char::is_whitespace)?;
    let quantity = quantity.strip_prefix('x')?.parse().ok()?;
    Some((type_name.trim(), quantity))
}

/// Разбираем строки независимо: пустые строки и скопированные отступы
/// не должны превращать заряд или скрипт в корабельный модуль.
pub fn load_eft(info: &impl InfoName, eft: &str) -> Result<EftFit, String> {
    let mut lines = eft.trim().trim_start_matches('\u{feff}').trim().lines();
    let header = lines.next().ok_or("Пустой фит")?.trim();
    let header = header
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .ok_or("Заголовок фита должен иметь вид [Корабль, Название]")?;
    let (ship, name) = header
        .split_once(',')
        .ok_or("В заголовке нужны корпус и название фита")?;
    let ship_type_id = info.type_name_to_id(ship.trim());
    if ship_type_id == 0
        || info
            .get_type(ship_type_id)
            .is_none_or(|t| t.category_id() != 6)
    {
        return Err(format!("Неизвестный корпус: {}", ship.trim()));
    }
    let mut result = EftFit {
        name: name.trim().into(),
        esf_fit: data_types::EsfFit {
            ship_type_id,
            modules: Vec::new(),
            drones: Vec::new(),
            implants: Vec::new(),
            boosters: Vec::new(),
            booster_effects: Vec::new(),
        },
        cargo: Vec::new(),
    };
    let mut slots = HashMap::new();
    for line in lines.map(str::trim).filter(|line| !line.is_empty()) {
        if line.starts_with("[Empty") {
            let slot = match line {
                "[Empty High slot]" => data_types::EsfSlotType::High,
                "[Empty Med slot]" => data_types::EsfSlotType::Medium,
                "[Empty Low slot]" => data_types::EsfSlotType::Low,
                "[Empty Rig slot]" => data_types::EsfSlotType::Rig,
                "[Empty Subsystem slot]" => data_types::EsfSlotType::SubSystem,
                _ => return Err(format!("Неизвестный пустой слот: {line}")),
            };
            *slots.entry(slot).or_insert(0) += 1;
            continue;
        }
        let (mut text, quantity) = match parse_quantity(line) {
            Some((name, count)) => {
                if !(1..=1_000_000_000).contains(&count) {
                    return Err("Неверное количество предметов".into());
                }
                (name, Some(count))
            }
            None => (line, None),
        };
        let mut state = data_types::EsfState::Active;
        for (suffix, value) in [
            ("/offline", data_types::EsfState::Passive),
            ("/online", data_types::EsfState::Online),
            ("/active", data_types::EsfState::Active),
            ("/overload", data_types::EsfState::Overload),
        ] {
            if let Some(name) = text.strip_suffix(suffix) {
                text = name.trim_end();
                state = value;
                break;
            }
        }
        let (name, charge) = text
            .split_once(',')
            .map_or((text, None), |(a, b)| (a.trim(), Some(b.trim())));
        let tid = info.type_name_to_id(name);
        if tid == 0 {
            return Err(format!("Неизвестный предмет: {name}"));
        }
        let item = info
            .get_type(tid)
            .ok_or_else(|| format!("Неизвестный предмет: {name}"))?;
        // Количество xN обозначает трюм/дронов даже в смешанном блоке.
        // Заряды и скрипты без xN также относятся к трюму, по одной штуке.
        if quantity.is_some() || ![7, 32, 66].contains(&item.category_id()) {
            let count = quantity.unwrap_or(1);
            if item.category_id() == 18 {
                if result.esf_fit.drones.len() + count as usize > 100 {
                    return Err("Слишком много дронов".into());
                }
                for _ in 0..count {
                    result.esf_fit.drones.push(data_types::EsfDrone {
                        type_id: tid,
                        state: data_types::EsfState::Active,
                    });
                }
            } else {
                result.cargo.push(EftCargo {
                    type_id: tid,
                    quantity: count,
                });
            }
            continue;
        }
        let (slot, index) = find_slot_type_index(info, tid, &mut slots)
            .ok_or_else(|| format!("Модуль {name} не подходит ни в один слот"))?;
        let charge = charge
            .map(|name| {
                let type_id = info.type_name_to_id(name);
                if info.get_type(type_id).is_none_or(|t| t.category_id() != 8) {
                    return Err(format!("Неизвестный заряд: {name}"));
                }
                Ok(data_types::EsfCharge { type_id })
            })
            .transpose()?;
        result.esf_fit.modules.push(data_types::EsfModule {
            type_id: tid,
            slot: data_types::EsfSlot {
                r#type: slot,
                index,
            },
            state,
            charge,
        });
    }
    Ok(result)
}
