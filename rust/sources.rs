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

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct UpdateProgress {
    pub total_days: usize,
    pub completed_days: usize,
    pub current_day: Option<String>,
    pub phase: String,
    pub downloaded_bytes: u64,
    pub inserted: usize,
    pub finished: bool,
    pub error: Option<String>,
}
impl UpdateProgress {
    pub fn fraction(&self) -> Option<f32> {
        (self.total_days > 0)
            .then(|| (self.completed_days as f32 / self.total_days as f32).clamp(0., 1.))
    }
    pub fn caption(&self) -> String {
        let mut text = self.phase.clone();
        if self.total_days > 0 {
            text += &format!(" · {} / {} дней", self.completed_days, self.total_days);
        }
        if let Some(day) = &self.current_day {
            text += &format!(" · {day}");
        }
        if self.downloaded_bytes > 0 && !self.finished {
            text += &format!(" · {:.1} МиБ", self.downloaded_bytes as f64 / 1048576.);
        }
        text += &format!(" · добавлено {}", self.inserted);
        text
    }
}

#[derive(Clone, Copy)]
struct RequestPolicy {
    limit: u64,
    attempts: u32,
    timeout_seconds: u64,
}
const ACTIVITY_POLICY: RequestPolicy = RequestPolicy {
    limit: 2 * 1024 * 1024,
    attempts: 1,
    timeout_seconds: 8,
};
pub struct Sources {
    pub store: Store,
    cancel: Arc<AtomicBool>,
    agent: ureq::Agent,
    last: Instant,
    trace: Option<crate::diagnostics::Trace>,
    catalog: crate::engine::Catalog,
    archive_base: String,
    update_progress: UpdateProgress,
    on_update: Option<Box<dyn Fn(UpdateProgress) + Send>>,
    interactive_deadline: Option<Instant>,
}
impl Sources {
    pub fn refresh_statistics(
        &mut self,
        settings: &crate::model::Settings,
        progress: &dyn Fn(String),
    ) -> Result<usize, String> {
        if settings.force_refresh {
            self.store.invalidate_statistics_cache()?;
        }
        let end = Utc::now().date_naive();
        let start = if settings.days == 0 {
            NaiveDate::from_ymd_opt(2003, 5, 1).unwrap()
        } else {
            end - TimeDelta::days(settings.days as i64)
        };
        self.record("statistics_update_start",json!({"scope":"all_catalog_hulls","period_start":start,"period_end":end,"force":settings.force_refresh,"catalog_hulls":self.catalog.ships.len(),"bundled":self.store.seed}));
        if settings.force_refresh {
            self.store.prepare_archive_refresh(
                &start.format("%Y%m%d").to_string(),
                &end.format("%Y%m%d").to_string(),
            )?;
        }
        let result = self.archive_update(start, end, settings.force_refresh, progress);
        self.record(
            "statistics_update_end",
            json!({"inserted":result.as_ref().ok(),"error":result.as_ref().err()}),
        );
        if result.is_ok() {
            self.store
                .set_meta("archive_refresh_pending", &Value::Null)?;
        }
        result
    }
    pub fn new(store: Store, cancel: Arc<AtomicBool>, catalog: crate::engine::Catalog) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Self {
            store,
            cancel,
            agent,
            last: Instant::now() - Duration::from_secs(2),
            trace: None,
            catalog,
            archive_base: "https://r2z2.zkillboard.com/history".into(),
            update_progress: UpdateProgress::default(),
            on_update: None,
            interactive_deadline: None,
        }
    }
    pub fn set_trace(&mut self, trace: crate::diagnostics::Trace) {
        self.store.set_trace(trace.clone());
        self.trace = Some(trace);
    }
    pub fn set_update_progress(&mut self, callback: impl Fn(UpdateProgress) + Send + 'static) {
        self.on_update = Some(Box::new(callback));
    }
    fn publish_update(&self) {
        if let Some(callback) = &self.on_update {
            callback(self.update_progress.clone());
        }
    }
    fn finish_day(&mut self) {
        self.update_progress.completed_days += 1;
        self.publish_update();
    }
    pub fn finish_update(&mut self, error: Option<String>) {
        self.update_progress.finished = true;
        self.update_progress.phase = if error.is_some() {
            "Обновление не завершено"
        } else {
            "База актуализирована"
        }
        .into();
        self.update_progress.error = error;
        self.update_progress.current_day = None;
        self.publish_update();
    }
    fn record(&self, kind: &str, fields: Value) {
        if let Some(trace) = &self.trace {
            trace.event(kind, fields);
        }
    }
    pub fn limit_interactive(&mut self, budget: Duration) {
        self.interactive_deadline = Some(Instant::now() + budget);
    }
    fn check(&self) -> Result<(), String> {
        if self
            .interactive_deadline
            .is_some_and(|end| Instant::now() >= end)
        {
            return Err("Время поиска истекло. Повторите запрос".into());
        }
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
        let limit = if url.contains("/history/raw/") {
            512 * 1024 * 1024
        } else {
            100 * 1024 * 1024
        };
        self.request_bounded(
            url,
            body,
            ttl,
            RequestPolicy {
                limit: if self.interactive_deadline.is_some() {
                    4 * 1024 * 1024
                } else {
                    limit
                },
                attempts: if self.interactive_deadline.is_some() {
                    1
                } else {
                    3
                },
                timeout_seconds: if self.interactive_deadline.is_some() {
                    8
                } else {
                    60
                },
            },
        )
    }
    fn request_bounded(
        &mut self,
        url: &str,
        body: Option<&Value>,
        ttl: i64,
        policy: RequestPolicy,
    ) -> Result<Value, String> {
        self.check()?;
        let request_id = crate::diagnostics::trace_id("http");
        let safe_url = crate::diagnostics::trace_url(url);
        let method = if body.is_some() { "POST" } else { "GET" };
        let shape = |value: &Value| -> Value {
            match value {
                Value::Array(rows) => json!({"type":"array","records":rows.len()}),
                Value::Object(map) => {
                    json!({"type":"object","keys":map.keys().take(20).collect::<Vec<_>>()})
                }
                _ => json!({"type":"scalar"}),
            }
        };
        self.record("http_request",json!({"request_id":request_id,"url":safe_url,"method":method,"cache_ttl_seconds":ttl,"body":body.map(shape)}));
        let key = format!("{url}{}", body.map(Value::to_string).unwrap_or_default());
        if ttl > 0
            && let Some(value) = self.store.cached(&key)
        {
            self.record(
                "http_cache_hit",
                json!({"request_id":request_id,"response":shape(&value)}),
            );
            return Ok(value);
        }
        self.record(
            "http_cache_miss",
            json!({"request_id":request_id,"cache_enabled":ttl>0}),
        );
        if url.contains("zkillboard.com/api/") {
            let wait = (1.1 - self.last.elapsed().as_secs_f64()).max(0.);
            self.record(
                "http_wait",
                json!({"request_id":request_id,"reason":"rate_limit","seconds":wait}),
            );
            self.wait(wait)?;
            self.last = Instant::now();
        }
        for attempt in 1..=policy.attempts {
            self.check()?;
            let started = Instant::now();
            self.record(
                "http_attempt",
                json!({"request_id":request_id,"attempt":attempt}),
            );
            let agent = concat!(
                "EVECombatAssistant/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/Haeniken/eve-combat-assistant)"
            );
            let timeout = Duration::from_secs(policy.timeout_seconds);
            let timeout = self
                .interactive_deadline
                .map(|end| timeout.min(end.saturating_duration_since(Instant::now())))
                .unwrap_or(timeout);
            let result = if let Some(body) = body {
                self.agent
                    .post(url)
                    .config()
                    .timeout_global(Some(timeout))
                    .build()
                    .header("User-Agent", agent)
                    .header("X-Compatibility-Date", "2026-10-27")
                    .send_json(body)
            } else {
                self.agent
                    .get(url)
                    .config()
                    .timeout_global(Some(timeout))
                    .build()
                    .header("User-Agent", agent)
                    .header("Accept-Language", "en")
                    .header("X-Compatibility-Date", "2026-10-27")
                    .call()
            };
            let mut response = match result {
                Ok(response) => response,
                Err(error) => {
                    let category = match &error {
                        ureq::Error::Timeout(_) => "timeout",
                        ureq::Error::HostNotFound => "dns",
                        ureq::Error::ConnectionFailed => "connection",
                        ureq::Error::Io(_) => "io",
                        ureq::Error::Tls(_) => "tls",
                        ureq::Error::RedirectFailed => "redirect",
                        _ => "transport",
                    };
                    self.record("http_error",json!({"request_id":request_id,"attempt":attempt,"category":category,"elapsed_ms":started.elapsed().as_millis()}));
                    if attempt < policy.attempts {
                        self.record("http_retry",json!({"request_id":request_id,"attempt":attempt,"seconds":2_u32.pow(attempt)}));
                        self.wait(2_u32.pow(attempt) as f64)?;
                        continue;
                    }
                    return Err(format!("Не удалось получить данные: {error}"));
                }
            };
            let status = response.status().as_u16();
            let mut headers = serde_json::Map::new();
            for name in [
                "content-type",
                "content-length",
                "retry-after",
                "cache-control",
                "etag",
                "x-esi-error-limit-remain",
                "x-esi-error-limit-reset",
            ] {
                if let Some(value) = response.headers().get(name).and_then(|v| v.to_str().ok()) {
                    headers.insert(name.into(), json!(value));
                }
            }
            self.record("http_headers",json!({"request_id":request_id,"attempt":attempt,"status":status,"headers":headers,"elapsed_ms":started.elapsed().as_millis()}));
            if !(200..300).contains(&status) {
                if [420, 429, 502, 503, 504].contains(&status) && attempt < policy.attempts {
                    let seconds = response
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(2_u32.pow(attempt) as f64)
                        .clamp(1., 120.);
                    self.record("http_retry",json!({"request_id":request_id,"attempt":attempt,"status":status,"seconds":seconds}));
                    drop(response);
                    self.wait(seconds)?;
                    continue;
                }
                self.record("http_error",json!({"request_id":request_id,"attempt":attempt,"status":status,"category":"status","elapsed_ms":started.elapsed().as_millis()}));
                return Err(format!("Источник вернул HTTP {status}"));
            }
            let limit = policy.limit;
            let mut raw = Vec::new();
            let track_download = url.contains("/raw/") && self.on_update.is_some();
            let reading = (|| -> std::io::Result<()> {
                let mut reader = response.body_mut().as_reader().take(limit + 1);
                let mut chunk = [0; 65536];
                let mut last_progress = Instant::now() - Duration::from_secs(1);
                loop {
                    self.check().map_err(std::io::Error::other)?;
                    let read = reader.read(&mut chunk)?;
                    if read == 0 {
                        break;
                    }
                    raw.extend_from_slice(&chunk[..read]);
                    if track_download && last_progress.elapsed() >= Duration::from_millis(200) {
                        self.update_progress.downloaded_bytes = raw.len() as u64;
                        self.publish_update();
                        last_progress = Instant::now();
                    }
                }
                if track_download {
                    self.update_progress.downloaded_bytes = raw.len() as u64;
                    self.publish_update();
                }
                Ok(())
            })();
            if let Err(error) = reading {
                self.record("http_error",json!({"request_id":request_id,"attempt":attempt,"status":status,"category":"body_read","bytes":raw.len(),"elapsed_ms":started.elapsed().as_millis()}));
                return Err(error.to_string());
            }
            if raw.len() as u64 > limit {
                self.record("http_error",json!({"request_id":request_id,"category":"body_limit","bytes":raw.len(),"limit":limit}));
                return Err("Слишком большой ответ источника".into());
            }
            let value: Value = match serde_json::from_slice(&raw) {
                Ok(value) => value,
                Err(_) => {
                    self.record("http_error",json!({"request_id":request_id,"category":"invalid_json","bytes":raw.len()}));
                    return Err("Источник вернул некорректные данные".into());
                }
            };
            if let Some(error) = value.get("error").and_then(Value::as_str) {
                self.record(
                    "http_error",
                    json!({"request_id":request_id,"category":"api_error","status":status}),
                );
                return Err(error.chars().take(200).collect());
            }
            if ttl > 0
                && let Err(error) = self.store.cache(&key, &value, ttl)
            {
                self.record(
                    "http_cache_write_error",
                    json!({"request_id":request_id,"error":error}),
                );
            }
            self.record("http_complete",json!({"request_id":request_id,"attempt":attempt,"status":status,"bytes":raw.len(),"response":shape(&value),"elapsed_ms":started.elapsed().as_millis(),"cached":ttl>0}));
            return Ok(value);
        }
        Err("Источник временно недоступен".into())
    }
    fn complete(&mut self, row: &Value) -> Result<Value, String> {
        if row.get("victim").is_some() {
            return Ok(compact_mail(row, &self.catalog));
        }
        let hash = s(&row["zkb"], "hash");
        let km = id(row, "killmail_id");
        if km == 0 || hash.is_empty() {
            return Err("Killmail без ID или hash".into());
        }
        let mut mail = self.request(
            &format!("https://esi.evetech.net/killmails/{km}/{hash}/"),
            None,
            2592000,
        )?;
        mail["zkb"] = row["zkb"].clone();
        Ok(compact_mail(&mail, &self.catalog))
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
    fn resolve_pilot(&mut self, name: &str) -> Result<Option<i64>, String> {
        if let Some(character) = self
            .store
            .meta(&format!("character:{}", name.to_lowercase()))
            .and_then(|v| v.as_i64())
            .filter(|id| *id > 0)
        {
            return Ok(Some(character));
        }
        let resolved = self.request(
            "https://esi.evetech.net/universe/ids/",
            Some(&json!([name])),
            86400,
        )?;
        let character = array(&resolved["characters"])
            .iter()
            .find(|r| s(r, "name").to_lowercase() == name.to_lowercase())
            .map(|r| id(r, "id"))
            .filter(|id| *id > 0);
        if let Some(character) = character {
            if let Some(row) = array(&resolved["characters"])
                .iter()
                .find(|row| id(row, "id") == character)
            {
                self.store
                    .set_meta(&format!("character_name:{character}"), &row["name"])?;
            }
            self.store.set_meta(
                &format!("character:{}", name.to_lowercase()),
                &json!(character),
            )?;
        }
        Ok(character)
    }
    pub fn pilot_rating(&mut self, name: &str) -> Result<crate::pilot::Rating, String> {
        let character = self.resolve_pilot(name)?.ok_or("Пилот не найден в ESI")?;
        self.character_rating(character)
    }
    pub fn character_rating(&mut self, character: i64) -> Result<crate::pilot::Rating, String> {
        if character <= 0 {
            return Err("Не выбран персонаж для соло-оценки".into());
        }
        self.character_rating_at(
            character,
            &format!("https://zkillboard.com/api/stats/characterID/{character}/"),
        )
    }
    fn character_rating_at(
        &mut self,
        character: i64,
        url: &str,
    ) -> Result<crate::pilot::Rating, String> {
        let missing_key = format!("pilot_stats_missing:v1:{character}");
        let previous = || {
            crate::pilot::cached_character(&self.store, character)
                .unwrap_or_else(|| crate::pilot::Rating::unavailable(character))
        };
        if self
            .store
            .meta(&missing_key)
            .and_then(|v| v.as_i64())
            .is_some_and(|at| (0..3600).contains(&Utc::now().timestamp().saturating_sub(at)))
        {
            self.record(
                "pilot_stats_missing_cache_hit",
                json!({"character_id":character}),
            );
            return Ok(previous());
        }
        let stats = match self.request(url, None, 3600) {
            Ok(stats) => stats,
            Err(error) if error.eq_ignore_ascii_case("Invalid type or id") => {
                self.store
                    .set_meta(&missing_key, &json!(Utc::now().timestamp()))?;
                self.record("pilot_stats_unavailable",json!({"character_id":character,"reason":"no_statistics_for_resolved_identity"}));
                return Ok(crate::pilot::cached_character(&self.store, character)
                    .unwrap_or_else(|| crate::pilot::Rating::unavailable(character)));
            }
            Err(error) => return Err(error),
        };
        self.store.set_meta(&missing_key, &Value::Null)?;
        let mut rating = crate::pilot::Rating::from_stats(&stats, character)?;
        if let Some(cached) = crate::pilot::cached_activity(&self.store, character) {
            rating.recent = cached.activity;
        }
        self.store.set_meta(
            &format!("pilot_rating:v1:{character}"),
            &serde_json::to_value(&rating).map_err(|e| e.to_string())?,
        )?;
        self.record(
            "pilot_rating_ready",
            json!({"character_id":character,"rating":rating}),
        );
        Ok(rating)
    }
    pub fn recent_participation(&mut self, rating: &mut crate::pilot::Rating) {
        let url = format!(
            "https://zkillboard.com/api/kills/characterID/{}/npc/0/pastSeconds/259200/",
            rating.character_id
        );
        self.recent_participation_at(rating, &url, Utc::now().timestamp());
    }
    fn recent_participation_at(&mut self, rating: &mut crate::pilot::Rating, url: &str, now: i64) {
        use crate::pilot::{ACTIVITY_TTL, ActivityCache, RecentActivity};
        let character = rating.character_id;
        let cached = crate::pilot::cached_activity(&self.store, character);
        if let Some(cached) = &cached
            && (0..ACTIVITY_TTL).contains(&now.saturating_sub(cached.checked_at))
        {
            rating.recent = cached.activity.clone();
            self.record(
                "pilot_activity_cache_hit",
                json!({"character_id":character,"activity":rating.recent}),
            );
            return;
        }
        let mut activity = cached.map(|c| c.activity).unwrap_or_default();
        self.record("pilot_activity_start",json!({"character_id":character,"period_seconds":crate::pilot::ACTIVITY_WINDOW,"max_pages":1,"max_records":200,"max_body_bytes":ACTIVITY_POLICY.limit,"timeout_seconds":ACTIVITY_POLICY.timeout_seconds,"attempts":1}));
        let result = self
            .request_bounded(url, None, 0, ACTIVITY_POLICY)
            .and_then(|value| {
                let rows = value
                    .as_array()
                    .ok_or("Источник не вернул список недавних боёв")?;
                let scanned = RecentActivity::from_kills(rows, character, now);
                Ok((scanned, rows.len()))
            });
        if let Ok((found, _)) = &result {
            activity.merge(found.clone());
        }
        rating.recent = activity.clone();
        // Failures also get an hour cooldown; confirmed timestamps remain valid until they age out.
        let cache = ActivityCache {
            character_id: character,
            checked_at: now,
            activity,
        };
        let saved = self
            .store
            .set_meta(&format!("pilot_activity:v1:{character}"), &json!(cache));
        self.record("pilot_activity_end",json!({"character_id":character,"records":result.as_ref().ok().map(|(_,n)|n),"activity":rating.recent,"error":result.as_ref().err(),"cache_write_error":saved.err()}));
    }
    pub fn lookup_profile(&mut self, name: &str) -> Result<crate::pilot_lookup::Profile, String> {
        let character = self.resolve_pilot(name)?.ok_or("Пилот не найден в ESI")?;
        let name = self
            .store
            .meta(&format!("character_name:{character}"))
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| name.to_string());
        let rating = match self.character_rating(character) {
            Ok(rating) => rating,
            Err(error) => {
                self.record(
                    "pilot_lookup_rating_unavailable",
                    json!({"character_id":character,"error":error}),
                );
                crate::pilot::cached_character(&self.store, character)
                    .unwrap_or_else(|| crate::pilot::Rating::unavailable(character))
            }
        };
        Ok(crate::pilot_lookup::Profile { name, rating })
    }
    pub fn lookup_latest_loss(
        &mut self,
        character: i64,
    ) -> Result<Option<crate::pilot_lookup::Loss>, String> {
        self.lookup_latest_loss_at(
            character,
            &format!("https://zkillboard.com/api/losses/characterID/{character}/npc/0/"),
        )
    }
    fn lookup_latest_loss_at(
        &mut self,
        character: i64,
        url: &str,
    ) -> Result<Option<crate::pilot_lookup::Loss>, String> {
        for page in 1..=100 {
            self.check()?;
            let page_url = if page == 1 {
                url.to_string()
            } else {
                format!("{}/page/{page}/", url.trim_end_matches('/'))
            };
            let value = self.request(&page_url, None, 3600)?;
            let rows = value.as_array().ok_or("Источник не вернул список потерь")?;
            let full_page = rows.len() >= 200;
            let mut rows: Vec<_> = rows.iter().take(200).collect();
            rows.sort_by_key(|row| std::cmp::Reverse(id(row, "killmail_id")));
            for row in rows {
                self.check()?;
                if row["zkb"]["npc"] == true {
                    continue;
                }
                let mail = self.complete(row)?;
                if id(&mail["victim"], "character_id") != character
                    || !array(&mail["attackers"])
                        .iter()
                        .any(|a| id(a, "character_id") > 0 && id(a, "character_id") != character)
                    || mail["zkb"]["npc"] == true
                {
                    continue;
                }
                let ship = id(&mail["victim"], "ship_type_id") as i32;
                if id(self.catalog.get(ship), "group") == 29 {
                    self.record("pilot_loss_skipped",json!({"reason":"capsule","character_id":character,"killmail_id":mail["killmail_id"],"ship_type_id":ship,"page":page}));
                    continue;
                }
                let fit = crate::fits::normalize(&mail, &self.catalog);
                return Ok(Some(crate::pilot_lookup::Loss {
                    killmail_id: id(&mail, "killmail_id"),
                    happened: s(&mail, "killmail_time").to_string(),
                    ship: self.catalog.esi_item_name(ship)?,
                    description: fit
                        .as_ref()
                        .map(|fit| crate::fits::describe(fit, &self.catalog).0),
                    eft: fit.as_ref().map(|fit| crate::fits::eft(fit, &self.catalog)),
                    fit,
                }));
            }
            if !full_page {
                return Ok(None);
            }
        }
        Err("Достигнут предел истории потерь".into())
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
        let character = self.resolve_pilot(name)?;
        let Some(character) = character else {
            self.record("pilot_not_found", json!({"pilot":name}));
            return Ok(());
        };
        self.record(
            "pilot_resolved",
            json!({"pilot":name,"character_id":character,"ship_id":ship}),
        );
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
        let inserted = self.store.ingest(&mails)?;
        self.record("pilot_ingested",json!({"pilot":name,"character_id":character,"ship_id":ship,"matching_losses":mails.len(),"inserted":inserted}));
        Ok(())
    }
    pub fn archive(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
        progress: &dyn Fn(String),
    ) -> Result<usize, String> {
        self.archive_update(start, end, false, progress)
    }
    fn archive_update(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
        force: bool,
        progress: &dyn Fn(String),
    ) -> Result<usize, String> {
        if start > end {
            return Err("Неверный период архива".into());
        }
        self.update_progress = UpdateProgress {
            total_days: (end - start).num_days() as usize + 1,
            phase: "Проверка архива".into(),
            ..Default::default()
        };
        self.publish_update();
        let totals = self.request(&format!("{}/totals.json", self.archive_base), None, 0)?;
        let totals = totals
            .as_object()
            .ok_or("Некорректный указатель дневного архива")?;
        let (mut date, mut count, mut failed) = (end, 0, Vec::new());
        while date >= start {
            self.check()?;
            let stamp = date.format("%Y%m%d").to_string();
            self.update_progress.current_day = Some(date.format("%d.%m.%Y").to_string());
            self.update_progress.downloaded_bytes = 0;
            self.update_progress.phase = "Проверка локальной базы".into();
            self.publish_update();
            let expected = totals.get(&stamp).and_then(Value::as_u64).unwrap_or(0) as usize;
            let covered = self.store.archive_count(&stamp);
            self.record("archive_day",json!({"day":stamp,"expected":expected,"local_or_bundled":covered,"download":expected>0 && (force || covered!=expected)}));
            if expected == 0 || !force && covered == expected {
                self.finish_day();
                date -= TimeDelta::days(1);
                continue;
            }
            progress(format!(
                "База всех кораблей · {} · добавлено {count}",
                date.format("%d.%m.%Y")
            ));
            self.update_progress.phase = "Загрузка данных".into();
            self.publish_update();
            let result = self.request(&format!("{}/raw/{stamp}.json", self.archive_base), None, 0);
            let rows = match result {
                Ok(rows) => rows,
                Err(error) => {
                    self.check()?;
                    self.record("archive_day_error", json!({"day":stamp,"error":error}));
                    failed.push(stamp);
                    self.finish_day();
                    date -= TimeDelta::days(1);
                    continue;
                }
            };
            let rows: Vec<Value> = if rows.is_array() {
                array(&rows).to_vec()
            } else {
                rows.as_object()
                    .ok_or("Некорректный архив")?
                    .values()
                    .cloned()
                    .collect()
            };
            let source_count = rows.len();
            let mut verified = source_count == expected;
            if !verified {
                let ids: std::collections::HashSet<_> = rows
                    .iter()
                    .map(|r| id(r, "killmail_id").to_string())
                    .collect();
                let index = self.request(&format!("{}/{stamp}.json", self.archive_base), None, 0);
                verified = index
                    .as_ref()
                    .ok()
                    .and_then(Value::as_object)
                    .is_some_and(|index| {
                        index.len() == ids.len() && index.keys().all(|key| ids.contains(key))
                    });
                self.record("archive_index_verification",json!({"day":stamp,"published_total":expected,"available_records":source_count,"verified":verified,"error":index.as_ref().err()}));
            }

            let mut mails = Vec::new();
            for row in rows {
                let row = self.complete(&row)?;
                if self.catalog.get(id(&row["victim"], "ship_type_id") as i32)["category"] == 6 {
                    mails.push(row);
                }
            }
            self.update_progress.phase = "Сохранение данных".into();
            self.publish_update();
            let inserted = self.store.ingest_archive(&mails, force)?;
            count += inserted;
            self.update_progress.inserted = count;
            self.store.set_meta(
                &format!("archive_count:{stamp}"),
                &json!(if verified { expected } else { source_count }),
            )?;
            self.record("archive_day_ingested",json!({"day":stamp,"source_count":source_count,"ship_records":mails.len(),"inserted":inserted,"complete":verified}));
            if !verified {
                failed.push(stamp);
            }
            self.finish_day();
            date -= TimeDelta::days(1);
        }
        self.record(
            "archive_complete",
            json!({"inserted":count,"failed_days":failed}),
        );
        if failed.is_empty() {
            Ok(count)
        } else {
            Err(format!(
                "База обновлена частично: добавлено {count}, не завершено дней: {}. Повторите обновление",
                failed.len()
            ))
        }
    }
}
/// Same public fitting subset as the installer builder. No response auth data.
pub fn compact_mail(mail: &Value, catalog: &crate::engine::Catalog) -> Value {
    let victim = &mail["victim"];
    let mut seen = std::collections::HashSet::new();
    let attackers: Vec<_> = array(&mail["attackers"])
        .iter()
        .filter_map(|a| {
            let character = id(a, "character_id");
            (character > 0 && seen.insert(character)).then(|| json!({"character_id":character}))
        })
        .take(6)
        .collect();
    let items: Vec<_> = array(&victim["items"])
        .iter()
        .filter(|item| {
            let flag = id(item, "flag");
            let category = id(catalog.get(id(item, "item_type_id") as i32), "category");
            ([7, 8, 32, 66].contains(&category)
                && ((11..=34).contains(&flag)
                    || (92..=99).contains(&flag)
                    || (125..=132).contains(&flag)))
                || category == 18 && flag == 87
        })
        .map(|item| {
            let mut row = serde_json::Map::new();
            for key in [
                "item_type_id",
                "flag",
                "quantity_destroyed",
                "quantity_dropped",
            ] {
                if let Some(value) = item.get(key) {
                    row.insert(key.into(), value.clone());
                }
            }
            Value::Object(row)
        })
        .collect();
    let mut result = json!({"killmail_id":mail["killmail_id"],"killmail_time":mail["killmail_time"],"solar_system_id":mail["solar_system_id"],"victim":{"ship_type_id":victim["ship_type_id"],"character_id":victim["character_id"],"corporation_id":victim["corporation_id"],"items":items},"attackers":attackers});
    if let Some(zkb) = mail.get("zkb") {
        let mut flags = serde_json::Map::new();
        for key in ["npc", "awox"] {
            if let Some(value) = zkb.get(key) {
                flags.insert(key.into(), value.clone());
            }
        }
        result["zkb"] = Value::Object(flags);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, net::TcpListener};
    fn mock(responses: Vec<(u16, Value)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(15);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(10))
                        }
                        Err(e) => panic!("mock HTTP accept: {e}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = [0; 8192];
                let n = stream.read(&mut bytes).unwrap();
                requests.push(
                    String::from_utf8_lossy(&bytes[..n])
                        .lines()
                        .next()
                        .unwrap()
                        .to_string(),
                );
                let body = body.to_string();
                write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nRetry-After: 1\r\nSet-Cookie: MUST_NOT_APPEAR\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
            requests
        });
        (address, thread)
    }
    fn temp() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(crate::diagnostics::trace_id("eve-source-test"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
    #[test]
    fn recent_activity_is_cached_for_an_hour_and_failure_keeps_confirmed_timestamp() {
        let directory = temp();
        let now = Utc::now().timestamp();
        let rows = json!([{"killmail_time":chrono::DateTime::from_timestamp(now-6*3600,0).unwrap().to_rfc3339(),"victim":{"character_id":456},"attackers":[{"character_id":123},{"character_id":124}],"zkb":{"npc":false,"solo":false}}]);
        let (url, server) = mock(vec![(200, rows), (503, json!({"error":"unavailable"}))]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.set_trace(
            crate::diagnostics::Trace::new(directory.join("debug"))
                .job(true, json!({"job_id":"activity-test"})),
        );
        let mut rating=crate::pilot::Rating::from_stats(&json!({"type":"characterID","id":123,"shipsDestroyed":100,"shipsLost":10,"soloKills":1,"soloLosses":10}),123).unwrap();
        source
            .store
            .set_meta("pilot_rating:v1:123", &json!(rating))
            .unwrap();
        source.recent_participation_at(&mut rating, &url, now);
        assert_eq!(rating.recent.group_at, Some(now - 6 * 3600));
        assert_eq!(
            crate::pilot::cached_character(&source.store, 123)
                .unwrap()
                .recent
                .group_at,
            rating.recent.group_at
        );
        source.recent_participation_at(&mut rating, &url, now + 3599);
        source.recent_participation_at(&mut rating, &url, now + 3600);
        source.recent_participation_at(&mut rating, &url, now + 3601);
        assert_eq!(rating.recent.group_at, Some(now - 6 * 3600));
        let cache = crate::pilot::cached_activity(&source.store, 123).unwrap();
        assert_eq!(cache.checked_at, now + 3600);
        assert_eq!(server.join().unwrap().len(), 2);
        let log = std::fs::read_to_string(directory.join("debug/fits.jsonl")).unwrap();
        assert_eq!(
            log.lines()
                .filter(|l| l.contains("\"kind\":\"http_attempt\""))
                .count(),
            2
        );
        assert!(!log.contains("http_retry"));
        assert!(log.contains("pilot_activity_cache_hit"));
    }
    #[test]
    fn recent_solo_activity_uses_one_cached_request_without_a_group_marker() {
        let directory = temp();
        let now = Utc::now().timestamp();
        let rows = json!([{"killmail_time":chrono::DateTime::from_timestamp(now-300,0).unwrap().to_rfc3339(),"victim":{"character_id":456},"attackers":[{"character_id":123}],"zkb":{"solo":true,"npc":false}}]);
        let (url, server) = mock(vec![(200, rows)]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        let mut rating=crate::pilot::Rating::from_stats(&json!({"type":"characterID","id":123,"shipsDestroyed":100,"shipsLost":30,"soloKills":70,"soloLosses":20}),123).unwrap();
        assert!(rating.group_marker().is_none());
        source.recent_participation_at(&mut rating, &url, now);
        assert_eq!(rating.recent.solo_at, Some(now - 300));
        source.recent_participation_at(&mut rating, &url, now + 3599);
        assert_eq!(server.join().unwrap().len(), 1);
        assert_eq!(
            crate::pilot::cached_activity(&source.store, 123)
                .unwrap()
                .activity
                .solo_at,
            Some(now - 300)
        );
    }
    #[test]
    fn oversized_recent_activity_does_not_retry_or_erase_the_rating() {
        use std::{io::Write, net::TcpListener};
        let directory = temp();
        let now = Utc::now().timestamp();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/activity", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = [0; 4096];
            let _ = stream.read(&mut bytes);
            let body =
                json!([{"padding":"x".repeat(ACTIVITY_POLICY.limit as usize+128)}]).to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body.as_bytes());
        });
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.set_trace(
            crate::diagnostics::Trace::new(directory.join("debug"))
                .job(true, json!({"job_id":"activity-limit-test"})),
        );
        let mut rating=crate::pilot::Rating::from_stats(&json!({"type":"characterID","id":123,"shipsDestroyed":100,"shipsLost":10,"soloKills":5,"soloLosses":10}),123).unwrap();
        let score = rating.score;
        source.recent_participation_at(&mut rating, &url, now);
        source.recent_participation_at(&mut rating, &url, now + 60);
        assert!(rating.recent.group_at.is_none());
        assert_eq!(rating.score, score);
        assert_eq!(
            crate::pilot::cached_activity(&source.store, 123)
                .unwrap()
                .checked_at,
            now
        );
        server.join().unwrap();
        let log = std::fs::read_to_string(directory.join("debug/fits.jsonl")).unwrap();
        assert!(log.contains("body_limit"));
        assert_eq!(
            log.lines()
                .filter(|l| l.contains("\"kind\":\"http_attempt\""))
                .count(),
            1
        );
    }
    #[test]
    fn missing_zkill_stats_keep_resolved_identity_unknown_score_and_hour_cooldown() {
        let directory = temp();
        let (url, server) = mock(vec![
            (200, json!({"error":"Invalid type or id"})),
            (
                200,
                json!({"type":"characterID","id":2124807832,"shipsDestroyed":100,"shipsLost":30,"soloKills":70,"soloLosses":20}),
            ),
        ]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        let unknown = source.character_rating_at(2124807832, &url).unwrap();
        assert!(!unknown.stats_available && unknown.score.is_none());
        assert_eq!(unknown.character_id, 2124807832);
        assert!(!unknown.hint().contains("Соло-убийства"));
        assert!(
            source
                .character_rating_at(2124807832, &url)
                .unwrap()
                .score
                .is_none()
        );
        source
            .store
            .set_meta(
                "pilot_stats_missing:v1:2124807832",
                &json!(Utc::now().timestamp() - 3601),
            )
            .unwrap();
        let recovered = source.character_rating_at(2124807832, &url).unwrap();
        assert!(recovered.stats_available && recovered.score.is_some());
        assert_eq!(server.join().unwrap().len(), 2);
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn quick_lookup_missing_stats_does_not_prevent_last_ship_loading() {
        let directory = temp();
        let store = Store::open_with_seed(&directory, None).unwrap();
        store
            .set_meta("character:archerxxxx", &json!(2124668894))
            .unwrap();
        store
            .set_meta(
                "pilot_stats_missing:v1:2124668894",
                &json!(Utc::now().timestamp()),
            )
            .unwrap();
        let (url, server) = mock(vec![(
            200,
            json!([{"killmail_id":1,"killmail_time":"2026-01-01T00:00:00Z","victim":{"character_id":2124668894,"ship_type_id":587,"items":[]},"attackers":[{"character_id":200}],"zkb":{"npc":false}}]),
        )]);
        let mut source = Sources::new(
            store,
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        let profile = source.lookup_profile("ArcherXXXX").unwrap();
        assert_eq!(profile.rating.character_id, 2124668894);
        assert!(!profile.rating.stats_available);
        assert_eq!(
            source
                .lookup_latest_loss_at(profile.rating.character_id, &url)
                .unwrap()
                .unwrap()
                .ship,
            "Rifter"
        );
        assert_eq!(server.join().unwrap().len(), 1);
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn last_loss_skips_consecutive_capsule_variants_and_keeps_newest_ship_without_modules() {
        let directory = temp();
        let row = |km, ship| json!({"killmail_id":km,"killmail_time":"2026-01-01T00:00:00Z","victim":{"character_id":123,"ship_type_id":ship,"items":[]},"attackers":[{"character_id":200}],"zkb":{"npc":false}});
        let capsules = json!([row(4, 670), row(3, 33328)]);
        let (url, server) = mock(vec![
            (
                200,
                json!([row(4, 670), row(2, 587), row(3, 33328), row(1, 33153)]),
            ),
            (200, capsules),
        ]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        let loss = source.lookup_latest_loss_at(123, &url).unwrap().unwrap();
        assert_eq!(loss.killmail_id, 2);
        assert_eq!(loss.ship, "Rifter");
        assert!(loss.fit.is_none());
        assert!(
            source
                .lookup_latest_loss_at(123, &format!("{url}/only-pods/"))
                .unwrap()
                .is_none()
        );
        assert_eq!(server.join().unwrap().len(), 2);
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn last_ship_continues_on_next_page_after_a_full_page_of_capsules() {
        let directory = temp();
        let row = |km, ship| json!({"killmail_id":km,"killmail_time":"2026-01-01T00:00:00Z","victim":{"character_id":123,"ship_type_id":ship,"items":[]},"attackers":[{"character_id":200}],"zkb":{"npc":false}});
        let pods: Vec<_> = (2..202).map(|id| row(id, 670)).collect();
        let (url, server) = mock(vec![(200, json!(pods)), (200, json!([row(1, 587)]))]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.limit_interactive(Duration::from_secs(10));
        assert_eq!(
            source
                .lookup_latest_loss_at(123, &url)
                .unwrap()
                .unwrap()
                .ship,
            "Rifter"
        );
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("/page/2/"));
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn quick_lookup_uses_lifetime_cached_rating_without_a_hull() {
        let directory = temp();
        let store = Store::open_with_seed(&directory, None).unwrap();
        store
            .set_meta("character:example pilot", &json!(123))
            .unwrap();
        store
            .set_meta("character_name:123", &json!("Example Pilot"))
            .unwrap();
        store.cache("https://zkillboard.com/api/stats/characterID/123/", &json!({"type":"characterID","id":123,"shipsDestroyed":1000,"shipsLost":30,"soloKills":70,"soloLosses":30}),3600).unwrap();
        let mut source = Sources::new(
            store,
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.limit_interactive(Duration::from_secs(1));
        let profile = source.lookup_profile("example pilot").unwrap();
        assert_eq!(profile.name, "Example Pilot");
        assert_eq!(profile.rating.character_id, 123);
        assert_eq!(profile.rating.solo_kills, 70);
        assert!(profile.rating.score.is_some());
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn quick_lookup_last_pvp_loss_accepts_any_hull_and_fleet_without_date_filter() {
        let directory = temp();
        let row = |km, character, npc, attackers, ship, items| json!({"killmail_id":km,"killmail_time":"2020-01-01T00:00:00Z","victim":{"character_id":character,"ship_type_id":ship,"items":items},"attackers":attackers,"zkb":{"npc":npc}});
        let mut fleet = Vec::new();
        for id in 200..230 {
            fleet.push(json!({"character_id":id}));
        }
        let rows = json!([
            row(4, 123, true, json!([]), 587, json!([])),
            row(3, 456, false, json!([{"character_id":200}]), 587, json!([])),
            row(1, 123, false, json!([{"character_id":200}]), 587, json!([])),
            row(
                2,
                123,
                false,
                json!(fleet),
                33153,
                json!([{"item_type_id":25715,"flag":27,"quantity_destroyed":1}])
            )
        ]);
        let (url, server) = mock(vec![(200, rows)]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.limit_interactive(Duration::from_secs(8));
        let loss = source.lookup_latest_loss_at(123, &url).unwrap().unwrap();
        assert_eq!(loss.killmail_id, 2);
        assert_eq!(loss.ship, "Drake Navy Issue");
        assert!(
            loss.eft
                .unwrap()
                .contains("Heavy Assault Missile Launcher II")
        );
        assert_eq!(
            source
                .lookup_latest_loss_at(123, &url)
                .unwrap()
                .unwrap()
                .killmail_id,
            2
        );
        assert_eq!(server.join().unwrap().len(), 1);
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn quick_lookup_keeps_latest_loss_even_without_modules_and_deadline_stops_requests() {
        let directory = temp();
        let (url, server) = mock(vec![(
            200,
            json!([{"killmail_id":3,"killmail_time":"2026-01-01T00:00:00Z","victim":{"character_id":123,"ship_type_id":587,"items":[]},"attackers":[{"character_id":200}],"zkb":{"npc":false}}]),
        )]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        let loss = source.lookup_latest_loss_at(123, &url).unwrap().unwrap();
        assert_eq!(loss.ship, "Rifter");
        assert!(loss.eft.is_none());
        source.limit_interactive(Duration::ZERO);
        assert!(
            source
                .lookup_latest_loss_at(123, &url)
                .unwrap_err()
                .contains("Время поиска")
        );
        assert_eq!(server.join().unwrap().len(), 1);
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn pilot_rating_uses_cached_identity_and_stats_independently_of_fitting() {
        let directory = temp();
        let store = Store::open_with_seed(&directory, None).unwrap();
        store.set_meta("character:pilot", &json!(123)).unwrap();
        store.cache("https://zkillboard.com/api/stats/characterID/123/",&json!({"type":"characterID","id":123,"shipsDestroyed":1000,"shipsLost":30,"soloKills":70,"soloLosses":30,"labels":{"#:10+":{"shipsDestroyed":800}}}),3600).unwrap();
        let mut source = Sources::new(
            store,
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.set_trace(
            crate::diagnostics::Trace::new(directory.join("debug"))
                .job(true, json!({"job_id":"rating-test"})),
        );
        let rating = source.pilot_rating("Pilot").unwrap();
        assert_eq!(rating.group_marker(), Some("БЛОБ"));
        assert_eq!(
            crate::pilot::cached(&source.store, "PILOT").unwrap().score,
            rating.score
        );
        assert!(crate::pilot::cached(&source.store, "Someone Else").is_none());
        // The logged-in character uses its known ID, the same calibration and HTTP cache.
        let own = source.character_rating(123).unwrap();
        assert_eq!(own.score, rating.score);
        assert_eq!(
            crate::pilot::cached_character(&source.store, 123)
                .unwrap()
                .score,
            rating.score
        );
        assert!(crate::pilot::cached_character(&source.store, 456).is_none());
        assert!(source.character_rating(0).is_err());
        let log = std::fs::read_to_string(directory.join("debug/fits.jsonl")).unwrap();
        assert!(log.contains("http_cache_hit") && log.contains("pilot_rating_ready"));
        assert!(!log.contains("http_attempt") && !log.contains("/api/losses/"));
    }
    #[test]
    fn http_retry_cache_and_trace_redact_secrets_and_response_body() {
        let directory = temp();
        let (base, server) = mock(vec![
            (429, json!({"ignored":"PRIVATE_RESPONSE"})),
            (200, json!([{"secret":"PRIVATE_RESPONSE"}])),
        ]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.set_trace(
            crate::diagnostics::Trace::new(directory.join("debug"))
                .job(true, json!({"job_id":"test"})),
        );
        let url = format!("{base}/test?token=PRIVATE_TOKEN&page=2");
        let expected = json!([{"secret":"PRIVATE_RESPONSE"}]);
        assert_eq!(source.request(&url, None, 3600).unwrap(), expected);
        assert_eq!(source.request(&url, None, 3600).unwrap(), expected);
        assert_eq!(server.join().unwrap().len(), 2);
        let log = std::fs::read_to_string(directory.join("debug/fits.jsonl")).unwrap();
        for absent in [
            "PRIVATE_TOKEN",
            "PRIVATE_RESPONSE",
            "MUST_NOT_APPEAR",
            "Set-Cookie",
        ] {
            assert!(!log.contains(absent), "trace leaked {absent}");
        }
        for event in [
            "http_retry",
            "http_cache_hit",
            "http_complete",
            "http_headers",
            "429",
            "page=2",
        ] {
            assert!(log.contains(event), "trace missing {event}");
        }
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn archive_visits_newest_days_first_and_does_not_reload_unchanged_days() {
        let directory = temp();
        let row = |id, day| json!({"killmail_id":id,"killmail_time":format!("2026-10-{day}T12:00:00Z"),"victim":{"ship_type_id":587,"items":[]},"attackers":[]});
        let (base, server) = mock(vec![
            (200, json!({"20261026":1,"20261027":1})),
            (200, json!({"2":row(2,27)})),
            (200, json!({"1":row(1,26)})),
        ]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.archive_base = format!("{base}/history");
        source
            .archive(
                NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
                NaiveDate::from_ymd_opt(2026, 10, 27).unwrap(),
                &|_| {},
            )
            .unwrap();
        let requests = server.join().unwrap();
        assert!(requests[1].contains("raw/20261027"));
        assert!(requests[2].contains("raw/20261026"));
    }
    #[test]
    fn archive_update_covers_all_hulls_and_only_downloads_new_or_changed_days() {
        let directory = temp();
        let record = |id, ship| json!({"killmail_id":id,"killmail_time":"2026-10-27T12:00:00Z","solar_system_id":30000142,"victim":{"ship_type_id":ship,"character_id":90000000+id,"corporation_id":98000000+id,"items":[]},"attackers":[{"character_id":80000000+id}]});
        let two = json!({"1":record(1,587),"2":record(2,47466)});
        let three = json!({"1":record(1,587),"2":record(2,47466),"3":record(3,593)});
        let totals = json!({"20261026":1,"20261027":2});
        let (base, server) = mock(vec![
            (200, totals.clone()),
            (200, two),
            (200, totals),
            (200, json!({"20261026":1,"20261027":3})),
            (200, three),
        ]);
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.archive_base = format!("{base}/history");
        let updates = Arc::new(std::sync::Mutex::new(Vec::<UpdateProgress>::new()));
        let observer = updates.clone();
        source.set_update_progress(move |progress| observer.lock().unwrap().push(progress));
        source
            .store
            .set_meta("archive_count:20261026", &json!(1))
            .unwrap();
        let start = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let end = start + TimeDelta::days(1);
        assert_eq!(source.archive(start, end, &|_| {}).unwrap(), 2);
        source.finish_update(None);
        {
            let updates = updates.lock().unwrap();
            assert_eq!(updates.first().unwrap().fraction(), Some(0.));
            assert_eq!(updates.last().unwrap().fraction(), Some(1.));
            assert!(updates.last().unwrap().finished);
            assert_eq!(updates.last().unwrap().inserted, 2);
            assert!(
                updates
                    .iter()
                    .any(|p| p.phase == "Загрузка данных" && p.downloaded_bytes > 0)
            );
            assert!(
                updates
                    .windows(2)
                    .all(|w| w[0].completed_days <= w[1].completed_days)
            );
        }
        assert_eq!(source.store.global_ships().unwrap(), vec![587, 47466]);
        assert_eq!(source.archive(start, end, &|_| {}).unwrap(), 0);
        assert_eq!(source.archive(start, end, &|_| {}).unwrap(), 1);
        assert_eq!(source.store.global_ships().unwrap(), vec![587, 593, 47466]);
        let requests = server.join().unwrap();
        assert_eq!(requests.iter().filter(|s| s.contains("/raw/")).count(), 2);
        assert!(requests.iter().all(|s| !s.contains("raw/20261026")));
        source.cancel.store(true, Ordering::Relaxed);
        let result = source.archive(start, end, &|_| {});
        assert!(result.is_err());
        source.finish_update(result.err());
        let last = updates.lock().unwrap().last().unwrap().clone();
        assert!(last.finished && last.error.is_some());
        assert_eq!(last.fraction(), Some(0.));
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod archive_index_tests {
    use super::*;
    #[test]
    fn discrepant_totals_require_matching_daily_ids_before_checkpointing() {
        use std::{io::Write, net::TcpListener};
        let directory = std::env::temp_dir().join(crate::diagnostics::trace_id("eve-index-test"));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let row = json!({"killmail_id":1,"killmail_time":"2026-10-27T12:00:00Z","victim":{"ship_type_id":587,"items":[]},"attackers":[]});
            let responses = [
                json!({"20261027":2}),
                json!({"1":row}),
                json!({"1":"public_hash"}),
                json!({"20261027":2}),
                json!({"20261028":2}),
                json!({"1":{"killmail_id":1,"killmail_time":"2026-10-28T12:00:00Z","victim":{"ship_type_id":587,"items":[]},"attackers":[]}}),
                json!({"1":"hash_one","2":"hash_two"}),
            ];
            for body in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut raw = [0; 8192];
                assert!(stream.read(&mut raw).unwrap() > 0);
                let body = body.to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
            }
        });
        let mut source = Sources::new(
            Store::open_with_seed(&directory, None).unwrap(),
            Arc::new(AtomicBool::new(false)),
            crate::engine::Catalog::load().unwrap(),
        );
        source.archive_base = base;
        let date = NaiveDate::from_ymd_opt(2026, 10, 27).unwrap();
        assert_eq!(source.archive(date, date, &|_| {}).unwrap(), 1);
        assert_eq!(source.store.archive_count("20261027"), 2);
        assert_eq!(source.archive(date, date, &|_| {}).unwrap(), 0);
        let next = date + TimeDelta::days(1);
        assert!(source.archive(next, next, &|_| {}).is_err());
        assert_ne!(source.store.archive_count("20261028"), 2);
        server.join().unwrap();
        drop(source);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
