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
#[derive(Clone)]
pub struct Workers {
    pub compute: mpsc::Sender<AnalysisRequest>,
    pub network: mpsc::Sender<NetworkRequest>,
    pub ocr: mpsc::Sender<OcrConfig>,
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
        let (compute, compute_rx) = mpsc::channel::<AnalysisRequest>();
        let (network, network_rx) = mpsc::channel::<NetworkRequest>();
        let (ocr, ocr_rx) = mpsc::channel::<OcrConfig>();
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
                let mut config: Option<OcrConfig> = None;
                let mut frames = ocr::FrameCache::default();
                let mut card_frames = ocr::FrameCache::default();
                let mut selected_text = String::new();
                let mut card_document = ocr::Document::default();
                let mut cached = None;
                loop {
                    let interval = config
                        .as_ref()
                        .map(|c| c.settings.ocr_interval.clamp(0.5, 10.))
                        .unwrap_or(10.);
                    match ocr_rx.recv_timeout(Duration::from_secs_f64(interval)) {
                        Ok(mut value) => {
                            while let Ok(next) = ocr_rx.try_recv() {
                                value = next;
                            }
                            config = Some(value);
                            frames = ocr::FrameCache::default();
                            card_frames = ocr::FrameCache::default();
                            selected_text.clear();
                            card_document = ocr::Document::default();
                            continue;
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    let Some(config) = &config else {
                        continue;
                    };
                    if !config.enabled
                        || !config.settings.ocr
                        || config.settings.pause_outside_eve && !platform::eve_active()
                    {
                        continue;
                    }
                    let Some(rect) = config.settings.region else {
                        continue;
                    };
                    let mut should_emit = true;
                    let result = (|| -> Result<Option<Target>, String> {
                        let image = platform::read_screen(rect)?;
                        if let Some(cards_rect) = config.settings.targets_region {
                            let selected_decision = frames.inspect(image.as_raw());
                            if selected_decision == ocr::FrameDecision::Recognize {
                                selected_text =
                                    platform::ocr_image(&image, &config.settings.language)?;
                            }
                            // Карточки могут менять позиции независимо от правой панели.
                            let cards = platform::read_screen(cards_rect)?;
                            let card_decision = card_frames.inspect(cards.as_raw());
                            if card_decision == ocr::FrameDecision::Recognize {
                                card_document =
                                    platform::ocr_document(&cards, &config.settings.language)?;
                            }
                            if selected_decision == ocr::FrameDecision::Ignore
                                && card_decision == ocr::FrameDecision::Ignore
                            {
                                should_emit = false;
                                return Ok(None);
                            }
                            return Ok(ocr::selected_card(
                                &selected_text,
                                &card_document,
                                &catalog,
                            ));
                        }
                        match frames.inspect(image.as_raw()) {
                            ocr::FrameDecision::Ignore => {
                                should_emit = false;
                                return Ok(None);
                            }
                            ocr::FrameDecision::Confirm => return Ok(cached.clone()),
                            ocr::FrameDecision::Recognize => {}
                        }
                        let text = platform::ocr_image(&image, &config.settings.language)?;
                        cached = ocr::parse(&text, &catalog);
                        Ok(cached.clone())
                    })();
                    // An unchanged frame after confirmation causes neither OCR nor a UI repaint.
                    if !should_emit {
                        continue;
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
