//! Три постоянных потока: расчёт, сеть и OCR. UI получает только сообщения.

use crate::{
    engine::{Catalog, Engine},
    fits::{self, Population, Variant},
    model::Settings,
    ocr::{self, Target},
    platform,
    sources::Sources,
    store::Store,
    tactics::{self, Advice},
};
use chrono::Utc;
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[derive(Clone)]
pub struct Matchup {
    pub target: Target,
    pub pop: Population,
    pub variants: Vec<Variant>,
    pub advice: Vec<Option<Advice>>,
    pub results: Vec<Option<Value>>,
    pub own: Option<Value>,
    pub failures: Vec<String>,
}
pub enum Message {
    CharacterReady(Result<crate::account::Profile, String>),
    FitReady(String, Result<crate::fit_plan::Plan, String>),
    Analyzed(u64, Result<Matchup, String>),
    Status(String),
    PilotReady(String, String, Result<(), String>),
    SyncDone(Result<usize, String>),
    Ocr(u64, Result<Option<Target>, String>),
    OcrState(u64, String),
    ScreenCaptured,
    ScreenChecked(Box<ocr::ScreenCheck>),
    #[cfg(windows)]
    Tray(String),
}
#[derive(Clone)]
pub struct AnalysisRequest {
    pub generation: u64,
    pub settings: Settings,
    pub target: Target,
}
pub struct NetworkRequest {
    pub settings: Settings,
    pub target: Target,
    pub global: bool,
}
#[derive(Clone)]
pub struct OcrConfig {
    pub epoch: u64,
    pub settings: Settings,
    pub enabled: bool,
}
pub enum OcrRequest {
    Configure(OcrConfig),
    Check(Settings),
}
#[derive(Clone)]
pub struct Workers {
    pub compute: mpsc::Sender<AnalysisRequest>,
    pub network: mpsc::Sender<NetworkRequest>,
    pub ocr: mpsc::Sender<OcrRequest>,
    pub cancel: Arc<AtomicBool>,
}
fn emit(event_tx: &mpsc::Sender<Message>, ctx: &egui::Context, message: Message) {
    let _ = event_tx.send(message);
    ctx.request_repaint();
}

type CachedComparison = (Option<Advice>, Option<Value>, Option<String>);
struct AnalysisCore {
    store: Store,
    engine: Engine,
    catalog: Catalog,
    // Ключ включает свой фит/навыки и полный фит противника, но не текущую дистанцию.
    cache: HashMap<String, CachedComparison>,
}
impl AnalysisCore {
    fn analyze(&mut self, request: &AnalysisRequest) -> Result<Matchup, String> {
        let target = &request.target;
        let settings = &request.settings;
        let ship = self
            .catalog
            .ship_id(&target.ship)
            .ok_or("Неизвестный тип корабля")?;
        let revision = self
            .store
            .meta(&format!("global_revision:{ship}"))
            .unwrap_or(json!(0));
        let key = format!(
            "population:v2-sde3503375:{}",
            fits::hash(
                &json!([
                    ship,
                    revision,
                    settings.days,
                    settings.max_attackers,
                    settings.max_variants,
                    settings.coverage,
                    settings.min_percent,
                    settings.excluded,
                    Utc::now().date_naive()
                ])
                .to_string()
            )
        );
        let pop: Population = if let Some(value) = self.store.cached(&key) {
            serde_json::from_value(value).map_err(|e| e.to_string())?
        } else {
            let p = fits::population(&self.store.global_mails(ship)?, &self.catalog, settings);
            self.store.cache(
                &key,
                &serde_json::to_value(&p).map_err(|e| e.to_string())?,
                86400,
            )?;
            p
        };
        let mut variants = pop.variants.clone();
        if !target.pilot.is_empty()
            && let Some(character) = self
                .store
                .meta(&format!("character:{}", target.pilot.to_lowercase()))
                .and_then(|v| v.as_i64())
            && let Some(personal) = fits::pilot_variant(
                &self.store.pilot_mails(ship, character)?,
                character,
                ship,
                &self.catalog,
                settings,
            )
        {
            variants.insert(0, personal);
        }
        let own = if settings.own_fit.trim().is_empty() {
            None
        } else {
            let req = settings.own_request(json!(settings.own_fit), true);
            Some(self.engine.evaluate(req)?)
        };
        let mut out = Matchup {
            target: target.clone(),
            pop,
            variants,
            advice: Vec::new(),
            results: Vec::new(),
            own,
            failures: Vec::new(),
        };
        let revision = fits::hash(
            &json!([
                settings.own_fit,
                settings.skill_level,
                settings.skills,
                settings.implants,
                settings.boosters,
                settings.booster_effects,
                (target.angular * 1000.).round()
            ])
            .to_string(),
        );
        for variant in &out.variants {
            let key = format!("{revision}:{}", fits::hash(&variant.fit.to_string()));
            if let Some((advice, result, error)) = self.cache.get(&key) {
                out.advice.push(advice.clone());
                out.results.push(result.clone());
                if let Some(e) = error {
                    out.failures.push(e.clone());
                }
                continue;
            }
            let result = self
                .engine
                .evaluate(json!({"fit":tactics::fill_charges(&variant.fit,&self.catalog)}));
            let (advice, result, error) = match result {
                Ok(result) => {
                    let advice = out.own.as_ref().map(|own| {
                        tactics::advise(
                            own,
                            &result,
                            &mut self.engine,
                            &self.catalog,
                            settings,
                            None,
                            target.angular,
                        )
                    });
                    match advice {
                        Some(Ok(a)) => (Some(a), Some(result), None),
                        Some(Err(e)) => (None, Some(result), Some(e)),
                        None => (None, Some(result), None),
                    }
                }
                Err(error) => (None, None, Some(error)),
            };
            if self.cache.len() >= 64 {
                self.cache.clear();
            }
            self.cache
                .insert(key, (advice.clone(), result.clone(), error.clone()));
            out.advice.push(advice);
            out.results.push(result);
            if let Some(e) = error {
                out.failures.push(e);
            }
        }
        Ok(out)
    }
}
impl Workers {
    pub fn start(
        directory: PathBuf,
        catalog: Catalog,
        event_tx: mpsc::Sender<Message>,
        ctx: egui::Context,
    ) -> Self {
        let debug_directory = directory.join("debug");
        let (compute, compute_rx) = mpsc::channel::<AnalysisRequest>();
        let (network, network_rx) = mpsc::channel::<NetworkRequest>();
        let (ocr, ocr_rx) = mpsc::channel::<OcrRequest>();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let event_tx = event_tx.clone();
            let ctx = ctx.clone();
            let dir = directory.clone();
            let catalog = catalog.clone();
            thread::spawn(move || {
                platform::low_priority();
                let store = match Store::open(&dir) {
                    Ok(s) => s,
                    Err(e) => {
                        emit(&event_tx, &ctx, Message::Status(e));
                        return;
                    }
                };
                let mut core = AnalysisCore {
                    store,
                    engine: Engine::default(),
                    catalog,
                    cache: HashMap::new(),
                };
                while let Ok(mut request) = compute_rx.recv() {
                    // Если игрок быстро меняет цели, считаем только последнюю ожидающую.
                    while let Ok(new) = compute_rx.try_recv() {
                        request = new;
                    }
                    let result = core.analyze(&request);
                    emit(
                        &event_tx,
                        &ctx,
                        Message::Analyzed(request.generation, result),
                    );
                }
            });
        }
        {
            let event_tx = event_tx.clone();
            let ctx = ctx.clone();
            let cancel = cancel.clone();
            let catalog = catalog.clone();
            thread::spawn(move || {
                platform::low_priority();
                while let Ok(mut request) = network_rx.recv() {
                    // Manual global jobs are never discarded by later pilot lookups.
                    if !request.global {
                        while let Ok(next) = network_rx.try_recv() {
                            request = next;
                            if request.global {
                                break;
                            }
                        }
                    }
                    let Some(ship) = catalog.ship_id(&request.target.ship) else {
                        continue;
                    };
                    let store = match Store::open(&directory) {
                        Ok(s) => s,
                        Err(e) => {
                            emit(&event_tx, &ctx, Message::Status(e));
                            continue;
                        }
                    };
                    if !request.global {
                        cancel.store(false, Ordering::Relaxed);
                    }
                    let mut source = Sources::new(store, cancel.clone());
                    let progress = |text| emit(&event_tx, &ctx, Message::Status(text));
                    if request.global {
                        let result = source.refresh_statistics(ship, &request.settings, &progress);
                        emit(&event_tx, &ctx, Message::SyncDone(result));
                    } else {
                        let result = source.sync_pilot(&request.target.pilot, ship, &progress);
                        emit(
                            &event_tx,
                            &ctx,
                            Message::PilotReady(request.target.ship, request.target.pilot, result),
                        );
                    }
                }
            });
        }
        {
            let event_tx = event_tx.clone();
            let ctx = ctx.clone();
            thread::spawn(move || {
                platform::low_priority();
                let mut debug = crate::diagnostics::DebugLog::new(debug_directory);
                let mut config: Option<OcrConfig> = None;
                let mut frames = ocr::FrameCache::default();
                let mut card_frames = ocr::FrameCache::default();
                let mut selected_document = ocr::Document::default();
                let mut card_document = ocr::Document::default();
                let mut cached = None;
                let mut last_state = String::new();
                loop {
                    let interval = config
                        .as_ref()
                        .map(|c| c.settings.ocr_interval.clamp(0.5, 10.))
                        .unwrap_or(10.);
                    match ocr_rx.recv_timeout(Duration::from_secs_f64(interval)) {
                        Ok(OcrRequest::Check(settings)) => {
                            // UI уже скрыт. Оба снимка делаются до его восстановления.
                            let started = std::time::Instant::now();
                            let mut check = ocr::ScreenCheck::capture(&settings);
                            emit(&event_tx, &ctx, Message::ScreenCaptured);
                            let selected_png = check.selected.image.as_ref().and_then(|image| {
                                debug.snapshot(settings.ocr_debug, "manual-selected", image)
                            });
                            let cards_png = check.cards.image.as_ref().and_then(|image| {
                                debug.snapshot(settings.ocr_debug, "manual-cards", image)
                            });
                            check.recognize(&settings.language, &catalog);
                            debug.event(settings.ocr_debug, json!({"kind":"manual_check", "region": settings.region, "targets_region": settings.targets_region, "language":settings.language,
                                "elapsed_ms": started.elapsed().as_millis(), "selected_png": selected_png, "cards_png": cards_png,
                                "selected_text":check.selected.text, "selected_words":check.selected.document, "selected_error":check.selected.error,
                                "cards_text":check.cards.text, "cards_words":check.cards.document, "cards_error":check.cards.error, "result":check.result}));
                            if let Some(error) = debug.take_error() {
                                emit(&event_tx, &ctx, Message::Status(error));
                            }
                            emit(&event_tx, &ctx, Message::ScreenChecked(Box::new(check)));
                            continue;
                        }
                        Ok(OcrRequest::Configure(value)) => {
                            // Не coalesce: иначе теряется ручная проверка между Configure.
                            debug.event(value.settings.ocr_debug || config.as_ref().is_some_and(|c| c.settings.ocr_debug), json!({"kind":"configure", "debug_enabled":value.settings.ocr_debug,
                                "epoch":value.epoch, "ocr_enabled":value.settings.ocr, "worker_enabled":value.enabled, "pause_outside_eve":value.settings.pause_outside_eve,
                                "region":value.settings.region, "targets_region":value.settings.targets_region, "language":value.settings.language, "interval":value.settings.ocr_interval}));
                            if let Some(error) = debug.take_error() {
                                emit(&event_tx, &ctx, Message::Status(error));
                            }
                            config = Some(value);
                            frames = ocr::FrameCache::default();
                            card_frames = ocr::FrameCache::default();
                            selected_document = ocr::Document::default();
                            card_document = ocr::Document::default();
                            last_state.clear();
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let Some(config) = &config else {
                        continue;
                    };
                    let state = if !config.settings.ocr {
                        "OCR выключен"
                    } else if !config.enabled {
                        "OCR приостановлен: открыты настройки"
                    } else if config.settings.region.is_none() {
                        "OCR: выделите правую область"
                    } else if config.settings.pause_outside_eve && !platform::eve_active() {
                        "OCR приостановлен: EVE неактивна"
                    } else {
                        "OCR работает"
                    };
                    if state != last_state {
                        debug.event(
                            config.settings.ocr_debug,
                            json!({"kind":"state", "epoch":config.epoch, "state":state}),
                        );
                        if let Some(error) = debug.take_error() {
                            emit(&event_tx, &ctx, Message::Status(error));
                        }
                        emit(
                            &event_tx,
                            &ctx,
                            Message::OcrState(config.epoch, state.into()),
                        );
                        last_state = state.into();
                    }
                    if state != "OCR работает" {
                        continue;
                    }
                    let Some(rect) = config.settings.region else {
                        continue;
                    };
                    let started = std::time::Instant::now();
                    let mut frame_event = json!({"kind":"automatic_frame", "epoch":config.epoch, "region":config.settings.region, "targets_region":config.settings.targets_region, "language":config.settings.language});
                    let mut should_emit = true;
                    let result = (|| -> Result<Option<Target>, String> {
                        let image = platform::read_screen(rect)?;
                        if let Some(cards_rect) = config.settings.targets_region {
                            let selected_decision = frames.inspect(image.as_raw());
                            frame_event["selected_frame"] = json!(format!("{selected_decision:?}"));
                            if selected_decision == ocr::FrameDecision::Recognize {
                                frame_event["selected_png"] = json!(debug.snapshot(
                                    config.settings.ocr_debug,
                                    "selected",
                                    &image
                                ));
                                selected_document =
                                    platform::ocr_document(&image, &config.settings.language)?;
                            }
                            // Карточки могут менять позиции независимо от правой панели.
                            let cards = platform::read_screen(cards_rect)?;
                            let card_decision = card_frames.inspect(cards.as_raw());
                            frame_event["cards_frame"] = json!(format!("{card_decision:?}"));
                            if card_decision == ocr::FrameDecision::Recognize {
                                frame_event["cards_png"] = json!(debug.snapshot(
                                    config.settings.ocr_debug,
                                    "cards",
                                    &cards
                                ));
                                card_document =
                                    platform::ocr_document(&cards, &config.settings.language)?;
                            }
                            if selected_decision == ocr::FrameDecision::Ignore
                                && card_decision == ocr::FrameDecision::Ignore
                            {
                                should_emit = false;
                                return Ok(None);
                            }
                            return ocr::identify_card(
                                &selected_document.text(),
                                &card_document,
                                &catalog,
                            )
                            .map(Some);
                        }
                        let decision = frames.inspect(image.as_raw());
                        frame_event["selected_frame"] = json!(format!("{decision:?}"));
                        match decision {
                            ocr::FrameDecision::Ignore => {
                                should_emit = false;
                                return Ok(None);
                            }
                            ocr::FrameDecision::Confirm => return Ok(cached.clone()),
                            ocr::FrameDecision::Recognize => {}
                        }
                        frame_event["selected_png"] =
                            json!(debug.snapshot(config.settings.ocr_debug, "selected", &image));
                        selected_document =
                            platform::ocr_document(&image, &config.settings.language)?;
                        cached = ocr::parse(&selected_document.text(), &catalog);
                        Ok(cached.clone())
                    })();
                    if config.settings.ocr_debug {
                        frame_event["elapsed_ms"] = json!(started.elapsed().as_millis());
                        frame_event["selected_text"] = json!(selected_document.text());
                        frame_event["selected_words"] = json!(selected_document);
                        frame_event["cards_text"] = json!(card_document.text());
                        frame_event["cards_words"] = json!(card_document);
                        frame_event["selected_pilot"] =
                            json!(ocr::selected_pilot(&selected_document.text()));
                        frame_event["result"] = json!(result);
                        frame_event["emitted"] = json!(should_emit);
                        debug.event(true, frame_event);
                        if let Some(error) = debug.take_error() {
                            emit(&event_tx, &ctx, Message::Status(error));
                        }
                    }
                    // An unchanged frame after confirmation causes neither OCR nor a UI repaint.
                    if !should_emit {
                        continue;
                    }
                    if result.is_err() {
                        // Ошибка не должна закрепляться в кеше неизменившегося кадра.
                        frames = ocr::FrameCache::default();
                        card_frames = ocr::FrameCache::default();
                    }
                    emit(&event_tx, &ctx, Message::Ocr(config.epoch, result));
                }
            });
        }
        Self {
            compute,
            network,
            ocr,
            cancel,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distance_change_reuses_population_and_dogma_results() {
        let dir = std::env::temp_dir().join(format!(
            "eve-combat-warm-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let mut store = Store::open(&dir).unwrap();
        let sample =
            crate::store::decompress(include_bytes!("../resources/sample-rifter.json.gz")).unwrap();
        store.ingest_global(crate::model::array(&sample)).unwrap();
        let settings = Settings {
            own_fit: include_str!("../resources/example-tristan.eft").into(),
            ..Default::default()
        };
        let mut core = AnalysisCore {
            store,
            engine: Engine::default(),
            catalog: Catalog::load().unwrap(),
            cache: HashMap::new(),
        };
        let mut request = AnalysisRequest {
            generation: 1,
            settings,
            target: Target {
                ship: "Rifter".into(),
                pilot: String::new(),
                distance: None,
                angular: 0.,
            },
        };
        let cold = std::time::Instant::now();
        let first = core.analyze(&request).unwrap();
        let cold = cold.elapsed();
        let calculations = core.engine.calculations;
        assert!(calculations > 0);
        request.target.distance = Some(23000.);
        request.generation = 2;
        let warm = std::time::Instant::now();
        let second = core.analyze(&request).unwrap();
        let warm = warm.elapsed();
        assert_eq!(calculations, core.engine.calculations);
        assert_eq!(first.pop.total, second.pop.total);
        assert_eq!(second.target.distance, Some(23000.));
        assert!(second.advice.iter().any(Option::is_some));
        eprintln!("cold={cold:?}; warm={warm:?}; dogma calls reused={calculations}");
        drop(core);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod ocr_tests {
    use super::*;

    #[test]
    fn manual_requests_survive_configuration_and_automatic_errors_retry() {
        let directory = std::env::temp_dir().join(format!(
            "eve-ocr-worker-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let (tx, rx) = mpsc::channel();
        let workers = Workers::start(
            directory.clone(),
            Catalog::load().unwrap(),
            tx,
            egui::Context::default(),
        );
        let draft = Settings {
            own_fit: "invalid fit must not prevent checking the screen".into(),
            region: Some([0, 0, 0, 0]),
            ocr_debug: true,
            ..Default::default()
        };
        for epoch in 1..=2 {
            workers
                .ocr
                .send(OcrRequest::Configure(OcrConfig {
                    epoch,
                    settings: draft.clone(),
                    enabled: false,
                }))
                .unwrap();
            workers.ocr.send(OcrRequest::Check(draft.clone())).unwrap();
        }
        let mut automatic = draft.clone();
        automatic.ocr = true;
        automatic.ocr_debug = false;
        automatic.pause_outside_eve = false;
        automatic.ocr_interval = 0.5;
        workers
            .ocr
            .send(OcrRequest::Configure(OcrConfig {
                epoch: 3,
                settings: automatic,
                enabled: true,
            }))
            .unwrap();
        let (mut captured, mut checked, mut failures) = (0, 0, 0);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while (checked < 2 || failures < 2) && std::time::Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(Message::ScreenCaptured) => captured += 1,
                Ok(Message::ScreenChecked(check)) => {
                    assert!(check.result.is_err());
                    checked += 1;
                }
                Ok(Message::Ocr(3, Err(_))) => failures += 1,
                _ => {}
            }
        }
        assert_eq!((captured, checked), (2, 2));
        assert!(
            failures >= 2,
            "automatic OCR must retry after capture failure"
        );
        let records = std::fs::read_to_string(directory.join("debug/ocr.jsonl")).unwrap();
        let manual: Vec<Value> = records
            .lines()
            .map(|s| serde_json::from_str::<Value>(s).unwrap())
            .filter(|v| v["event"]["kind"] == "manual_check")
            .collect();
        assert_eq!(manual.len(), 2);
        assert!(manual[0]["event"]["selected_error"].is_string());
        assert!(!records.contains("invalid fit must not prevent"));
        drop(workers);
        // Compute owns SQLite briefly during shutdown; a later run may reuse it.
    }
}
