//! Разбор видимого текста цели и подтверждение смены корабля по двум кадрам.

use crate::engine::Catalog;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct TextLine {
    pub text: String,
    pub words: Vec<Word>,
}
#[derive(Clone, Default, Debug, serde::Serialize)]
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
fn empty_selection(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "объект не выбран",
        "object not selected",
        "no object selected",
        "06bekt he bb16pah",
        "o6bekt he bb16pah",
    ]
    .iter()
    .any(|message| text.contains(message))
}
pub fn selected_pilot(text: &str) -> Option<String> {
    if empty_selection(text) {
        return None;
    }
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
#[cfg(test)]
pub fn selected_card(selected: &str, cards: &Document, catalog: &Catalog) -> Option<Target> {
    identify_card(selected, cards, catalog).ok()
}

pub fn identify_card(
    selected: &str,
    cards: &Document,
    catalog: &Catalog,
) -> Result<Target, String> {
    if empty_selection(selected) {
        return Err("В EVE не выбран объект. Выберите пилота в правой панели".into());
    }
    let pilot = selected_pilot(selected).ok_or("В правой области не найден ник. Выделите строку ника вместе с расстоянием, без заголовков других окон")?;
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
        return Err(if anchors.is_empty() {
            format!("Ник «{pilot}» не найден на карточках слева. Проверьте обе области и текст OCR")
        } else {
            format!("Ник «{pilot}» найден в нескольких местах. Сузьте область карточек")
        });
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
                if !text.trim_end().ends_with(['*', '★', '∗', '✱']) {
                    continue;
                }
                let Some(tid) = starred_hull(&text, catalog) else {
                    continue;
                };
                let x = (words[0].x + words[end - start - 1].x + words[end - start - 1].width) / 2.;
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
        return Err(if matches.is_empty() {
            format!(
                "У «{pilot}» не найден корпус со звёздочкой. Включите в левую область всю строку корпуса и небольшой отступ снизу"
            )
        } else {
            format!("У «{pilot}» подходят несколько корпусов. Проверьте границы карточек")
        });
    };
    let ship = catalog.name(*tid).to_string();
    // Дистанция в списке карточек может принадлежать соседу; читаем её справа.
    let mut target = parse(&format!("{selected}\n{ship}"), catalog)
        .ok_or("Не удалось разобрать выбранную цель")?;
    target.pilot = pilot;
    target.ship = ship;
    Ok(target)
}
// Полный корпус всё равно должен быть отмечен звёздочкой. Исправляем только
// одну потерянную/перепутанную букву, если в справочнике есть единственный ответ.
fn starred_hull(text: &str, catalog: &Catalog) -> Option<i32> {
    let name = text
        .trim_end()
        .trim_end_matches(['*', '★', '∗', '✱'])
        .trim();
    if let Some(id) = catalog.ship_id(name) {
        return Some(id);
    }
    let normalized = name.to_lowercase().replace('0', "o");
    if let Some(id) = catalog.ship_id(&normalized) {
        return Some(id);
    }
    if normalized.chars().count() < 5 {
        return None;
    }
    let mut candidates = catalog
        .ships
        .iter()
        .filter(|(_, ship)| one_edit(&normalized, &ship.to_lowercase()));
    let (id, _) = candidates.next()?;
    if candidates.next().is_some() {
        return None;
    }
    Some(*id)
}
fn one_edit(a: &str, b: &str) -> bool {
    let (a, b): (Vec<_>, Vec<_>) = (a.chars().collect(), b.chars().collect());
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let (mut i, mut j, mut errors) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        errors += 1;
        if errors > 1 {
            return false;
        }
        if a.len() >= b.len() {
            i += 1;
        }
        if b.len() >= a.len() {
            j += 1;
        }
    }
    errors + usize::from(i < a.len() || j < b.len()) <= 1
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

/// Предпросмотр живёт в памяти; отдельные копии сохраняет включённый DebugLog.
#[derive(Default)]
pub struct RegionCheck {
    pub rect: Option<[i32; 4]>,
    pub image: Option<image::RgbaImage>,
    pub text: String,
    pub document: Option<Document>,
    pub retries: Vec<RetryRead>,
    pub error: Option<String>,
}
impl RegionCheck {
    fn capture(rect: Option<[i32; 4]>) -> Self {
        let mut result = Self {
            rect,
            ..Default::default()
        };
        if let Some(rect) = rect {
            match crate::platform::read_screen(rect) {
                Ok(image) => result.image = Some(image),
                Err(error) => result.error = Some(error),
            }
        }
        result
    }
    fn recognize(&mut self, language: &str) -> Option<Document> {
        let image = self.image.as_ref()?;
        match crate::platform::ocr_document(image, language) {
            Ok(document) => {
                self.text = document.text();
                self.document = Some(document.clone());
                Some(document)
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }
}
pub struct ScreenCheck {
    pub selected: RegionCheck,
    pub cards: RegionCheck,
    pub result: Result<Target, String>,
}
impl ScreenCheck {
    pub fn capture(settings: &crate::model::Settings) -> Self {
        Self {
            selected: RegionCheck::capture(settings.region),
            cards: RegionCheck::capture(settings.targets_region),
            result: Err("Ожидание OCR".into()),
        }
    }
    pub fn recognize(&mut self, language: &str, catalog: &Catalog) {
        let selected = self.selected.recognize(language);
        let cards = self.cards.recognize(language);
        self.result = if self.selected.rect.is_none() {
            Err("Сначала выделите правую область текущего выбора".into())
        } else if let Some(error) = self.selected.error.as_ref().or(self.cards.error.as_ref()) {
            Err(error.clone())
        } else if let (Some(selected), Some(cards)) = (&selected, &cards) {
            identify_capture(
                self.selected.image.as_ref().unwrap(),
                selected,
                cards,
                language,
                catalog,
                &mut self.selected.retries,
            )
        } else if let Some(selected) = selected {
            parse(&selected.text(), catalog)
                .ok_or("В правой области не найден корпус; выделите также карточки слева".into())
        } else {
            Err("Не удалось прочитать выбранную область".into())
        };
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct RetryRead {
    pub method: String,
    pub rect: [u32; 4],
    pub text: String,
    pub error: Option<String>,
}
fn normalized_pilot(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn nickname_distance(a: &str, b: &str) -> usize {
    let b: Vec<_> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = row[j + 1];
            row[j + 1] = (row[j] + 1)
                .min(next + 1)
                .min(previous + usize::from(ca != *cb));
            previous = next;
        }
    }
    row[b.len()]
}
fn card_pilot_names(cards: &Document) -> Vec<String> {
    let mut names = Vec::new();
    for line in &cards.lines {
        for start in 0..line.words.len() {
            for end in start + 1..=line.words.len().min(start + 8) {
                let words = &line.words[start..end];
                let height = words.iter().map(|w| w.height).fold(0_f32, f32::max);
                if height <= 0. || words.iter().any(|w| !w.x.is_finite() || !w.y.is_finite()) {
                    continue;
                }
                let left = words[0].x;
                let right = words.iter().map(|w| w.x + w.width).fold(left, f32::max);
                if start > 0
                    && left - (line.words[start - 1].x + line.words[start - 1].width) < height * 1.5
                {
                    continue;
                }
                if end < line.words.len() && line.words[end].x - right < height * 1.5 {
                    continue;
                }
                let text = words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if selected_pilot(&text).as_deref() == Some(text.as_str()) {
                    names.push(text);
                }
            }
        }
    }
    names.sort_by_key(|name| normalized_pilot(name));
    names.dedup_by(|a, b| normalized_pilot(a) == normalized_pilot(b));
    names
}
fn closest_pilot(selected: &str, names: Vec<String>) -> Result<Option<String>, String> {
    let selected = normalized_pilot(selected);
    let length = selected.chars().filter(|c| !c.is_whitespace()).count();
    let budget = match length {
        0..=2 => 0,
        3..=9 => 1,
        10..=17 => 2,
        _ => 3,
    };
    let mut scores: Vec<_> = names
        .into_iter()
        .map(|name| (nickname_distance(&selected, &normalized_pilot(&name)), name))
        .filter(|(distance, _)| *distance <= budget)
        .collect();
    scores.sort();
    let Some((distance, _)) = scores.first() else {
        return Ok(None);
    };
    if scores.get(1).is_some_and(|(next, _)| next == distance) {
        return Err(
            "Несколько карточек одинаково похожи на выбранный ник. Уточните область справа".into(),
        );
    }
    Ok(Some(scores.remove(0).1))
}
/// Сначала точное совпадение и повторное чтение, затем уникальный ближайший
/// ник с ограничением ошибок по длине. Корпус и его положение проверяются всегда.
pub fn identify_with_retries(
    selected: &str,
    retries: &[RetryRead],
    cards: &Document,
    catalog: &Catalog,
) -> Result<Target, String> {
    let original = match identify_card(selected, cards, catalog) {
        Ok(target) => return Ok(target),
        Err(error) => error,
    };
    let Some(pilot) = selected_pilot(selected) else {
        return Err(original);
    };
    let mut targets = Vec::new();
    for retry in retries {
        if retry.error.is_some() {
            continue;
        }
        if let Some(name) = selected_pilot(&retry.text)
            && let Ok(target) = identify_card(&format!("{name}\n{selected}"), cards, catalog)
        {
            targets.push(target);
        }
    }
    targets.sort_by(|a, b| a.identity().cmp(&b.identity()));
    targets.dedup_by(|a, b| a.identity() == b.identity());
    if targets.len() == 1 && closest_pilot(&pilot, vec![targets[0].pilot.clone()])?.is_some() {
        return Ok(targets.remove(0));
    }
    if let Some(name) = closest_pilot(&pilot, card_pilot_names(cards))? {
        return identify_card(&format!("{name}\n{selected}"), cards, catalog);
    }
    Err(original)
}
#[derive(serde::Serialize)]
pub struct PilotMatch {
    pub method: &'static str,
    pub edit_distance: usize,
}
pub fn pilot_match(selected: &str, retries: &[RetryRead], target: &Target) -> PilotMatch {
    let original = selected_pilot(selected).unwrap_or_default();
    let expected = normalized_pilot(&target.pilot);
    let distance = nickname_distance(&normalized_pilot(&original), &expected);
    let method = if distance == 0 {
        "exact"
    } else if retries.iter().any(|r| {
        r.error.is_none()
            && selected_pilot(&r.text).is_some_and(|p| normalized_pilot(&p) == expected)
    }) {
        "retry_exact"
    } else {
        "closest"
    };
    PilotMatch {
        method,
        edit_distance: distance,
    }
}
pub fn identify_capture(
    image: &image::RgbaImage,
    selected: &Document,
    cards: &Document,
    language: &str,
    catalog: &Catalog,
    retries: &mut Vec<RetryRead>,
) -> Result<Target, String> {
    let text = selected.text();
    let result = identify_card(&text, cards, catalog);
    if result.is_ok() || selected_pilot(&text).is_none() {
        return result;
    }
    if retries.is_empty() {
        *retries = crate::platform::selected_retry_texts(image, selected, language);
    }
    identify_with_retries(&text, retries, cards, catalog)
}
