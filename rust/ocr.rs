//! Разбор видимого текста цели и подтверждение смены корабля по двум кадрам.

use crate::engine::Catalog;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Word {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TextLine {
    pub text: String,
    pub words: Vec<Word>,
}
#[derive(Clone, Default, Debug, serde::Serialize, serde::Deserialize)]
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
fn clean_pilot_name(text: &str) -> String {
    let chars: Vec<_> = text.chars().collect();
    chars
        .iter()
        .enumerate()
        .map(|(i, c)| match c {
            '’' | '‘' | 'ʼ' | '`' | '´' => '\'',
            '•' | '·'
                if i > 0
                    && i + 1 < chars.len()
                    && chars[i - 1].is_alphabetic()
                    && chars[i + 1].is_alphabetic() =>
            {
                '\''
            }
            _ => *c,
        })
        .collect()
}
fn status_line(text: &str) -> bool {
    let visual: String = text
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'с' => 'c',
            'т' => 't',
            'а' => 'a',
            'у' => 'y',
            'р' => 'p',
            'н' => 'h',
            'о' | '0' => 'o',
            'е' => 'e',
            'м' => 'm',
            'и' => 'u',
            'п' => 'n',
            'б' | '6' => 'b',
            'з' => 'z',
            _ => c,
        })
        .collect();
    visual.replace(' ', "").contains("securitystatus")
        || visual.contains("статус безопасности")
        || (visual.contains("onac")
            && visual.split_whitespace().next().is_some_and(|first| {
                ["ctatyc", "ctatus", "status"].iter().any(|prefix| {
                    nickname_distance(&first.chars().take(6).collect::<String>(), prefix) <= 2
                })
            }))
}
/// Отделяем ник от произвольного звания в меньшем шрифте/бейдже справа.
/// Остальные текстовые строки не могут подменить первую строку имени.
pub fn selected_document(raw: &Document) -> Document {
    if empty_selection(&raw.text()) {
        return raw.clone();
    }
    let mut ordered: Vec<_> = raw.lines.iter().collect();
    ordered.sort_by(|a, b| {
        let top = |line: &TextLine| line.words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
        top(a).total_cmp(&top(b))
    });
    let mut header = None;
    for line in &ordered {
        if status_line(&line.text)
            || read_distance(&line.text).is_some()
            || line.text.contains(['(', ')'])
        {
            continue;
        }
        let mut words = line.words.clone();
        words.sort_by(|a, b| a.x.total_cmp(&b.x));
        // Speech/ship icons may be emitted as separate OCR words before the name.
        // They are not a nickname and must not turn the real name into a rank suffix.
        let first_name = words
            .iter()
            .position(|word| word.text.chars().any(char::is_alphanumeric))
            .unwrap_or(words.len());
        words.drain(..first_name);
        let height = words.iter().map(|w| w.height).fold(0_f32, f32::max);
        for index in 1..words.len() {
            let gap = words[index].x - (words[index - 1].x + words[index - 1].width);
            if gap >= height * 0.85 || words[index].height < height * 0.82 && gap >= height * 0.6 {
                words.truncate(index);
                break;
            }
        }
        let text = clean_pilot_name(&if words.is_empty() {
            line.text.clone()
        } else {
            words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        });
        let text = text
            .trim_end()
            .trim_end_matches(['*', '★', '∗', '✱', '•', '·', '●'])
            .trim();
        if let Some(pilot) = selected_pilot(text) {
            header = Some(TextLine { text: pilot, words });
            break;
        }
    }
    let Some(header) = header else {
        return Document::default();
    };
    let top = header.words.iter().map(|w| w.y).fold(0_f32, f32::max);
    let height = header.words.iter().map(|w| w.height).fold(0_f32, f32::max);
    let mut lines = vec![header];
    for line in ordered {
        if status_line(&line.text) {
            continue;
        }
        let y = line.words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
        if height > 0. && (y < top || y > top + height * 4.5) {
            continue;
        }
        if read_distance(&line.text).is_some()
            || line.text.contains("rad/s")
            || line.text.contains("рад/с")
        {
            lines.push(line.clone());
        }
    }
    Document { lines }
}
pub fn selected_text(raw: &Document, retries: &[RetryRead]) -> String {
    let mut text = selected_document(raw).text();
    if let Some(original) = selected_pilot(&text).filter(|p| p.chars().count() <= 6) {
        let mut votes = std::collections::BTreeMap::<String, (String, usize)>::new();
        for name in retries
            .iter()
            .filter(|r| r.method.starts_with("nickname_") && r.error.is_none())
            .filter_map(|r| selected_pilot(&r.text))
        {
            if !clearly_different_pilot(&original, &name) {
                let entry = votes.entry(normalized_pilot(&name)).or_insert((name, 0));
                entry.1 += 1;
            }
        }
        let agreed: Vec<_> = votes.values().filter(|(_, count)| *count >= 2).collect();
        if let [name] = agreed.as_slice() {
            text = text.replacen(&original, &name.0, 1);
        }
    }
    if read_distance(&text).is_none() {
        let mut distances: Vec<_> = retries
            .iter()
            .filter(|r| r.method.starts_with("distance_") && r.error.is_none())
            .filter_map(|r| read_distance(&r.text))
            .collect();
        distances.sort_by(f64::total_cmp);
        distances.dedup_by(|a, b| (*a - *b).abs() < 0.1);
        if let [distance] = distances.as_slice() {
            text.push_str(&format!("\n{distance} m"));
        }
    }
    text
}
pub fn selected_pilot(text: &str) -> Option<String> {
    if empty_selection(text) {
        return None;
    }
    for line in text.lines() {
        if status_line(line) {
            continue;
        }
        let cleaned = clean_pilot_name(line);
        let candidate = cleaned
            .trim()
            .trim_start_matches("Pilot:")
            .trim_start_matches("Пилот:")
            .trim_start_matches(['•', '·', '●', '▪', '■'])
            .trim();
        let lower = candidate.to_lowercase();
        if candidate.is_empty()
            || candidate.len() > 100
            || candidate.chars().filter(|c| c.is_alphanumeric()).count() < 3
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
/// Ищем ник и корпус ниже в той же колонке; при обрезанном имени нужен уникальный префикс каталога.
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
    let names = card_pilot_names(cards, catalog);
    if names
        .iter()
        .any(|name| normalized_pilot(name).starts_with(&format!("{} ", normalized_pilot(&pilot))))
    {
        return Err(format!(
            "Ник «{pilot}» прочитан не полностью: имя на карточке перенесено"
        ));
    }
    let anchors = nickname_anchors(&pilot, cards);
    if anchors.is_empty() {
        return Err(format!(
            "Ник «{pilot}» не найден на карточках слева. Проверьте обе области и текст OCR"
        ));
    }
    // A matching name floating in space is not a locked-target card. Require
    // its own hull below it before choosing between nickname anchors.
    let viable: Vec<_> = anchors
        .into_iter()
        .map(|(x, y, h)| hulls_at_anchor(cards, catalog, x, y, h))
        .filter(|hulls| !hulls.is_empty())
        .collect();
    if viable.len() > 1 {
        return Err(format!(
            "Ник «{pilot}» найден в нескольких местах на карточках. Сузьте область карточек"
        ));
    }
    let matches = viable.into_iter().next().unwrap_or_default();
    let [tid] = matches.as_slice() else {
        return Err(if matches.is_empty() {
            format!(
                "У «{pilot}» не удалось однозначно прочитать корпус. Включите в левую область строку корпуса целиком; звёздочка необязательна"
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
fn nickname_anchors(pilot: &str, cards: &Document) -> Vec<(f32, f32, f32)> {
    let mut anchors = single_line_nickname_anchors(pilot, cards);
    let words: Vec<_> = pilot.split_whitespace().collect();
    // Narrow EVE cards can wrap a complete nickname onto two lines. Match both
    // exact parts in one column; never concatenate names of adjacent cards.
    for split in 1..words.len() {
        let first = single_line_nickname_anchors(&words[..split].join(" "), cards);
        let second = single_line_nickname_anchors(&words[split..].join(" "), cards);
        for (x, y, h) in &first {
            for (nx, ny, nh) in &second {
                let height = h.max(*nh);
                if ny - y >= height * 0.7
                    && ny - y <= height * 1.8
                    && (nx - x).abs() <= height * 2.5
                {
                    anchors.push(((x + nx) / 2., *y, height));
                }
            }
        }
    }
    anchors
}
fn single_line_nickname_anchors(pilot: &str, cards: &Document) -> Vec<(f32, f32, f32)> {
    let normalized = normalized_pilot(pilot);
    let mut anchors = Vec::new();
    for line in &cards.lines {
        for start in 0..line.words.len() {
            for end in start + 1..=line.words.len().min(start + 8) {
                let words = &line.words[start..end];
                if words
                    .iter()
                    .skip(1)
                    .any(|w| w.text.starts_with('[') || w.text.ends_with(']'))
                {
                    continue;
                }
                let candidate = words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if normalized_pilot(&card_header_text(
                    candidate.split('[').next().unwrap_or(&candidate),
                )) == normalized
                {
                    let left = words[0].x;
                    let mut right = words.iter().map(|w| w.x + w.width).fold(left, f32::max);
                    let top = words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
                    let mut height = words.iter().map(|w| w.height).fold(0., f32::max);
                    if start > 0
                        && left - (line.words[start - 1].x + line.words[start - 1].width)
                            < height * 1.5
                    {
                        continue;
                    }
                    if let Some(next) = line.words.get(end) {
                        if next.text.starts_with('[') || next.text.ends_with(']') {
                            right = right.max(next.x + next.width);
                            height = height.max(next.height);
                        } else if next.x - right < height * 1.5 {
                            continue;
                        }
                    }
                    anchors.push(((left + right) / 2., top, height));
                }
            }
        }
    }
    anchors
}
fn hulls_at_anchor(
    cards: &Document,
    catalog: &Catalog,
    anchor_x: f32,
    anchor_y: f32,
    height: f32,
) -> Vec<i32> {
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
                if !text
                    .trim_end()
                    .ends_with(['*', '★', '∗', '✱', '•', '·', '●'])
                {
                    continue;
                }
                let Some(tid) = starred_hull(&text, catalog) else {
                    continue;
                };
                let x = (words[0].x + words[end - start - 1].x + words[end - start - 1].width) / 2.;
                let y = words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
                // Отступы считаются от размера шрифта, а не от конкретного разрешения EVE.
                if y > anchor_y
                    && y < anchor_y + height * 6.
                    && (x - anchor_x).abs() <= height * 2.5
                {
                    matches.push(tid);
                }
            }
        }
    }
    if matches.is_empty() {
        matches = clipped_card_hulls(cards, catalog, anchor_x, anchor_y, height);
    }
    matches.sort();
    matches.dedup();
    matches
}
/// Missing stars and clipped/wrapped names are accepted only in the selected
/// pilot's column, before that card's distance row. A damaged prefix must also
/// have one closest catalog match; an ambiguous exact prefix stays unresolved.
fn clipped_card_hulls(
    cards: &Document,
    catalog: &Catalog,
    x: f32,
    y: f32,
    height: f32,
) -> Vec<i32> {
    let mut segments = Vec::new();
    for line in &cards.lines {
        let mut start = 0;
        while start < line.words.len() {
            let mut end = start + 1;
            while end < line.words.len()
                && line.words[end].x - (line.words[end - 1].x + line.words[end - 1].width)
                    <= height * 1.8
            {
                end += 1;
            }
            let words = &line.words[start..end];
            start = end;
            if words
                .iter()
                .any(|w| w.height <= 0. || !w.x.is_finite() || !w.y.is_finite())
            {
                continue;
            }
            let cx = (words[0].x + words.last().unwrap().x + words.last().unwrap().width) / 2.;
            let cy = words.iter().map(|w| w.y).fold(f32::INFINITY, f32::min);
            if cy > y && cy < y + height * 6. && (cx - x).abs() <= height * 2.5 {
                segments.push((
                    words
                        .iter()
                        .map(|w| w.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                    cx,
                    cy,
                ));
            }
        }
    }
    segments.sort_by(|a, b| a.2.total_cmp(&b.2));
    let bottom = segments
        .iter()
        .filter(|(text, _, _)| read_distance(text).is_some())
        .map(|s| s.2)
        .fold(y + height * 6., f32::min);
    let mut matches = Vec::new();
    for (i, (text, cx, cy)) in segments.iter().enumerate() {
        if *cy >= bottom || text.contains(['[', ']']) {
            continue;
        }
        if let Some(id) = unique_clipped_hull(text, catalog) {
            matches.push(id);
        }
        if let Some((next, nx, ny)) = segments.get(i + 1)
            && *ny < bottom
            && ny - cy <= height * 2.
            && (nx - cx).abs() <= height * 1.5
            && let Some(id) = unique_clipped_hull(&format!("{text} {next}"), catalog)
        {
            matches.push(id);
        }
    }
    matches
}
fn unique_clipped_hull(text: &str, catalog: &Catalog) -> Option<i32> {
    let normalized = text
        .trim()
        .trim_end_matches(['.', '…', '*', '★', '∗', '✱', '•', '·', '●', ';', ':', ','])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
        .replace('0', "o");
    if normalized.chars().count() < 4 {
        return None;
    }
    let mut candidates = catalog
        .ships
        .iter()
        .filter(|(_, name)| name.to_lowercase().starts_with(&normalized));
    let Some((id, name)) = candidates.next() else {
        return damaged_clipped_hull(&normalized, catalog);
    };
    if name.to_lowercase() != normalized
        && normalized.chars().count() < 8
        && !(normalized.chars().count() >= 5
            && name
                .chars()
                .count()
                .saturating_sub(normalized.chars().count())
                <= 2)
    {
        return None;
    }
    candidates.next().is_none().then_some(*id)
}
fn damaged_clipped_hull(text: &str, catalog: &Catalog) -> Option<i32> {
    let length = text.chars().count();
    // Small fragments and short names are too easily confused without a star.
    if length < 8 {
        return None;
    }
    let budget = if length <= 14 { 2 } else { 3 };
    let mut candidates: Vec<_> = catalog
        .ships
        .iter()
        .filter_map(|(id, ship)| {
            let name = ship.to_lowercase();
            let ship_length = name.chars().count();
            let score = if ship_length >= length {
                // The end of the label may be clipped by EVE's card width.
                nickname_distance(text, &name.chars().take(length).collect::<String>())
            } else if length - ship_length <= budget {
                nickname_distance(text, &name)
            } else {
                return None;
            };
            (score <= budget).then_some((score, *id))
        })
        .collect();
    candidates.sort();
    let (score, id) = *candidates.first()?;
    if candidates.get(1).is_some_and(|(next, _)| *next == score) {
        return None;
    }
    Some(id)
}
// Подбираем только строку корпуса с терминальным маркером звезды/его OCR-вариантом.
fn starred_hull(text: &str, catalog: &Catalog) -> Option<i32> {
    let name = text
        .trim_end()
        .trim_end_matches(['*', '★', '∗', '✱', '•', '·', '●'])
        .trim();
    if let Some(id) = catalog.ship_id(name) {
        return Some(id);
    }
    let normalized = name.to_lowercase().replace('0', "o");
    if let Some(id) = catalog.ship_id(&normalized) {
        return Some(id);
    }
    let length = normalized.chars().count();
    let budget = match length {
        0..=3 => 0,
        4..=6 => 1,
        7..=14 => 2,
        _ => 3,
    };
    let mut candidates: Vec<_> = catalog
        .ships
        .iter()
        .filter(|(_, ship)| ship.chars().count().abs_diff(length) <= budget)
        .map(|(id, ship)| (nickname_distance(&normalized, &ship.to_lowercase()), *id))
        .filter(|(distance, _)| *distance <= budget)
        .collect();
    candidates.sort();
    let (distance, id) = *candidates.first()?;
    if candidates.get(1).is_some_and(|(next, _)| *next == distance) {
        return None;
    }
    Some(id)
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
    let distance = read_distance(text);
    let mut angular = 0.;
    for i in 1..tokens.len() {
        let unit = tokens[i].to_lowercase();
        let value = tokens[i - 1].replace(',', ".").parse::<f64>().ok();
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
fn numeric_token(text: &str) -> Option<String> {
    let mut output = String::new();
    for c in text.trim_matches(['•', '·', '(', ')']).chars() {
        output.push(match c {
            '0'..='9' | ',' | '.' => c,
            'O' | 'o' | 'ø' | 'Ø' | 'О' | 'о' => '0',
            'l' | 'I' | 'i' | '|' | 'ı' => '1',
            'S' | 's' => '5',
            'B' | 'В' => '8',
            _ => return None,
        });
    }
    (!output.is_empty()).then_some(output)
}
pub fn read_distance(text: &str) -> Option<f64> {
    let mut tokens = Vec::new();
    for token in text.split_whitespace() {
        let lower = token.to_lowercase();
        let mut split = None;
        if token.chars().any(|c| c.is_ascii_digit()) {
            for unit in ["km", "км", "kм", "кm", "м", "m"] {
                if let Some(number) = lower.strip_suffix(unit)
                    && !number.is_empty()
                {
                    split = Some((number.to_string(), unit.to_string()));
                    break;
                }
            }
        }
        if let Some((number, unit)) = split {
            tokens.push(number);
            tokens.push(unit);
        } else {
            tokens.push(token.to_string());
        }
    }
    for index in 1..tokens.len() {
        let unit = tokens[index]
            .trim_matches([',', ';'])
            .to_lowercase()
            .replace('к', "k")
            .replace('м', "m");
        if !["km", "m"].contains(&unit.as_str()) {
            continue;
        }
        let Some(mut number) = numeric_token(&tokens[index - 1]) else {
            continue;
        };
        if unit == "m" {
            number = number.replace([',', '.'], "");
        } else {
            number = number.replace(',', ".");
        }
        if number.chars().all(|c| c.is_ascii_digit()) {
            let mut previous = index - 1;
            while previous > 0 && number.len().is_multiple_of(3) {
                let Some(group) = numeric_token(&tokens[previous - 1]) else {
                    break;
                };
                if !group.chars().all(|c| c.is_ascii_digit()) {
                    break;
                }
                previous -= 1;
                number.insert_str(0, &group);
            }
        }
        if let Ok(value) = number.parse::<f64>()
            && value.is_finite()
            && value >= 0.
        {
            return Some(value * if unit == "km" { 1000. } else { 1. });
        }
    }
    None
}

#[derive(Default)]
pub struct Gate {
    current: Option<Target>,
    candidate: Option<Target>,
    count: usize,
    missing_since: Option<std::time::Instant>,
    selection_candidate: Option<(String, usize)>,
}
impl Gate {
    /// A failed card read is a missing observation, not proof of a different target.
    pub fn observe_at(
        &mut self,
        target: Option<Target>,
        observed: Option<&str>,
        now: std::time::Instant,
        hold_seconds: u32,
    ) -> Option<Option<Target>> {
        if let Some(mut target) = target {
            if let Some(current) = &self.current
                && current.ship == target.ship
                && observed.is_some_and(|pilot| {
                    normalized_pilot(pilot) == normalized_pilot(&current.pilot)
                })
                && normalized_pilot(&current.pilot).starts_with(&normalized_pilot(&target.pilot))
            {
                target.pilot = current.pilot.clone();
            }
            if let Some(current) = self.current.clone()
                && current.ship == target.ship
                && current.identity() != target.identity()
                && !clearly_different_pilot(&current.pilot, &target.pilot)
                && observed.is_some_and(|p| !clearly_different_pilot(p, &current.pilot))
            {
                // A one-frame spelling change is weak evidence after acquisition.
                // Two consistent complete reads replace the identity atomically.
                let name = format!("near:{}", normalized_pilot(&target.pilot));
                let count = match &self.selection_candidate {
                    Some((previous, count)) if previous == &name => count + 1,
                    _ => 1,
                };
                self.selection_candidate = Some((name, count));
                if count < 2 {
                    target.pilot = current.pilot;
                    return self.feed_at(Some(target), now, hold_seconds);
                }
                self.current = Some(target.clone());
                self.candidate = Some(target.clone());
                self.count = 2;
                self.missing_since = None;
                self.selection_candidate = None;
                return Some(Some(target));
            }
            self.selection_candidate = None;
            return self.feed_at(Some(target), now, hold_seconds);
        }
        if hold_seconds > 0
            && self.current.is_none()
            && let Some(candidate) = self.candidate.clone()
            && observed
                .is_some_and(|pilot| normalized_pilot(pilot) == normalized_pilot(&candidate.pilot))
        {
            // One complete card plus the same Selected Item on the next frame confirms
            // the last known target even if the ship has already warped away.
            let change = self.feed_at(Some(candidate), now, hold_seconds);
            if self.current.is_some() {
                self.missing_since = Some(now);
            }
            return change;
        }
        let changed = observed
            .zip(self.current.as_ref())
            .filter(|(pilot, current)| clearly_different_pilot(pilot, &current.pilot));
        if let Some((pilot, _)) = changed {
            let name = normalized_pilot(pilot);
            let count = match &self.selection_candidate {
                Some((previous, count)) if previous == &name => count + 1,
                _ => 1,
            };
            self.selection_candidate = Some((name, count));
            if count >= 2 {
                self.current = None;
                self.candidate = None;
                self.count = 0;
                self.missing_since = None;
                self.selection_candidate = None;
                return Some(None);
            }
        } else {
            self.selection_candidate = None;
        }
        self.feed_at(None, now, hold_seconds)
    }
    pub fn diagnostic(&self, now: std::time::Instant, hold_seconds: u32) -> serde_json::Value {
        serde_json::json!({"current":self.current,"candidate":self.candidate,"confirmations":self.count,"selection_candidate":self.selection_candidate,"hold_seconds":hold_seconds,"remaining_ms":self.remaining(now,hold_seconds).map(|d|d.as_millis())})
    }
    // Старую цель снимаем сразу, новую подтверждаем двумя одинаковыми наблюдениями.
    #[cfg(test)]
    pub fn feed(&mut self, target: Option<Target>) -> Option<Option<Target>> {
        self.feed_at(target, std::time::Instant::now(), 0)
    }
    pub fn remaining(
        &self,
        now: std::time::Instant,
        hold_seconds: u32,
    ) -> Option<std::time::Duration> {
        self.missing_since.map(|since| {
            std::time::Duration::from_secs(hold_seconds as u64)
                .saturating_sub(now.saturating_duration_since(since))
        })
    }
    pub fn expire(&mut self, now: std::time::Instant, hold_seconds: u32) -> bool {
        if self
            .remaining(now, hold_seconds)
            .is_some_and(|remaining| remaining.is_zero())
        {
            self.missing_since = None;
            self.candidate = None;
            self.count = 0;
            self.selection_candidate = None;
            return self.current.take().is_some();
        }
        false
    }
    pub fn feed_at(
        &mut self,
        target: Option<Target>,
        now: std::time::Instant,
        hold_seconds: u32,
    ) -> Option<Option<Target>> {
        let Some(target) = target else {
            if self.current.is_some() && hold_seconds > 0 {
                self.missing_since.get_or_insert(now);
                return self.expire(now, hold_seconds).then_some(None);
            }
            let changed = self.current.take().is_some();
            self.candidate = None;
            self.count = 0;
            self.missing_since = None;
            self.selection_candidate = None;
            return if changed { Some(None) } else { None };
        };
        self.missing_since = None;
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
        if self
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.identity() == target.identity())
        {
            self.count += 1
        } else {
            self.count = 1;
        }
        self.candidate = Some(target.clone());
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

/// Context is acquired from two complete reads. Weak recovery never renews it.
#[derive(Default)]
pub struct Continuity {
    known: Option<(Target, std::time::Instant)>,
    pending: Option<(Target, usize)>,
}
impl Continuity {
    pub fn learn(&mut self, target: &Target, now: std::time::Instant) {
        if self
            .known
            .as_ref()
            .is_some_and(|(known, _)| known.identity() == target.identity())
        {
            self.known = Some((target.clone(), now));
            self.pending = None;
            return;
        }
        let count = match &self.pending {
            Some((candidate, count)) if candidate.identity() == target.identity() => count + 1,
            _ => 1,
        };
        self.pending = Some((target.clone(), count));
        if count >= 2 {
            self.known = Some((target.clone(), now));
            self.pending = None;
        }
    }
    pub fn touch(&mut self, now: std::time::Instant) {
        if let Some((_, last)) = &mut self.known {
            *last = now;
        }
    }
    pub fn observe_selection(&mut self, text: &str) {
        if empty_selection(text)
            || selected_pilot(text).is_some_and(|p| {
                self.known
                    .as_ref()
                    .is_some_and(|(known, _)| clearly_different_pilot(&p, &known.pilot))
            })
        {
            self.known = None;
            self.pending = None;
        }
    }
    pub fn recover(
        &self,
        selected: &str,
        cards: &Document,
        catalog: &Catalog,
        now: std::time::Instant,
        grace: std::time::Duration,
    ) -> Option<(Target, &'static str)> {
        let (known, last) = self.known.as_ref()?;
        if now.saturating_duration_since(*last) > grace || empty_selection(selected) {
            return None;
        }
        let pilot = selected_pilot(selected)?;
        let exact = normalized_pilot(&pilot) == normalized_pilot(&known.pilot);
        let card = identify_card(&known.pilot, cards, catalog);
        if card.as_ref().is_ok_and(|c| c.ship != known.ship) {
            return None;
        }
        if card
            .as_ref()
            .err()
            .is_some_and(|e| e.to_lowercase().contains("несколь"))
        {
            return None;
        }
        let supported_card = card.as_ref().is_ok_and(|c| c.ship == known.ship);
        if !exact && (!supported_card || clearly_different_pilot(&pilot, &known.pilot)) {
            return None;
        }
        let mut recovered = known.clone();
        recovered.distance = read_distance(selected).or(known.distance);
        Some((
            recovered,
            if exact {
                "confirmed_selected"
            } else {
                "confirmed_card"
            },
        ))
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
    pub parsed_text: String,
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
        if let Some(document) = &selected {
            self.selected.parsed_text = selected_text(document, &self.selected.retries);
        }
        self.result = if self.selected.rect.is_none() {
            Err("Сначала выделите правую область текущего выбора".into())
        } else if let Some(error) = self.selected.error.as_ref().or(self.cards.error.as_ref()) {
            Err(error.clone())
        } else if let (Some(selected), Some(cards)) = (&selected, &cards) {
            let result = identify_images(
                self.selected.image.as_ref().unwrap(),
                selected,
                self.cards.image.as_ref().unwrap(),
                cards,
                language,
                catalog,
                &mut self.selected.retries,
            );
            self.selected.parsed_text = selected_text(selected, &self.selected.retries);
            result
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle_degrees: Option<f64>,
    pub method: String,
    pub rect: [u32; 4],
    pub text: String,
    pub error: Option<String>,
}
pub fn clearly_different_pilot(a: &str, b: &str) -> bool {
    let a = normalized_pilot(a);
    let b = normalized_pilot(b);
    let length = a.chars().count().max(b.chars().count());
    nickname_distance(&a, &b)
        > match length {
            0..=2 => 0,
            3..=9 => 1,
            10..=17 => 2,
            _ => 3,
        }
}
fn normalized_pilot(text: &str) -> String {
    clean_pilot_name(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn card_header_text(text: &str) -> String {
    // A tiny bright object between words is often emitted as a bullet. Keep
    // genuine apostrophes; only OCR's non-name bullet glyphs become spacing.
    let text = text.split('[').next().unwrap_or(text);
    let mut words: Vec<_> = text.split_whitespace().collect();
    if words.last().is_some_and(|word| word.ends_with(']')) {
        words.pop();
    }
    words
        .join(" ")
        .trim_end_matches(',')
        .replace(['•', '·', '●'], " ")
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
fn card_pilot_fragments(cards: &Document) -> Vec<String> {
    let mut names = Vec::new();
    for line in &cards.lines {
        for start in 0..line.words.len() {
            for end in start + 1..=line.words.len().min(start + 8) {
                let words = &line.words[start..end];
                let height = words.iter().map(|w| w.height).fold(0_f32, f32::max);
                if height <= 0. || words.iter().any(|w| !w.x.is_finite() || !w.y.is_finite()) {
                    continue;
                }
                if words
                    .windows(2)
                    .any(|pair| pair[1].x - (pair[0].x + pair[0].width) > height * 1.5)
                {
                    continue;
                }
                let left = words[0].x;
                let right = words.iter().map(|w| w.x + w.width).fold(left, f32::max);
                if start > 0
                    && left - (line.words[start - 1].x + line.words[start - 1].width) < height * 1.5
                {
                    continue;
                }
                if end < line.words.len()
                    && !(line.words[end].text.starts_with('[')
                        || line.words[end].text.ends_with(']'))
                    && line.words[end].x - right < height * 1.5
                {
                    continue;
                }
                let text = words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if let Some(pilot) = selected_pilot(&card_header_text(&text)) {
                    names.push(pilot);
                }
            }
        }
    }
    names.sort_by_key(|name| normalized_pilot(name));
    names.dedup_by(|a, b| normalized_pilot(a) == normalized_pilot(b));
    names
}
fn card_pilot_names(cards: &Document, catalog: &Catalog) -> Vec<String> {
    let fragments = card_pilot_fragments(cards);
    let mut wrapped = Vec::new();
    for first in &fragments {
        if catalog.ship_id(first).is_some() {
            continue;
        }
        for second in &fragments {
            if first == second || catalog.ship_id(second).is_some() {
                continue;
            }
            let first_anchors = single_line_nickname_anchors(first, cards);
            let second_anchors = single_line_nickname_anchors(second, cards);
            let paired = first_anchors.iter().any(|(x, y, h)| {
                second_anchors.iter().any(|(nx, ny, nh)| {
                    let height = h.max(*nh);
                    h.min(*nh) >= height * 0.77
                        && ny - y >= height * 0.7
                        && ny - y <= height * 1.8
                        && (nx - x).abs() <= height * 2.5
                        && !hulls_at_anchor(cards, catalog, *nx, *ny, *nh).is_empty()
                })
            });
            if paired {
                wrapped.push(format!("{first} {second}"));
            }
        }
    }
    let mut names: Vec<_> = fragments
        .into_iter()
        .filter(|part| {
            !wrapped.iter().any(|full| {
                full.starts_with(&format!("{part} ")) || full.ends_with(&format!(" {part}"))
            })
        })
        .collect();
    names.extend(wrapped);
    names.sort_by_key(|name| normalized_pilot(name));
    names.dedup_by(|a, b| normalized_pilot(a) == normalized_pilot(b));
    names
}
fn closest_pilot(selected: &str, mut names: Vec<String>) -> Result<Option<String>, String> {
    names.sort_by_key(|name| normalized_pilot(name));
    names.dedup_by(|a, b| normalized_pilot(a) == normalized_pilot(b));
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
        if retry.error.is_some() || !retry.method.starts_with("nickname_") {
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
    if let Some(name) = closest_pilot(
        &pilot,
        card_pilot_names(cards, catalog)
            .into_iter()
            .filter(|name| identify_card(name, cards, catalog).is_ok())
            .collect(),
    )? {
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
    let method = if retries.iter().any(|r| r.method == "cards_match") {
        "cards_retry"
    } else if distance == 0 {
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
#[derive(Clone)]
struct CardCandidate {
    target: Target,
    anchor: (f32, f32, f32),
}
fn located_candidate(target: Target, cards: &Document, catalog: &Catalog) -> Option<CardCandidate> {
    let ship = catalog.ship_id(&target.ship)?;
    let anchors: Vec<_> = nickname_anchors(&target.pilot, cards)
        .into_iter()
        .filter(|(x, y, h)| hulls_at_anchor(cards, catalog, *x, *y, *h).contains(&ship))
        .collect();
    let [anchor] = anchors.as_slice() else {
        return None;
    };
    Some(CardCandidate {
        target,
        anchor: *anchor,
    })
}
fn same_card(a: &CardCandidate, b: &CardCandidate) -> bool {
    let h = a.anchor.2.max(b.anchor.2);
    (a.anchor.0 - b.anchor.0).abs() <= h * 1.5 && (a.anchor.1 - b.anchor.1).abs() <= h * 1.2
}
fn confirm_card_name(
    candidates: &mut [CardCandidate],
    selected: &str,
    retries: &[RetryRead],
) -> Option<String> {
    let pilot = selected_pilot(selected)?;
    let reads: Vec<_> = retries
        .iter()
        .filter(|r| r.method.starts_with("nickname_") && r.error.is_none())
        .filter_map(|r| selected_pilot(&r.text))
        .collect();
    let first = candidates.first()?;
    let confirmed = if pilot.chars().count() <= 6 {
        reads
            .iter()
            .filter(|p| normalized_pilot(p) == normalized_pilot(&pilot))
            .count()
            >= 2
            && reads.iter().all(|p| !clearly_different_pilot(p, &pilot))
    } else {
        reads.len() >= 2
            && reads
                .iter()
                .all(|p| normalized_pilot(p) == normalized_pilot(&pilot))
    };
    if !confirmed
        || !candidates
            .iter()
            .all(|c| same_card(first, c) && c.target.ship == first.target.ship)
    {
        return None;
    }
    let before = candidates
        .iter()
        .map(|c| c.target.pilot.clone())
        .collect::<Vec<_>>()
        .join(" / ");
    let changed = candidates.iter().any(|c| c.target.pilot != pilot);
    for candidate in candidates {
        candidate.target.pilot = pilot.clone();
    }
    changed.then(|| format!("{before} -> {pilot}"))
}
/// Retry the left region as well: transparent target cards inherit changing
/// space scenery. Prefer exact nickname recovery across all filtered readings.
pub fn identify_images(
    selected_image: &image::RgbaImage,
    selected: &Document,
    cards_image: &image::RgbaImage,
    cards: &Document,
    language: &str,
    catalog: &Catalog,
    retries: &mut Vec<RetryRead>,
) -> Result<Target, String> {
    retries.retain(|r| !r.method.starts_with("cards_"));
    let original = identify_capture(selected_image, selected, cards, language, catalog, retries);
    let text = selected_text(selected, retries);
    if original
        .as_ref()
        .err()
        .is_some_and(|e| e.contains("нескольких местах") || e.contains("несколько корпусов"))
    {
        return original;
    }
    if identify_card(&text, cards, catalog).is_ok() || selected_pilot(&text).is_none() {
        return original;
    }
    let readings = crate::platform::card_retry_documents(cards_image, language);
    let mut exact = Vec::new();
    let mut fuzzy = Vec::new();
    if let Ok(target) = &original
        && let Some(c) = located_candidate(target.clone(), cards, catalog)
    {
        fuzzy.push(c);
    }
    for (read, document) in readings {
        if read.error.is_none() {
            if let Ok(target) = identify_card(&text, &document, catalog) {
                if let Some(c) = located_candidate(target, &document, catalog) {
                    exact.push(c);
                }
            } else if let Ok(target) = identify_with_retries(&text, retries, &document, catalog)
                && let Some(c) = located_candidate(target, &document, catalog)
            {
                fuzzy.push(c);
            }
        }
        retries.push(read);
    }
    retries.push(RetryRead{angle_degrees:None,method:"cards_candidates".into(),rect:[0,0,cards_image.width(),cards_image.height()],text:serde_json::json!(fuzzy.iter().map(|c|serde_json::json!({"pilot":c.target.pilot,"ship":c.target.ship,"anchor":c.anchor})).collect::<Vec<_>>()).to_string(),error:None});
    let original_names = card_pilot_names(cards, catalog);
    fuzzy.retain(|c| {
        !original_names.iter().any(|name| {
            normalized_pilot(name).starts_with(&format!("{} ", normalized_pilot(&c.target.pilot)))
        })
    });
    if let Some(first) = fuzzy.first()
        && fuzzy
            .iter()
            .all(|c| same_card(first, c) && c.target.ship == first.target.ship)
    {
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for c in &fuzzy {
            *counts.entry(normalized_pilot(&c.target.pilot)).or_default() += 1;
        }
        let max = counts.values().copied().max().unwrap_or(0);
        let winners: Vec<_> = counts
            .iter()
            .filter(|(_, count)| **count == max)
            .map(|(name, _)| name.clone())
            .collect();
        if let [winner] = winners.as_slice() {
            fuzzy.retain(|c| normalized_pilot(&c.target.pilot) == *winner);
        }
    }
    let mut targets = if exact.is_empty() {
        if let Some(note) = confirm_card_name(&mut fuzzy, &text, retries) {
            retries.push(RetryRead {
                angle_degrees: None,
                method: "cards_name_confirmed_right".into(),
                rect: [0, 0, selected_image.width(), selected_image.height()],
                text: note,
                error: None,
            });
        }
        let pilot = selected_pilot(&text).unwrap();
        let names = fuzzy.iter().map(|c| c.target.pilot.clone()).collect();
        if let Some(name) = closest_pilot(&pilot, names)? {
            fuzzy
                .into_iter()
                .filter(|c| c.target.pilot == name)
                .collect()
        } else {
            Vec::new()
        }
    } else {
        exact
    };
    targets.sort_by(|a, b| a.target.identity().cmp(&b.target.identity()));
    targets.dedup_by(|a, b| a.target.identity() == b.target.identity() && same_card(a, b));
    match targets.len() {
        0 => original,
        1 => {
            let target = targets.remove(0).target;
            retries.push(RetryRead {
                angle_degrees: None,
                method: "cards_match".into(),
                rect: [0, 0, cards_image.width(), cards_image.height()],
                text: format!("{} / {}", target.pilot, target.ship),
                error: None,
            });
            Ok(target)
        }
        _ => Err(
            "Повторное чтение карточек дало неоднозначную цель. Уточните область карточек".into(),
        ),
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
    let clean = selected_document(selected);
    if selected_pilot(&clean.text()).is_some()
        && read_distance(&clean.text()).is_none()
        && !retries.iter().any(|r| r.method.starts_with("distance_"))
    {
        retries.extend(crate::platform::distance_retry_texts(
            image, &clean, language,
        ));
    }
    if selected_pilot(&clean.text()).is_some_and(|p| p.chars().count() <= 6)
        && !retries.iter().any(|r| r.method.starts_with("nickname_"))
    {
        retries.extend(crate::platform::selected_retry_texts(
            image, &clean, language,
        ));
    }
    let text = selected_text(selected, retries);
    let result = identify_card(&text, cards, catalog);
    if result.is_ok() || selected_pilot(&text).is_none() {
        return result;
    }
    if !retries.iter().any(|r| r.method.starts_with("nickname_")) {
        retries.extend(crate::platform::selected_retry_texts(
            image, &clean, language,
        ));
    }
    identify_with_retries(&text, retries, cards, catalog)
}

#[cfg(test)]
mod card_read_regressions {
    use super::*;
    fn candidate(pilot: &str, x: f32) -> CardCandidate {
        CardCandidate {
            target: Target {
                pilot: pilot.into(),
                ship: "Thrasher".into(),
                distance: Some(21000.),
                angular: 0.,
            },
            anchor: (x, 180., 22.),
        }
    }
    fn reads() -> Vec<RetryRead> {
        (0..3)
            .map(|i| RetryRead {
                angle_degrees: None,
                method: format!("nickname_{i}"),
                rect: [0, 0, 80, 20],
                text: "Flora Artua".into(),
                error: None,
            })
            .collect()
    }
    #[test]
    fn rereading_one_card_does_not_turn_ocr_spellings_into_competing_pilots() {
        let mut candidates = vec![
            candidate("Fiora Artüa", 1000.),
            candidate("Fiora Artha", 1004.),
        ];
        assert!(confirm_card_name(&mut candidates, "Flora Artua", &reads()).is_some());
        assert!(candidates.iter().all(|c| c.target.pilot == "Flora Artua"));
        let mut separate = vec![
            candidate("Fiora Artüa", 1000.),
            candidate("Fiora Artha", 1200.),
        ];
        assert!(confirm_card_name(&mut separate, "Flora Artua", &reads()).is_none());
        let mut mixed = vec![
            candidate("Fiora Artüa", 1000.),
            candidate("Fiora Artha", 1004.),
        ];
        mixed[1].target.ship = "Rifter".into();
        assert!(confirm_card_name(&mut mixed, "Flora Artua", &reads()).is_none());
    }
    #[test]
    fn changing_right_reads_cannot_overwrite_a_clearer_card_nickname() {
        let mut reads = reads();
        reads[2].text = "Flora Artur".into();
        let mut candidates = vec![candidate("Flora Artur", 1000.)];
        assert!(confirm_card_name(&mut candidates, "Flora Artua", &reads).is_none());
        assert_eq!(candidates[0].target.pilot, "Flora Artur");
        assert!(closest_pilot("Pilot8", vec!["Pilot9".into(), "Pilot7".into()]).is_err());
    }
}
