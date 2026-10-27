//! Публичные zKillboard/ESI: кеш HTTP, ограничение частоты и ручной импорт.

use crate::{
    model::{array, id, s},
    store::Store,
};
use chrono::{Datelike, NaiveDate, TimeDelta, Utc};
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub struct Sources {
    pub store: Store,
    cancel: Arc<AtomicBool>,
    agent: ureq::Agent,
    last: Instant,
}
impl Sources {
    pub fn refresh_statistics(
        &mut self,
        ship: i32,
        settings: &crate::model::Settings,
        progress: &dyn Fn(String),
    ) -> Result<usize, String> {
        if settings.force_refresh {
            self.store.invalidate_statistics_cache()?;
        }
        if settings.sync_scope == 2 {
            let end = Utc::now().date_naive() - TimeDelta::days(1);
            let start = if settings.days == 0 {
                NaiveDate::from_ymd_opt(2003, 5, 1).unwrap()
            } else {
                end - TimeDelta::days(settings.days.saturating_sub(1) as i64)
            };
            if settings.force_refresh {
                self.store.prepare_archive_refresh(
                    &start.format("%Y%m%d").to_string(),
                    &end.format("%Y%m%d").to_string(),
                )?;
            }
            let count = self.archive(start, end, progress)?;
            self.store
                .set_meta("archive_refresh_pending", &Value::Null)?;
            return Ok(count);
        }
        let mut ships = if settings.sync_scope == 1 {
            self.store.global_ships()?
        } else {
            Vec::new()
        };
        if !ships.contains(&ship) {
            ships.push(ship);
        }
        let mut count = 0;
        for (index, hull) in ships.iter().enumerate() {
            self.check()?;
            let report = |text| progress(format!("Корпус {}/{} · {text}", index + 1, ships.len()));
            count += self.sync_ship(*hull, settings.days, &report)?;
        }
        Ok(count)
    }
    pub fn new(store: Store, cancel: Arc<AtomicBool>) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(25)))
            .build()
            .into();
        Self {
            store,
            cancel,
            agent,
            last: Instant::now() - Duration::from_secs(2),
        }
    }
    fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err("Обновление остановлено".into())
        } else {
            Ok(())
        }
    }
    fn wait(&self, seconds: f64) -> Result<(), String> {
        let end = Instant::now() + Duration::from_secs_f64(seconds.max(0.));
        while Instant::now() < end {
            self.check()?;
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
    pub fn request(&mut self, url: &str, body: Option<&Value>, ttl: i64) -> Result<Value, String> {
        self.check()?;
        let key = format!("{url}{}", body.map(Value::to_string).unwrap_or_default());
        if ttl > 0
            && let Some(v) = self.store.cached(&key)
        {
            return Ok(v);
        }
        if url.contains("zkillboard.com/api/") {
            self.wait((1.1 - self.last.elapsed().as_secs_f64()).max(0.))?;
            self.last = Instant::now();
        }
        for attempt in 0..3 {
            self.check()?;
            let result = if let Some(b) = body {
                self.agent
                    .post(url)
                    .header(
                        "User-Agent",
                        "EVECombatAssistant/0.2 (haeniken; local desktop)",
                    )
                    .header("X-Compatibility-Date", "2026-10-27")
                    .send_json(b)
            } else {
                self.agent
                    .get(url)
                    .header(
                        "User-Agent",
                        "EVECombatAssistant/0.2 (haeniken; local desktop)",
                    )
                    .header("Accept-Language", "en")
                    .header("X-Compatibility-Date", "2026-10-27")
                    .call()
            };
            match result {
                Ok(mut response) => {
                    let mut raw = Vec::new();
                    response
                        .body_mut()
                        .as_reader()
                        .take(100 * 1024 * 1024 + 1)
                        .read_to_end(&mut raw)
                        .map_err(|e| e.to_string())?;
                    if raw.len() > 100 * 1024 * 1024 {
                        return Err("Слишком большой ответ источника".into());
                    }
                    let value: Value = serde_json::from_slice(&raw)
                        .map_err(|_| "Источник вернул некорректные данные")?;
                    if let Some(error) = value.get("error").and_then(Value::as_str) {
                        return Err(error.chars().take(200).collect());
                    }
                    if ttl > 0 {
                        self.store.cache(&key, &value, ttl)?;
                    }
                    return Ok(value);
                }
                Err(ureq::Error::StatusCode(code))
                    if [420, 429, 502, 503, 504].contains(&code) && attempt < 2 =>
                {
                    self.wait((2_u32.pow(attempt + 1)) as f64)?
                }
                Err(e) => return Err(format!("Не удалось получить данные: {e}")),
            }
        }
        Err("Источник временно недоступен".into())
    }
    fn complete(&mut self, row: &Value) -> Result<Value, String> {
        if row.get("victim").is_some() {
            return Ok(row.clone());
        }
        let hash = s(&row["zkb"], "hash");
        let km = id(row, "killmail_id");
        if km == 0 || hash.is_empty() {
            return Err("Killmail без ID или hash".into());
        }
        let mut mail = self.request(
            &format!("https://esi.evetech.net/killmails/{km}/{hash}"),
            None,
            2592000,
        )?;
        mail["zkb"] = row["zkb"].clone();
        Ok(mail)
    }
    pub fn sync_ship(
        &mut self,
        ship: i32,
        days: u32,
        progress: &dyn Fn(String),
    ) -> Result<usize, String> {
        let now = Utc::now();
        let since = if days == 0 {
            NaiveDate::from_ymd_opt(2003, 5, 1).unwrap()
        } else {
            (now - TimeDelta::days(days as i64)).date_naive()
        };
        let (mut year, mut month) = (now.year(), now.month());
        let mut total = 0;
        let mut months = Vec::new();
        while (year, month) >= (since.year(), since.month()) {
            let mut exhausted = false;
            for page in 1..=100 {
                self.check()?;
                progress(format!("Обновление статистики · страница {page}"));
                let url = format!(
                    "https://zkillboard.com/api/losses/shipTypeID/{ship}/year/{year}/month/{month}/page/{page}/"
                );
                let rows = self.request(&url, None, 3600)?;
                if !rows.is_array() {
                    return Err("Некорректный список потерь".into());
                }
                let rows = array(&rows);
                let mut mails = Vec::new();
                for row in rows {
                    let m = self.complete(row)?;
                    if id(&m["victim"], "ship_type_id") == ship as i64 {
                        mails.push(m);
                    }
                }
                total += self.store.ingest_global(&mails)?;
                if rows.len() < 200 {
                    exhausted = true;
                    break;
                }
            }
            if !exhausted {
                self.store.set_meta(
                    &format!("sync:{ship}"),
                    &json!({"complete":false,"days":days,"months":months}),
                )?;
                return Err("Достигнут предел месячного API. Используйте дневной архив".into());
            }
            months.push(format!("{year}-{month:02}"));
            if month == 1 {
                year -= 1;
                month = 12
            } else {
                month -= 1
            }
        }
        self.store.set_meta(
            &format!("sync:{ship}"),
            &json!({"complete":true,"days":days,"months":months,"updated":now.to_rfc3339()}),
        )?;
        Ok(total)
    }
    pub fn sync_pilot(
        &mut self,
        name: &str,
        ship: i32,
        progress: &dyn Fn(String),
    ) -> Result<(), String> {
        if name.trim().is_empty() {
            return Ok(());
        }
        progress("Поиск фита пилота…".into());
        let resolved = self.request(
            "https://esi.evetech.net/universe/ids",
            Some(&json!([name])),
            86400,
        )?;
        let character = array(&resolved["characters"])
            .iter()
            .find(|r| s(r, "name").to_lowercase() == name.to_lowercase())
            .map(|r| id(r, "id"));
        let Some(character) = character else {
            return Ok(());
        };
        self.store.set_meta(
            &format!("character:{}", name.to_lowercase()),
            &json!(character),
        )?;
        let rows = self.request(
            &format!(
                "https://zkillboard.com/api/losses/characterID/{character}/shipTypeID/{ship}/"
            ),
            None,
            3600,
        )?;
        let mut mails = Vec::new();
        for row in array(&rows) {
            let m = self.complete(row)?;
            if id(&m["victim"], "character_id") == character
                && id(&m["victim"], "ship_type_id") == ship as i64
            {
                mails.push(m);
            }
        }
        self.store.ingest(&mails)?;
        Ok(())
    }
    pub fn archive(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
        progress: &dyn Fn(String),
    ) -> Result<usize, String> {
        if start > end {
            return Err("Неверный период архива".into());
        }
        let mut date = start;
        let mut count = 0;
        let totals = self.request("https://r2z2.zkillboard.com/history/totals.json", None, 0)?;
        let totals = totals
            .as_object()
            .ok_or("Некорректный указатель дневного архива")?;
        while date <= end {
            let stamp = date.format("%Y%m%d");
            self.check()?;
            if totals
                .get(&stamp.to_string())
                .and_then(Value::as_i64)
                .unwrap_or(0)
                == 0
            {
                date += TimeDelta::days(1);
                continue;
            }
            if self.store.meta(&format!("archive:{stamp}")) == Some(json!(true)) {
                date += TimeDelta::days(1);
                continue;
            }
            progress(format!("Дневной архив · {stamp}"));
            let rows = self.request(
                &format!("https://r2z2.zkillboard.com/history/raw/{stamp}.json"),
                None,
                0,
            )?;
            let rows: Vec<Value> = if rows.is_array() {
                array(&rows).to_vec()
            } else {
                rows.as_object()
                    .ok_or("Некорректный архив")?
                    .values()
                    .cloned()
                    .collect()
            };
            let mut mails = Vec::new();
            for row in rows {
                mails.push(self.complete(&row)?);
            }
            count += self.store.ingest_global(&mails)?;
            self.store
                .set_meta(&format!("archive:{stamp}"), &json!(true))?;
            date += TimeDelta::days(1);
        }
        Ok(count)
    }
}
