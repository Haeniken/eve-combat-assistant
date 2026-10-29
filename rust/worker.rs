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
    pub seed: Option<crate::seed::SeedInfo>,
    pub seed_error: Option<String>,
}
pub enum Message {
    CharacterReady(Result<crate::account::Profile, String>),
    OwnShipReady(i64, Result<Option<crate::account::Profile>, String>),
    OwnShipChanging(i64),
    FitReady(String, Result<crate::fit_plan::Plan, String>),
    Analyzed(u64, AnalysisPhase, Result<Box<Matchup>, String>),
    Status(String),
    PilotReady(u64, String, String, Result<(), String>),
    PilotStatus(u64, String, String, String),
    PilotRating(u64, String, Result<crate::pilot::Rating, String>),
    OwnPilotRating(i64, Result<crate::pilot::Rating, String>),
    SyncDone(Result<usize, String>),
    SyncProgress(crate::sources::UpdateProgress),
    Ocr(u64, Result<Option<Target>, String>, Option<String>),
    OcrState(u64, String),
    ScreenCaptured,
    DesktopCaptured(Result<platform::DesktopCapture, String>),
    ScreenChecked(Box<ocr::ScreenCheck>),
    #[cfg(windows)]
    Tray(String),
}
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub enum AnalysisPhase {
    Local,
    CachedPilot,
    NetworkPilot,
}
#[derive(Clone)]
pub struct AnalysisRequest {
    pub generation: u64,
    pub phase: AnalysisPhase,
    pub parent_generation: Option<u64>,
    pub queued_at: std::time::Instant,
    pub settings: Settings,
    pub target: Target,
}
pub struct NetworkRequest {
    pub generation: u64,
    pub queued_at: std::time::Instant,
    pub settings: Settings,
    pub target: Target,
    pub global: bool,
    pub own_character: Option<i64>,
}
fn next_network_request(
    rx: &mpsc::Receiver<NetworkRequest>,
    pending: &mut Option<NetworkRequest>,
) -> Option<NetworkRequest> {
    let mut request = pending.take().or_else(|| rx.recv().ok())?;
    if !request.global && request.own_character.is_none() {
        while let Ok(next) = rx.try_recv() {
            // Keep the last target; own-profile jobs must neither consume nor be consumed by it.
            if next.own_character.is_some() {
                *pending = Some(next);
                break;
            }
            request = next;
            if request.global {
                break;
            }
        }
    }
    Some(request)
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
    CaptureDesktop,
}
#[derive(Clone)]
pub struct Workers {
    pub trace: crate::diagnostics::Trace,
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
    trace: crate::diagnostics::Trace,
}
impl AnalysisCore {
    fn analyze(&mut self, request: &AnalysisRequest) -> Result<Matchup, String> {
        let trace=self.trace.job(request.settings.ocr_debug,json!({"correlation":format!("{}-{}",std::process::id(),request.generation),"generation":request.generation,"parent_generation":request.parent_generation,"job":"analysis","phase":request.phase,"target_ship":request.target.ship,"pilot":request.target.pilot}));
        self.store.set_trace(trace.clone());
        let started = std::time::Instant::now();
        trace.event("analysis_start",json!({"queue_ms":request.queued_at.elapsed().as_millis(),"filters":{"days":request.settings.days,"max_attackers":request.settings.max_attackers,"max_variants":request.settings.max_variants,"coverage":request.settings.coverage,"min_percent":request.settings.min_percent,"excluded":request.settings.excluded.len()},"bundled":self.store.seed,"seed_error":self.store.seed_error}));
        let result = self.analyze_inner(request, &trace);
        trace.event("analysis_end",json!({"elapsed_ms":started.elapsed().as_millis(),"error":result.as_ref().err(),"variants":result.as_ref().ok().map(|m|m.variants.len()),"failures":result.as_ref().ok().map(|m|&m.failures)}));
        result
    }
    fn analyze_inner(
        &mut self,
        request: &AnalysisRequest,
        trace: &crate::diagnostics::Trace,
    ) -> Result<Matchup, String> {
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
            "population:v3-sde3503375:{}",
            fits::hash(
                &json!([
                    ship,
                    revision,
                    self.store.seed.as_ref().map(|s| &s.id),
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
        let mut cache_source = "user_cache";
        let pop: Population = if let Some(value) = self.store.cached(&key) {
            serde_json::from_value(value).map_err(|e| e.to_string())?
        } else {
            let seed_key = fits::seed_population_key(ship, settings);
            let prepared = if self.store.local_global_count(ship)? == 0 {
                self.store.seed_population(&seed_key)
            } else {
                None
            };
            let p = if let Some(value) = prepared {
                cache_source = "bundled_prepared";
                serde_json::from_value(value).map_err(|e| e.to_string())?
            } else {
                cache_source = "scanned_local";
                fits::population(&self.store.global_mails(ship)?, &self.catalog, settings)
            };
            if let Err(error) = self.store.cache_nonblocking(
                &key,
                &serde_json::to_value(&p).map_err(|e| e.to_string())?,
                86400,
            ) {
                trace.event("population_cache_write_failed", json!({"error":error}));
            }
            p
        };
        trace.event("popular_ready",json!({"ship_id":ship,"cache":cache_source,"report":pop.report,"variants":pop.variants.iter().map(|v|json!({"key":v.key,"count":v.count,"percent":v.percent})).collect::<Vec<_>>() }));
        let mut variants = pop.variants.clone();
        if request.phase != AnalysisPhase::Local && !target.pilot.is_empty() {
            let character = self
                .store
                .meta(&format!("character:{}", target.pilot.to_lowercase()))
                .and_then(|v| v.as_i64());
            if let Some(character) = character {
                let selection = fits::pilot_selection(
                    &self.store.pilot_mails(ship, character)?,
                    character,
                    ship,
                    &self.catalog,
                    settings,
                );
                trace.event("personal_cache_ready",json!({"character_id":character,"report":selection.report,"found":selection.variant.is_some(),"period_days":settings.pilot_days,"policy":"latest_pvp_loss_independent_of_global_fleet_filters"}));
                if let Some(personal) = selection.variant {
                    variants.insert(0, personal);
                }
            } else {
                trace.event(
                    "personal_cache_unresolved",
                    json!({"reason":"no_character_id_for_name"}),
                );
            }
        }
        let mut failures = Vec::new();
        let own = if settings.own_fit.trim().is_empty() {
            None
        } else {
            match self
                .engine
                .evaluate(settings.own_request(json!(settings.own_fit), true))
            {
                Ok(mut own) => {
                    // Never present the default all-V profile as the player's skills.
                    if settings
                        .skills
                        .as_ref()
                        .and_then(Value::as_object)
                        .is_some_and(|s| !s.is_empty())
                    {
                        match crate::fit_metrics::fitted_speed(
                            &own,
                            &mut self.engine,
                            &self.catalog,
                            Some(settings),
                        ) {
                            Ok(speed) => own["fitted_speed"] = speed,
                            Err(error) => {
                                trace.event("own_fitted_speed_error", json!({"error":error}))
                            }
                        }
                    }
                    Some(own)
                }
                Err(error) => {
                    trace.event("own_fit_error", json!({"error":error}));
                    failures.push(format!("Мой фит: {error}"));
                    None
                }
            }
        };
        let mut out = Matchup {
            target: target.clone(),
            pop,
            variants,
            advice: Vec::new(),
            results: Vec::new(),
            own,
            failures,
            seed: self.store.seed.clone(),
            seed_error: self.store.seed_error.clone(),
        };
        if let Some(own) = &out.own {
            trace.event(
                "own_weapons_ready",
                tactics::own_weapon_summary(own, &self.catalog),
            );
        }
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
                trace.event(
                    "comparison_cache_hit",
                    json!({"variant_key":variant.key,"personal":variant.personal}),
                );
                out.advice.push(advice.clone());
                out.results.push(result.clone());
                if let Some(e) = error {
                    out.failures.push(e.clone());
                }
                continue;
            }
            let comparison_started = std::time::Instant::now();
            let result = self
                .engine
                .evaluate(json!({"fit":tactics::fill_charges(&variant.fit,&self.catalog)}));
            let (advice, result, error) = match result {
                Ok(mut result) => {
                    let ship_id = result["fit"]["ship_type_id"].clone();
                    if let Ok(bare)=self.engine.evaluate(json!({"fit":{"ship_type_id":ship_id,"modules":[],"drones":[]},"skill_level":5})) {
                        result["base_defense"]=bare["summary"]["defense"].clone();
                    }
                    result["incoming"] =
                        tactics::incoming_summary(&result, &variant.fit, &self.catalog);
                    result["weapon_range"] =
                        crate::fit_metrics::weapon_range(&result, &mut self.engine, &self.catalog);
                    result["neutralizers"] = crate::fit_metrics::neutralizers(&result);
                    match crate::fit_metrics::fitted_speed(
                        &result,
                        &mut self.engine,
                        &self.catalog,
                        None,
                    ) {
                        Ok(speed) => result["fitted_speed"] = speed,
                        Err(error) => {
                            trace.event(
                                "fitted_speed_error",
                                json!({"variant_key":variant.key,"error":error}),
                            );
                        }
                    }
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
                        Some(Err(error)) => {
                            result["comparison_error"] = json!(error);
                            (None, Some(result), Some(error))
                        }
                        None => (None, Some(result), None),
                    }
                }
                Err(error) => (None, None, Some(error)),
            };
            trace.event("comparison_ready",json!({"variant_key":variant.key,"personal":variant.personal,"error":error,"elapsed_ms":comparison_started.elapsed().as_millis(),"hull_id":result.as_ref().map(|r|&r["hull"]["id"]),"defense":result.as_ref().map(|r|&r["summary"]["defense"]),"base_defense":result.as_ref().map(|r|&r["base_defense"]),"incoming":result.as_ref().map(|r|&r["incoming"]),"fitted_speed":result.as_ref().map(|r|&r["fitted_speed"]),"own_fitted_speed":out.own.as_ref().map(|r|&r["fitted_speed"]),"weapon_range":result.as_ref().map(|r|&r["weapon_range"]),"neutralizers":result.as_ref().map(|r|&r["neutralizers"]),"warnings":result.as_ref().map(|r|&r["warnings"]),"approximations":result.as_ref().map(|r|&r["approximations"]),"own_approximations":out.own.as_ref().map(|r|&r["approximations"]),"advice":advice}));
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
        let fit_trace = crate::diagnostics::Trace::new(debug_directory.clone());
        let (compute, compute_rx) = mpsc::channel::<AnalysisRequest>();
        let (network, network_rx) = mpsc::channel::<NetworkRequest>();
        let (ocr, ocr_rx) = mpsc::channel::<OcrRequest>();
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let event_tx = event_tx.clone();
            let ctx = ctx.clone();
            let dir = directory.clone();
            let fit_trace = fit_trace.clone();
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
                    trace: fit_trace,
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
                        Message::Analyzed(request.generation, request.phase, result.map(Box::new)),
                    );
                    if request.settings.pilot_rating
                        && let Some(rating) =
                            crate::pilot::cached(&core.store, &request.target.pilot)
                    {
                        emit(
                            &event_tx,
                            &ctx,
                            Message::PilotRating(
                                request.generation,
                                request.target.pilot.clone(),
                                Ok(rating),
                            ),
                        );
                    }
                    if request.phase == AnalysisPhase::Local
                        && !request.target.pilot.is_empty()
                        && core
                            .store
                            .meta(&format!(
                                "character:{}",
                                request.target.pilot.to_lowercase()
                            ))
                            .is_some()
                    {
                        let mut cached = request.clone();
                        cached.phase = AnalysisPhase::CachedPilot;
                        let result = core.analyze(&cached);
                        if result
                            .as_ref()
                            .is_ok_and(|m| m.variants.iter().any(|v| v.personal))
                        {
                            emit(
                                &event_tx,
                                &ctx,
                                Message::Analyzed(
                                    cached.generation,
                                    cached.phase,
                                    result.map(Box::new),
                                ),
                            );
                        }
                    }
                    if let Some(error) = core.trace.take_error() {
                        emit(&event_tx, &ctx, Message::Status(error));
                    }
                }
            });
        }
        {
            let event_tx = event_tx.clone();
            let ctx = ctx.clone();
            let cancel = cancel.clone();
            let fit_trace = fit_trace.clone();
            let catalog = catalog.clone();
            thread::spawn(move || {
                platform::low_priority();
                let mut pending = None;
                while let Some(request) = next_network_request(&network_rx, &mut pending) {
                    let ship = catalog.ship_id(&request.target.ship);
                    if !request.global && request.own_character.is_none() && ship.is_none() {
                        continue;
                    }
                    let trace=fit_trace.job(request.settings.ocr_debug,json!({"correlation":format!("{}-{}",std::process::id(),request.generation),"generation":request.generation,"job":if request.global{"all_hull_update"}else if request.own_character.is_some(){"own_pilot_rating"}else{"pilot_lookup"},"job_id":crate::diagnostics::trace_id("network"),"target_ship":request.target.ship,"pilot":request.target.pilot}));
                    trace.event(
                        "network_start",
                        json!({"queue_ms":request.queued_at.elapsed().as_millis()}),
                    );
                    let store = match Store::open(&directory) {
                        Ok(s) => s,
                        Err(e) => {
                            if let Some(character) = request.own_character {
                                emit(&event_tx, &ctx, Message::OwnPilotRating(character, Err(e)));
                            } else {
                                emit(&event_tx, &ctx, Message::Status(e));
                            }
                            continue;
                        }
                    };
                    if !request.global {
                        cancel.store(false, Ordering::Relaxed);
                    }
                    let job_cancel = if request.global {
                        cancel.clone()
                    } else {
                        Arc::new(AtomicBool::new(false))
                    };
                    let mut source = Sources::new(store, job_cancel, catalog.clone());
                    source.set_trace(trace.clone());
                    if let Some(character) = request.own_character {
                        let result = source.character_rating(character);
                        trace.event("own_pilot_rating_end", json!({"character_id":character,"rating":result.as_ref().ok(),"error":result.as_ref().err()}));
                        emit(&event_tx, &ctx, Message::OwnPilotRating(character, result));
                        continue;
                    }
                    let progress = |text| {
                        if request.global {
                            emit(&event_tx, &ctx, Message::Status(text))
                        } else {
                            emit(
                                &event_tx,
                                &ctx,
                                Message::PilotStatus(
                                    request.generation,
                                    request.target.ship.clone(),
                                    request.target.pilot.clone(),
                                    text,
                                ),
                            )
                        }
                    };
                    if request.global {
                        let update_tx = event_tx.clone();
                        let update_ctx = ctx.clone();
                        source.set_update_progress(move |progress| {
                            emit(&update_tx, &update_ctx, Message::SyncProgress(progress))
                        });
                        let result = source.refresh_statistics(&request.settings, &progress);
                        source.finish_update(result.as_ref().err().cloned());
                        trace.event(
                            "network_end",
                            json!({"result":result.as_ref().ok(),"error":result.as_ref().err()}),
                        );
                        emit(&event_tx, &ctx, Message::SyncDone(result));
                    } else {
                        let started = std::time::Instant::now();
                        if request.settings.pilot_rating {
                            let rating = source.pilot_rating(&request.target.pilot);
                            trace.event("pilot_rating_end",json!({"error":rating.as_ref().err(),"rating":rating.as_ref().ok()}));
                            emit(
                                &event_tx,
                                &ctx,
                                Message::PilotRating(
                                    request.generation,
                                    request.target.pilot.clone(),
                                    rating,
                                ),
                            );
                        }
                        if !request.settings.auto_pilot_lookup {
                            continue;
                        }
                        let result =
                            source.sync_pilot(&request.target.pilot, ship.unwrap(), &progress);
                        trace.event("network_end",json!({"error":result.as_ref().err(),"elapsed_ms":started.elapsed().as_millis()}));
                        emit(
                            &event_tx,
                            &ctx,
                            Message::PilotReady(
                                request.generation,
                                request.target.ship,
                                request.target.pilot,
                                result,
                            ),
                        );
                    }
                    if let Some(error) = trace.take_error() {
                        emit(&event_tx, &ctx, Message::Status(error));
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
                let mut selected_retries = Vec::new();
                let mut cached = None;
                let mut last_state = String::new();
                loop {
                    let interval = config
                        .as_ref()
                        .map(|c| c.settings.ocr_interval.clamp(0.5, 10.))
                        .unwrap_or(10.);
                    match ocr_rx.recv_timeout(Duration::from_secs_f64(interval)) {
                        Ok(OcrRequest::CaptureDesktop) => {
                            let capture = platform::capture_desktop();
                            debug.event(config.as_ref().is_some_and(|c|c.settings.ocr_debug), json!({"kind":"selector_capture", "rect":capture.as_ref().ok().map(|c|c.rect), "error":capture.as_ref().err(), "background":"in_memory_desktop_snapshot"}));
                            emit(&event_tx, &ctx, Message::DesktopCaptured(capture));
                            continue;
                        }
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
                                "selected_text":check.selected.text, "selected_parsed":check.selected.parsed_text, "selected_words":check.selected.document, "selected_retries":check.selected.retries, "pilot_match":check.result.as_ref().ok().map(|target|ocr::pilot_match(&check.selected.parsed_text,&check.selected.retries,target)), "selected_error":check.selected.error,
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
                            selected_retries.clear();
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
                    let mut observed_pilot = None;
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
                                selected_retries.clear();
                                selected_document =
                                    platform::ocr_document(&image, &config.settings.language)?;
                            }
                            observed_pilot = ocr::selected_pilot(&ocr::selected_text(
                                &selected_document,
                                &selected_retries,
                            ));
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
                            return ocr::identify_images(
                                &image,
                                &selected_document,
                                &cards,
                                &card_document,
                                &config.settings.language,
                                &catalog,
                                &mut selected_retries,
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
                        observed_pilot = ocr::selected_pilot(&ocr::selected_text(
                            &selected_document,
                            &selected_retries,
                        ));
                        cached = ocr::parse(&selected_document.text(), &catalog);
                        Ok(cached.clone())
                    })();
                    if config.settings.ocr_debug {
                        frame_event["elapsed_ms"] = json!(started.elapsed().as_millis());
                        frame_event["selected_text"] = json!(selected_document.text());
                        frame_event["selected_words"] = json!(selected_document);
                        frame_event["selected_parsed"] =
                            json!(ocr::selected_text(&selected_document, &selected_retries));
                        frame_event["selected_retries"] = json!(selected_retries);
                        frame_event["pilot_match"] =
                            json!(result.as_ref().ok().and_then(|target| target.as_ref()).map(
                                |target| ocr::pilot_match(
                                    &selected_document.text(),
                                    &selected_retries,
                                    target
                                )
                            ));
                        frame_event["cards_text"] = json!(card_document.text());
                        frame_event["cards_words"] = json!(card_document);
                        frame_event["selected_pilot"] = json!(ocr::selected_pilot(
                            &ocr::selected_text(&selected_document, &selected_retries)
                        ));
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
                    emit(
                        &event_tx,
                        &ctx,
                        Message::Ocr(config.epoch, result, observed_pilot),
                    );
                }
            });
        }
        Self {
            trace: fit_trace,
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
    fn own_rating_preserves_latest_target_and_manual_update_jobs() {
        let (tx, rx) = mpsc::channel();
        for (generation, global, own_character) in [
            (1, false, None),
            (2, false, None),
            (3, false, Some(123)),
            (4, true, None),
            (5, false, None),
        ] {
            tx.send(NetworkRequest {
                generation,
                global,
                own_character,
                queued_at: std::time::Instant::now(),
                settings: Settings::default(),
                target: Target {
                    ship: "Rifter".into(),
                    pilot: String::new(),
                    distance: None,
                    angular: 0.,
                },
            })
            .unwrap();
        }
        drop(tx);
        let mut pending = None;
        for expected in [2, 3, 4, 5] {
            assert_eq!(
                next_network_request(&rx, &mut pending).unwrap().generation,
                expected
            );
        }
        assert!(next_network_request(&rx, &mut pending).is_none());
    }
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
            trace: crate::diagnostics::Trace::new(dir.join("debug")),
        };
        let mut request = AnalysisRequest {
            generation: 1,
            phase: AnalysisPhase::Local,
            parent_generation: None,
            queued_at: std::time::Instant::now(),
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
                Ok(Message::Ocr(3, Err(_), _)) => failures += 1,
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

#[cfg(test)]
mod local_first_tests {
    use super::*;
    #[test]
    fn local_variants_are_emitted_before_cached_personal_fit_without_network() {
        let directory = std::env::temp_dir().join(crate::diagnostics::trace_id("eve-local-first"));
        let sample =
            crate::store::decompress(include_bytes!("../resources/sample-rifter.json.gz")).unwrap();
        let mut mail = crate::model::array(&sample)[0].clone();
        mail["killmail_time"] = json!(Utc::now().to_rfc3339());
        let pilot = crate::model::id(&mail["victim"], "character_id");
        assert!(pilot > 0);
        let mut store = Store::open_with_seed(&directory, None).unwrap();
        store.ingest_global(&[mail]).unwrap();
        store
            .set_meta("character:test pilot", &json!(pilot))
            .unwrap();
        drop(store);
        let (tx, rx) = mpsc::channel();
        let workers = Workers::start(
            directory.clone(),
            Catalog::load().unwrap(),
            tx,
            egui::Context::default(),
        );
        workers
            .compute
            .send(AnalysisRequest {
                generation: 1,
                phase: AnalysisPhase::Local,
                parent_generation: None,
                queued_at: std::time::Instant::now(),
                target: Target {
                    ship: "Rifter".into(),
                    pilot: "Test Pilot".into(),
                    distance: None,
                    angular: 0.,
                },
                settings: Settings {
                    max_attackers: 5,
                    ocr_debug: true,
                    own_fit: "invalid EFT".into(),
                    ..Default::default()
                },
            })
            .unwrap();
        let mut phases = Vec::new();
        while phases.len() < 2 {
            if let Message::Analyzed(1, phase, result) =
                rx.recv_timeout(Duration::from_secs(30)).unwrap()
            {
                let result = result.unwrap();
                assert!(!result.pop.variants.is_empty());
                assert!(!result.failures.is_empty());
                assert_eq!(
                    result.variants.iter().any(|v| v.personal),
                    phase == AnalysisPhase::CachedPilot
                );
                phases.push(phase);
            }
        }
        assert_eq!(
            phases,
            vec![AnalysisPhase::Local, AnalysisPhase::CachedPilot]
        );
        let log = std::fs::read_to_string(directory.join("debug/fits.jsonl")).unwrap();
        assert!(!log.contains("http_request"));
        assert!(log.contains("popular_ready"));
        drop(workers);
    }
}

/// Headless release verification: same local pipeline as the overlay, no network worker.
pub fn diagnose(
    directory: &std::path::Path,
    ship: &str,
    seed: Option<&std::path::Path>,
    settings: Settings,
) -> Result<Value, String> {
    let store = if let Some(seed) = seed {
        Store::open_with_seed(directory, Some(seed))?
    } else {
        Store::open(directory)?
    };
    let mut core = AnalysisCore {
        store,
        engine: Engine::default(),
        catalog: Catalog::load()?,
        cache: HashMap::new(),
        trace: crate::diagnostics::Trace::new(directory.join("debug")),
    };
    let request = AnalysisRequest {
        generation: 1,
        phase: AnalysisPhase::Local,
        parent_generation: None,
        queued_at: std::time::Instant::now(),
        target: Target {
            ship: ship.into(),
            pilot: settings.pilot.clone(),
            distance: None,
            angular: 0.,
        },
        settings,
    };
    let started = std::time::Instant::now();
    let m = core.analyze(&request)?;
    Ok(
        json!({"version":env!("CARGO_PKG_VERSION"),"ship":ship,"seed":m.seed,"seed_error":m.seed_error,"report":m.pop.report,"variants":m.variants.iter().enumerate().map(|(i,v)|{let r=m.results.get(i).and_then(Option::as_ref);json!({"title":v.title,"percent":v.percent,"personal":v.personal,"defense":r.map(|r|&r["summary"]["defense"]),"base_defense":r.map(|r|&r["base_defense"]),"incoming":r.map(|r|&r["incoming"]),"fitted_speed":r.map(|r|&r["fitted_speed"]),"weapon_range":r.map(|r|&r["weapon_range"]),"neutralizers":r.map(|r|&r["neutralizers"]),"advice":m.advice.get(i).and_then(Option::as_ref),"comparison_error":r.map(|r|&r["comparison_error"]),"warnings":r.map(|r|&r["warnings"]),"approximations":r.map(|r|&r["approximations"])})}).collect::<Vec<_>>(),"failures":m.failures,"own_evaluated":m.own.is_some(),"own_fitted_speed":m.own.as_ref().map(|r|&r["fitted_speed"]),"advice_count":m.advice.iter().filter(|v|v.is_some()).count(),"elapsed_ms":started.elapsed().as_millis()}),
    )
}
