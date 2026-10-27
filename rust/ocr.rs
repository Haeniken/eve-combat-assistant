//! Разбор видимого текста цели и подтверждение смены корабля по двум кадрам.

use crate::engine::Catalog;

#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Clone, Debug)]
pub struct TextLine {
    pub text: String,
    pub words: Vec<Word>,
}
#[derive(Clone, Default, Debug)]
pub struct Document {
    pub lines: Vec<TextLine>,
}
impl Document {
    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Ник берётся только из Selected Item: список захваченных целей не выбирает активную.
pub fn selected_pilot(text: &str) -> Option<String> {
    for line in text.lines() {
        let candidate = line
            .trim()
            .trim_start_matches("Pilot:")
            .trim_start_matches("Пилот:")
            .trim();
        let lower = candidate.to_lowercase();
        if candidate.is_empty()
            || candidate.len() > 100
            || candidate.contains(':')
            || [
                "selected item",
                "выбранный объект",
                "security status",
                "статус безопасности",
            ]
            .iter()
            .any(|word| lower.contains(word))
            || candidate
                .split_whitespace()
                .any(|word| ["km", "км", "m", "м"].contains(&word.to_lowercase().as_str()))
            || !candidate.chars().any(char::is_alphabetic)
            || candidate
                .chars()
                .any(|c| !(c.is_alphanumeric() || [' ', '\'', '-'].contains(&c)))
        {
            continue;
        }
        return Some(candidate.into());
    }
    None
}

/// В карточках OCR может объединить несколько колонок в одну строку.
/// Ищем точный ник и корпус со звёздочкой ниже в той же колонке.
pub fn selected_card(selected: &str, cards: &Document, catalog: &Catalog) -> Option<Target> {
    let pilot = selected_pilot(selected)?;
    let normalized = pilot
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let mut anchors = Vec::new();
    for line in &cards.lines {
        for start in 0..line.words.len() {
            for end in start + 1..=line.words.len().min(start + 8) {
                let words = &line.words[start..end];
                if words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_lowercase()
                    == normalized
                {
                    let left = words[0].x;
                    let right = words.iter().map(|w| w.x + w.width).fold(left, f32::max);
                    let top = words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
                    let height = words.iter().map(|w| w.height).fold(0., f32::max);
                    if start > 0
                        && left - (line.words[start - 1].x + line.words[start - 1].width)
                            < height * 1.5
                    {
                        continue;
                    }
                    if end < line.words.len() && line.words[end].x - right < height * 1.5 {
                        continue;
                    }
                    anchors.push(((left + right) / 2., top, height));
                }
            }
        }
    }
    let [(anchor_x, anchor_y, height)] = anchors.as_slice() else {
        return None;
    };
    let mut matches = Vec::new();
    for line in &cards.lines {
        for start in 0..line.words.len() {
            for end in start + 1..=line.words.len().min(start + 8) {
                let words = &line.words[start..end];
                if words
                    .iter()
                    .any(|word| word.height <= 0. || !word.x.is_finite() || !word.y.is_finite())
                {
                    continue;
                }
                let text = words
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.trim_end().ends_with('*') {
                    continue;
                }
                let Some(tid) = catalog.ship_id(text.trim_end().trim_end_matches('*').trim())
                else {
                    continue;
                };
                let x = (words[0].x + words.last()?.x + words.last()?.width) / 2.;
                let y = words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
                // Отступы считаются от размера шрифта, а не от конкретного разрешения EVE.
                if y > *anchor_y
                    && y < anchor_y + height * 6.
                    && (x - anchor_x).abs() <= height * 2.5
                {
                    matches.push(tid);
                }
            }
        }
    }
    matches.sort();
    matches.dedup();
    let [tid] = matches.as_slice() else {
        return None;
    };
    let ship = catalog.name(*tid).to_string();
    // Дистанция в списке карточек может принадлежать соседу; читаем её справа.
    let mut target = parse(&format!("{selected}\n{ship}"), catalog)?;
    target.pilot = pilot;
    target.ship = ship;
    Some(target)
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Target {
    pub ship: String,
    pub pilot: String,
    pub distance: Option<f64>,
    pub angular: f64,
}
impl Target {
    pub fn identity(&self) -> (&str, &str) {
        (&self.ship, &self.pilot)
    }
}
pub fn parse(text: &str, c: &Catalog) -> Option<Target> {
    let lower = text.to_ascii_lowercase();
    let mut ships = c.ships.to_vec();
    ships.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));
    let ship = ships
        .iter()
        .find(|(_, name)| {
            let name = name.to_lowercase();
            lower.match_indices(&name).any(|(i, _)| {
                let before = lower[..i].chars().last();
                let after = lower[i + name.len()..].chars().next();
                !before.is_some_and(|c| c.is_alphanumeric())
                    && !after.is_some_and(|c| c.is_alphanumeric())
            })
        })?
        .1
        .clone();
    let mut pilot = String::new();
    for line in text.lines() {
        let low = line.to_lowercase();
        if let Some(i) = low.find("pilot:").or_else(|| low.find("пилот:"))
            && let Some((_, name)) = line[i..].split_once(':')
        {
            pilot = name.trim().to_string();
            break;
        }
    }
    if pilot.is_empty() {
        let mut preceding = "";
        for line in text.lines() {
            if let Some(i) = line.to_ascii_lowercase().find(&ship.to_ascii_lowercase()) {
                let before = line[..i].trim_matches([' ', '|', '\t']);
                let candidate = if before.is_empty() { preceding } else { before };
                let mut words: Vec<_> = candidate.split_whitespace().collect();
                if words.len() >= 2
                    && words[0].replace(',', ".").parse::<f64>().is_ok()
                    && ["km", "км", "m", "м"].contains(&words[1])
                {
                    words.drain(..2);
                }
                let candidate = words
                    .join(" ")
                    .trim_matches([' ', '|', '(', ')'])
                    .to_string();
                if candidate.chars().any(char::is_alphabetic)
                    && !candidate.contains(':')
                    && !["Selected Item", "Overview", "Выбранный объект", "Обзор"]
                        .contains(&candidate.as_str())
                {
                    pilot = candidate;
                }
                break;
            }
            if !line.trim().is_empty() {
                preceding = line.trim();
            }
        }
    }
    let tokens: Vec<_> = text.split_whitespace().collect();
    let mut distance = None;
    let mut angular = 0.;
    for i in 1..tokens.len() {
        let unit = tokens[i].to_lowercase();
        let value = tokens[i - 1].replace(',', ".").parse::<f64>().ok();
        if ["km", "км", "m", "м"].contains(&unit.as_str()) && distance.is_none() {
            distance = distance_value(&tokens, i, unit == "m" || unit == "м").map(|v| {
                v * if unit == "km" || unit == "км" {
                    1000.
                } else {
                    1.
                }
            });
        }
        if unit == "rad/s" || unit == "рад/с" {
            angular = value.unwrap_or(0.);
        }
    }
    Some(Target {
        ship,
        pilot,
        distance,
        angular,
    })
}
// EVE выводит метры целыми: «9,655 m» и «9 655 м» означают 9655, а не 9,655.
// Десятичная запятая в километрах при этом сохраняется: «12,5 км».
fn distance_value(tokens: &[&str], unit_index: usize, meters: bool) -> Option<f64> {
    let mut number = tokens[unit_index - 1].to_string();
    if meters {
        number = number.replace([',', '.'], "");
        let mut index = unit_index - 1;
        while index > 0
            && number.len().is_multiple_of(3)
            && tokens[index - 1]
                .chars()
                .all(|character| character.is_ascii_digit())
        {
            index -= 1;
            number.insert_str(0, tokens[index]);
        }
    } else {
        number = number.replace(',', ".");
    }
    number
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.)
}

#[derive(Default)]
pub struct Gate {
    current: Option<Target>,
    candidate: Option<(String, String)>,
    count: usize,
}
impl Gate {
    // Старую цель снимаем сразу, новую подтверждаем двумя одинаковыми наблюдениями.
    pub fn feed(&mut self, target: Option<Target>) -> Option<Option<Target>> {
        let Some(target) = target else {
            let changed = self.current.take().is_some();
            self.candidate = None;
            self.count = 0;
            return if changed { Some(None) } else { None };
        };
        if self
            .current
            .as_ref()
            .is_some_and(|t| t.identity() == target.identity())
        {
            let changed = self.current.as_ref() != Some(&target);
            self.current = Some(target.clone());
            return if changed { Some(Some(target)) } else { None };
        }
        let cleared = self.current.take().is_some();
        let key = (target.ship.clone(), target.pilot.clone());
        if self.candidate.as_ref() == Some(&key) {
            self.count += 1
        } else {
            self.candidate = Some(key);
            self.count = 1;
        }
        if self.count >= 2 {
            self.current = Some(target.clone());
            Some(Some(target))
        } else if cleared {
            Some(None)
        } else {
            None
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum FrameDecision {
    Recognize,
    Confirm,
    Ignore,
}
#[derive(Default)]
pub struct FrameCache {
    hash: Option<Vec<u8>>,
    confirmed: bool,
}
impl FrameCache {
    pub fn inspect(&mut self, pixels: &[u8]) -> FrameDecision {
        use sha2::Digest;
        let hash = sha2::Sha256::digest(pixels).to_vec();
        if self.hash.as_ref() != Some(&hash) {
            self.hash = Some(hash);
            self.confirmed = false;
            FrameDecision::Recognize
        } else if !self.confirmed {
            self.confirmed = true;
            FrameDecision::Confirm
        } else {
            FrameDecision::Ignore
        }
    }
}
