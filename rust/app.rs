//! Состояние приложения: выбранная цель, ответы потоков и окна интерфейса.

use crate::{
    Args,
    engine::{Catalog, Engine},
    fits,
    model::{Settings, array, id, n, s},
    ocr::{Gate, Target},
    platform,
    sources::Sources,
    store::Store,
    style::{self, CYAN, MUTED, TEXT},
    tactics,
    worker::{
        AnalysisPhase, AnalysisRequest, Matchup, Message, NetworkRequest, OcrConfig, OcrRequest,
        Workers,
    },
};
use eframe::egui::{self, Color32, RichText, Vec2, ViewportCommand, ViewportId};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Instant,
};

#[derive(Clone, Copy)]
enum CaptureKind {
    Check,
    Region,
}

#[derive(PartialEq)]
struct WindowLayout {
    on_top: bool,
    click_through: bool,
    scale: f32,
    width: f32,
    work_area: egui::Rect,
}

pub struct CombatApp {
    args: Args,
    app_icon: egui::TextureHandle,
    directory: PathBuf,
    settings: Settings,
    draft: Option<Settings>,
    catalog: Catalog,
    target: Option<Target>,
    matchup: Option<Matchup>,
    selected: usize,
    generation: u64,
    workers: Workers,
    rx: mpsc::Receiver<Message>,
    tx: mpsc::Sender<Message>,
    profile: crate::account::Profile,
    fit_state: crate::settings_ui::FitState,
    account_busy: bool,
    own_ship_busy: bool,
    next_ship_check: Instant,
    own_ship_error: Option<String>,
    fit_busy: bool,
    account_cancel: Arc<std::sync::atomic::AtomicBool>,
    status: String,
    pilot_status: String,
    pilot_error: Option<String>,
    pilot_generation: u64,
    pilot_rating: Option<crate::pilot::Rating>,
    rating_pilot: String,
    rating_error: Option<String>,
    own_rating: Option<crate::pilot::Rating>,
    own_rating_character: i64,
    own_rating_error: Option<String>,
    next_own_rating_check: Instant,
    tab: usize,
    syncing: bool,
    sync_progress: Option<crate::sources::UpdateProgress>,
    ocr_epoch: u64,
    ocr_status: String,
    screen_state: crate::settings_ui::ScreenState,
    capture_pending: Option<(Instant, CaptureKind)>,
    capturing: bool,
    restore_pending: bool,
    gate: Gate,
    details: Option<String>,
    selecting: bool,
    selecting_targets: bool,
    selection_start: Option<egui::Pos2>,
    selection_background: Option<([i32; 4], egui::TextureHandle)>,
    root_before_selection: Option<egui::Rect>,
    root_shot: bool,
    settings_frames: usize,
    settings_shot: bool,
    shots_done: Arc<AtomicUsize>,
    started: Instant,
    initialized: bool,
    system_info: Value,
    system_reported: bool,
    layout: Option<WindowLayout>,
    monitor_bounds: Box<dyn Fn() -> Option<egui::Rect>>,
    #[cfg(windows)]
    _tray: Option<crate::tray::Tray>,
}
impl CombatApp {
    pub fn new(cc: &eframe::CreationContext<'_>, args: Args, directory: PathBuf) -> Self {
        let catalog = Catalog::load().expect("bundled catalog");
        let mut settings = Settings::load(&directory);
        if args.demo {
            if let Ok(mut store) = Store::open(&directory)
                && let Ok(value) =
                    crate::store::decompress(include_bytes!("../resources/sample-rifter.json.gz"))
            {
                let _ = store.ingest_global(array(&value));
            }
            settings.own_fit = include_str!("../resources/example-tristan.eft").into();
            settings.ship = "Rifter".into();
            settings.pilot.clear();
            settings.ocr = false;
            settings.click_through = false;
            settings.auto_pilot_lookup = false;
        }
        let (tx, rx) = mpsc::channel();
        let workers = Workers::start(
            directory.clone(),
            catalog.clone(),
            tx.clone(),
            cc.egui_ctx.clone(),
        );
        let target = Some(Target {
            ship: settings.ship.clone(),
            pilot: settings.pilot.clone(),
            distance: None,
            angular: 0.,
        });
        let mut style = egui::Style {
            animation_time: 0.,
            visuals: egui::Visuals::dark(),
            ..Default::default()
        };
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.panel_fill = Color32::from_rgb(10, 20, 29);
        style.visuals.window_fill = Color32::from_rgb(11, 23, 33);
        style.visuals.widgets.active.bg_fill = Color32::from_rgb(32, 78, 96);
        style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(25, 57, 73);
        style.visuals.selection.bg_fill = Color32::from_rgb(37, 83, 101);
        style.spacing.item_spacing = Vec2::new(8., 7.);
        style.spacing.button_padding = Vec2::new(11., 7.);
        cc.egui_ctx.set_global_style(style);
        cc.egui_ctx.set_zoom_factor(settings.scale / 100.);
        #[cfg(windows)]
        let tray = crate::tray::Tray::new(tx.clone(), cc.egui_ctx.clone()).ok();
        #[cfg(windows)]
        if tray.is_none() {
            settings.click_through = false;
        }
        #[cfg(not(windows))]
        {
            settings.click_through = false;
        }
        let window = cc.winit_window().cloned();
        let monitor_bounds = Box::new(move || {
            let monitor = window.as_ref()?.current_monitor()?;
            let position = monitor.position();
            let size = monitor.size();
            let bounds = platform::monitor_work_area([
                position.x,
                position.y,
                position.x + size.width as i32,
                position.y + size.height as i32,
            ]);
            Some(egui::Rect::from_min_max(
                egui::pos2(bounds[0] as f32, bounds[1] as f32),
                egui::pos2(bounds[2] as f32, bounds[3] as f32),
            ))
        });
        let icon = image::load_from_memory(include_bytes!("../resources/icon.png"))
            .expect("bundled icon")
            .to_rgba8();
        let app_icon = cc.egui_ctx.load_texture(
            "app-icon",
            egui::ColorImage::from_rgba_unmultiplied(
                [icon.width() as usize, icon.height() as usize],
                icon.as_raw(),
            ),
            egui::TextureOptions::LINEAR,
        );
        Self {
            app_icon,
            tab: args.settings_tab.min(6),
            draft: args.open_settings.then(|| settings.clone()),
            profile: crate::account::Profile::load(&directory),
            fit_state: Default::default(),
            account_busy: false,
            own_ship_busy: false,
            next_ship_check: Instant::now(),
            own_ship_error: None,
            fit_busy: false,
            account_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tx,
            args,
            directory,
            settings,
            catalog,
            target,
            matchup: None,
            selected: 0,
            generation: 0,
            workers,
            rx,
            status: "Локальный анализ · zKillboard".into(),
            pilot_status: String::new(),
            pilot_error: None,
            pilot_generation: 0,
            pilot_rating: None,
            rating_pilot: String::new(),
            rating_error: None,
            own_rating: None,
            own_rating_character: 0,
            own_rating_error: None,
            next_own_rating_check: Instant::now(),
            syncing: false,
            sync_progress: None,
            ocr_epoch: 0,
            ocr_status: "OCR: ожидание".into(),
            screen_state: Default::default(),
            capture_pending: None,
            capturing: false,
            restore_pending: false,
            gate: Gate::default(),
            details: None,
            selecting: false,
            selecting_targets: false,
            selection_start: None,
            selection_background: None,
            root_before_selection: None,
            root_shot: false,
            settings_frames: 0,
            settings_shot: false,
            shots_done: Arc::new(AtomicUsize::new(0)),
            started: Instant::now(),
            initialized: false,
            system_info: crate::system_info::graphics(cc),
            system_reported: false,
            layout: None,
            monitor_bounds,
            #[cfg(windows)]
            _tray: tray,
        }
    }
    fn recalculate(&mut self) {
        self.request_analysis(AnalysisPhase::Local, None);
    }
    fn request_analysis(&mut self, phase: AnalysisPhase, parent_generation: Option<u64>) {
        if let Some(target) = &self.target {
            self.generation += 1;
            if phase == AnalysisPhase::Local {
                self.pilot_status.clear();
                self.pilot_error = None;
                self.pilot_generation = 0;
            }
            self.workers.trace.job(self.settings.ocr_debug,json!({"generation":self.generation,"parent_generation":parent_generation,"job":"ui","target_ship":target.ship,"pilot":target.pilot})).event("analysis_enqueued",json!({"phase":phase}));
            let _ = self.workers.compute.send(AnalysisRequest {
                generation: self.generation,
                phase,
                parent_generation,
                queued_at: Instant::now(),
                settings: self.settings.clone(),
                target: target.clone(),
            });
        }
    }
    fn request_pilot(&mut self) {
        if !(self.settings.auto_pilot_lookup || self.settings.pilot_rating) || self.args.demo {
            return;
        }
        if let Some(target) = self.target.clone()
            && !target.pilot.trim().is_empty()
        {
            self.pilot_generation = self.generation;
            self.pilot_error = None;
            if self.settings.auto_pilot_lookup {
                self.pilot_status = "Поиск фита пилота в фоне…".into();
            }
            self.workers.trace.job(self.settings.ocr_debug,json!({"generation":self.generation,"job":"ui","target_ship":target.ship,"pilot":target.pilot})).event("pilot_enqueued",json!({"after":"local_analysis_displayed"}));
            let _ = self.workers.network.send(NetworkRequest {
                generation: self.generation,
                queued_at: Instant::now(),
                settings: self.settings.clone(),
                target,
                global: false,
                own_character: None,
            });
        }
    }
    fn configure_ocr(&mut self) {
        self.ocr_epoch += 1;
        self.gate = Gate::default();
        if self.settings.ocr && self.draft.is_none() && !self.selecting {
            self.generation += 1;
            self.target = None;
            self.matchup = None;
        }
        let _ = self.workers.ocr.send(OcrRequest::Configure(OcrConfig {
            epoch: self.ocr_epoch,
            settings: self.settings.clone(),
            enabled: self.draft.is_none() && !self.selecting,
        }));
    }
    fn open_settings(&mut self) {
        if self.draft.is_none() {
            let mut draft = self.settings.clone();
            if let Some(target) = &self.target {
                draft.ship = target.ship.clone();
            }
            self.draft = Some(draft);
            self.configure_ocr();
        }
    }
    fn sync(&mut self, settings: Settings) {
        if self.syncing {
            return;
        }
        self.syncing = true;
        self.sync_progress = Some(crate::sources::UpdateProgress {
            phase: "Ожидание обновления".into(),
            ..Default::default()
        });
        self.workers.cancel.store(false, Ordering::Relaxed);
        let target = Target {
            ship: settings.ship.clone(),
            pilot: String::new(),
            distance: None,
            angular: 0.,
        };
        let _ = self.workers.network.send(NetworkRequest {
            generation: self.generation,
            queued_at: Instant::now(),
            settings,
            target,
            global: true,
            own_character: None,
        });
    }
    fn refresh_own_rating(&mut self, ctx: &egui::Context) {
        let character = self.profile.character_id;
        if character != self.own_rating_character {
            self.own_rating_character = character;
            self.own_rating = Store::open(&self.directory)
                .ok()
                .and_then(|store| crate::pilot::cached_character(&store, character));
            self.own_rating_error = None;
            self.next_own_rating_check = Instant::now();
        }
        if character <= 0 || !self.settings.pilot_rating || self.args.demo {
            return;
        }
        if Instant::now() >= self.next_own_rating_check {
            self.next_own_rating_check = Instant::now() + std::time::Duration::from_secs(3600);
            let result = self.workers.network.send(NetworkRequest {
                generation: self.generation,
                queued_at: Instant::now(),
                settings: self.settings.clone(),
                target: Target {
                    ship: self.settings.ship.clone(),
                    pilot: self.profile.name.clone(),
                    distance: None,
                    angular: 0.,
                },
                global: false,
                own_character: Some(character),
            });
            if result.is_err() {
                self.own_rating_error = Some("Поток сети недоступен".into());
            }
        }
        ctx.request_repaint_after(
            self.next_own_rating_check
                .saturating_duration_since(Instant::now()),
        );
    }
    fn ensure_system_report(&mut self, settings: &Settings) {
        if !self.system_reported {
            self.system_reported = true;
            self.workers.trace.system_report(json!({"runtime":self.system_info,"ocr_language":settings.language,"ui_scale_percent":settings.scale}));
        }
    }
    fn apply(&mut self, draft: Settings) -> Result<(), String> {
        if self.catalog.ship_id(&draft.ship).is_none() {
            return Err("Выбери известный тип корабля".into());
        }
        if draft.ocr && draft.region.is_none() {
            return Err("Выдели область экрана".into());
        }
        if !draft.own_fit.trim().is_empty() {
            let request = draft.own_request(json!(draft.own_fit), true);
            if let Err(error) = Engine::default().evaluate(request) {
                self.workers
                    .trace
                    .job(draft.ocr_debug, json!({"job":"own_fit_validation"}))
                    .event(
                        "own_fit_validation_error",
                        json!({"error":error,"fit_hash":fits::hash(&draft.own_fit)}),
                    );
                return Err(error);
            }
        }
        self.settings = draft;
        self.settings.save(&self.directory)?;
        self.draft = None;
        self.screen_state = Default::default();
        self.target = Some(Target {
            ship: self.settings.ship.clone(),
            pilot: self.settings.pilot.clone(),
            distance: None,
            angular: 0.,
        });
        self.matchup = None;
        self.configure_ocr();
        self.recalculate();
        Ok(())
    }
    fn clear_target_view(&mut self) {
        self.target = None;
        self.matchup = None;
        self.generation += 1;
        self.pilot_generation = 0;
        self.pilot_status.clear();
        self.pilot_error = None;
        self.pilot_rating = None;
        self.rating_pilot.clear();
        self.rating_error = None;
    }
    fn messages(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::OwnPilotRating(character, result) => {
                    if character == self.profile.character_id {
                        match result {
                            Ok(rating) => {
                                self.own_rating = Some(rating);
                                self.own_rating_error = None;
                            }
                            Err(error) => {
                                self.own_rating_error = Some(error);
                                self.next_own_rating_check =
                                    Instant::now() + std::time::Duration::from_secs(60);
                            }
                        }
                    }
                }
                Message::OwnShipChanging(character) => {
                    if character == self.profile.character_id {
                        self.settings.own_fit.clear();
                        if let Some(draft) = &mut self.draft {
                            draft.own_fit.clear();
                        }
                        if let Some(matchup) = &mut self.matchup {
                            matchup.own = None;
                            matchup.advice.fill(None);
                        }
                        self.own_ship_error = Some("Загружается фит нового корабля".into());
                        self.recalculate();
                    }
                }
                Message::OwnShipReady(character, result) => {
                    self.own_ship_busy = false;
                    if character != self.profile.character_id {
                        continue;
                    }
                    match result {
                        Ok(Some(profile)) => {
                            self.own_ship_error = None;
                            let _ = self.tx.send(Message::CharacterReady(Ok(profile)));
                        }
                        Ok(None) => self.own_ship_error = None,
                        Err(error) => {
                            self.workers
                                .trace
                                .job(self.settings.ocr_debug, json!({"job":"own_ship"}))
                                .event("own_ship_check_error", json!({"error":error}));
                            self.own_ship_error = Some(error);
                        }
                    }
                }
                Message::CharacterReady(result) => {
                    self.account_busy = false;
                    match result {
                        Ok(profile) => {
                            let settings = self.draft.as_mut().unwrap_or(&mut self.settings);
                            settings.skills = Some(json!(profile.skills));
                            settings.implants = profile.implants.clone();
                            profile.apply_current_fit(&self.profile, &mut settings.own_fit);
                            self.status = format!(
                                "{} · загружено навыков: {}",
                                profile.name,
                                profile.skills.len()
                            );
                            self.profile = profile;
                            self.own_ship_error = None;
                            self.next_ship_check =
                                Instant::now() + std::time::Duration::from_secs(60);
                            if self.draft.is_none() {
                                if let Err(error) = self.settings.save(&self.directory) {
                                    self.status = error;
                                }
                                self.recalculate();
                            }
                        }
                        Err(error) => self.status = error,
                    }
                }
                Message::FitReady(key, result) => {
                    self.fit_busy = false;
                    match result {
                        Ok(plan) => {
                            self.fit_state.plan = Some(plan);
                            self.fit_state.plan_key = key;
                        }
                        Err(error) => self.status = error,
                    }
                }
                Message::Analyzed(generation, phase, result) => {
                    if generation == self.generation {
                        match result {
                            Ok(m) => {
                                self.status = if self.args.demo {
                                    format!("Демонстрация · {} подходящих фитов", m.pop.total)
                                } else {
                                    format!(
                                        "{}: {} подходящих потерь из {} локальных",
                                        m.target.ship, m.pop.total, m.pop.report.input
                                    )
                                };
                                let personal = m.variants.iter().any(|v| v.personal);
                                self.workers.trace.job(self.settings.ocr_debug,json!({"generation":generation,"job":"ui","target_ship":m.target.ship,"pilot":m.target.pilot})).event("analysis_displayed",json!({"phase":phase,"popular_variants":m.pop.variants.len(),"personal":personal}));
                                self.matchup = Some(*m);
                                self.selected = 0;
                                if phase == AnalysisPhase::Local {
                                    self.request_pilot();
                                } else if personal {
                                    self.pilot_status = if self.pilot_error.is_some() {
                                        "Фит пилота из кеша; обновление недоступно"
                                    } else {
                                        "Фит пилота найден"
                                    }
                                    .into();
                                } else if phase == AnalysisPhase::NetworkPilot
                                    && self.pilot_error.is_none()
                                {
                                    self.pilot_status =
                                        "Подходящая PvP-потеря пилота на этом корпусе не найдена"
                                            .into();
                                }
                            }
                            Err(e) => self.status = e,
                        }
                    }
                }
                Message::Status(text) => self.status = text,
                Message::PilotRating(generation, pilot, result) => {
                    if (generation == self.pilot_generation || generation == self.generation)
                        && self.target.as_ref().is_some_and(|t| t.pilot == pilot)
                        && self.settings.pilot_rating
                    {
                        if self.rating_pilot != pilot {
                            self.pilot_rating = None;
                        }
                        self.rating_pilot = pilot;
                        match result {
                            Ok(rating) => {
                                self.pilot_rating = Some(rating);
                                self.rating_error = None;
                            }
                            Err(error) => self.rating_error = Some(error),
                        }
                    }
                }
                Message::PilotStatus(generation, ship, pilot, text) => {
                    if generation == self.pilot_generation
                        && self
                            .target
                            .as_ref()
                            .is_some_and(|t| t.ship == ship && t.pilot == pilot)
                    {
                        self.pilot_status = text;
                    }
                }
                Message::PilotReady(generation, ship, pilot, result) => {
                    if generation == self.pilot_generation
                        && self
                            .target
                            .as_ref()
                            .is_some_and(|t| t.ship == ship && t.pilot == pilot)
                    {
                        if let Err(error) = result {
                            self.pilot_status = format!("Фит пилота: {error}");
                            self.pilot_error = Some(error);
                        }
                        self.request_analysis(AnalysisPhase::NetworkPilot, Some(generation));
                    }
                }
                Message::SyncProgress(progress) => self.sync_progress = Some(progress),
                Message::SyncDone(result) => {
                    self.syncing = false;
                    self.status = match result {
                        Ok(count) => format!("Добавлено записей: {count}"),
                        Err(e) => e,
                    };
                    self.recalculate();
                }
                Message::DesktopCaptured(result) => {
                    match result {
                        Ok(capture) => {
                            let image = &capture.image;
                            let texture = ctx.load_texture(
                                "region-background",
                                egui::ColorImage::from_rgba_unmultiplied(
                                    [image.width() as usize, image.height() as usize],
                                    image.as_raw(),
                                ),
                                Default::default(),
                            );
                            self.selection_background = Some((capture.rect, texture));
                            self.capturing = false;
                            ctx.send_viewport_cmd_to(
                                ViewportId::ROOT,
                                ViewportCommand::Title("Выделение области".into()),
                            );
                            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Focus);
                        }
                        Err(error) => {
                            self.status = format!("Не удалось показать фон выделения: {error}");
                            self.selecting = false;
                            self.restore_pending = true;
                        }
                    }
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                }
                Message::ScreenCaptured => {
                    self.restore_pending = true;
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                }
                Message::ScreenChecked(check) => {
                    if self.draft.is_some() {
                        self.screen_state.set(ctx, *check);
                    }
                }
                Message::OcrState(epoch, status) => {
                    if epoch == self.ocr_epoch {
                        self.ocr_status = status;
                    }
                }
                Message::Ocr(epoch, result, observed_pilot) => {
                    if epoch == self.ocr_epoch {
                        match result {
                            Ok(target) => {
                                self.ocr_status = if target.is_some() {
                                    "OCR работает: цель распознана"
                                } else {
                                    "OCR работает: цель не найдена"
                                }
                                .into();
                                if let Some(change) = self.gate.feed_at(
                                    target,
                                    Instant::now(),
                                    self.settings.target_hold_seconds,
                                ) {
                                    if change
                                        .as_ref()
                                        .zip(self.target.as_ref())
                                        .is_some_and(|(a, b)| a.identity() == b.identity())
                                    {
                                        self.target = change.clone();
                                        if let Some(m) = &mut self.matchup {
                                            m.target = change.unwrap();
                                        }
                                    } else {
                                        self.target = change;
                                        self.matchup = None;
                                        self.generation += 1;
                                        if self.target.is_some() {
                                            self.recalculate();
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                self.ocr_status = format!(
                                    "OCR: {e} · повтор через {:.1} сек.",
                                    self.settings.ocr_interval
                                );
                                let changed = observed_pilot
                                    .as_ref()
                                    .zip(self.target.as_ref())
                                    .is_some_and(|(pilot, current)| {
                                        crate::ocr::clearly_different_pilot(pilot, &current.pilot)
                                    });
                                if changed {
                                    self.gate = Gate::default();
                                    self.clear_target_view();
                                } else if self
                                    .gate
                                    .feed_at(
                                        None,
                                        Instant::now(),
                                        self.settings.target_hold_seconds,
                                    )
                                    .is_some()
                                {
                                    self.clear_target_view();
                                }
                            }
                        }
                    }
                }
                #[cfg(windows)]
                Message::Tray(action) => match action.as_str() {
                    "settings" => {
                        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                        self.open_settings();
                    }
                    "show" => {
                        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true))
                    }
                    "quit" => ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close),
                    _ => {}
                },
            }
        }
    }
    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.image((self.app_icon.id(), Vec2::splat(22.)));
            let response = ui.add(
                egui::Label::new(
                    RichText::new("ТАКТИЧЕСКИЙ ПОМОЩНИК")
                        .size(11.)
                        .color(CYAN)
                        .strong(),
                )
                .sense(egui::Sense::drag()),
            );
            if response.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Закрыть").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
                if style::gear(ui).clicked() {
                    self.open_settings();
                }
            });
        });
        style::separator(ui);
    }
    fn overlay(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 4.;
        self.header(ui);
        let advice = self
            .matchup
            .as_ref()
            .and_then(|m| m.advice.get(self.selected))
            .and_then(Option::as_ref)
            .cloned();
        let diagnostic = self
            .matchup
            .as_ref()
            .and_then(|m| m.results.get(self.selected))
            .and_then(Option::as_ref);
        let comparison_error = diagnostic
            .and_then(|r| r["comparison_error"].as_str())
            .map(str::to_owned);
        let incoming_label = diagnostic
            .and_then(|r| r["incoming"]["label"].as_str())
            .unwrap_or("—")
            .to_string();
        let incoming_hint = diagnostic
            .and_then(|r| r["incoming"]["hint"].as_str())
            .unwrap_or("Выберите вариант фита")
            .to_string();
        let weapon_range = diagnostic
            .map(|r| r["weapon_range"].clone())
            .unwrap_or(Value::Null);
        let fitted_speed = diagnostic
            .map(|r| r["fitted_speed"].clone())
            .unwrap_or(Value::Null);
        let neutralizers = diagnostic
            .map(|r| r["neutralizers"].clone())
            .unwrap_or(Value::Null);
        let own_result = self.matchup.as_ref().and_then(|m| m.own.as_ref());
        let own_speed = own_result
            .map(|r| r["fitted_speed"].clone())
            .unwrap_or(Value::Null);
        let mut approximation_hint = String::new();
        for (label, result) in [("Противник", diagnostic), ("Свой фит", own_result)]
        {
            if let Some(result) = result {
                for estimate in array(&result["approximations"]) {
                    approximation_hint += &format!(
                        "{label}: {} → {}\n",
                        s(estimate, "original_name"),
                        s(estimate, "estimated_name")
                    );
                }
            }
        }
        let controls = self
            .matchup
            .as_ref()
            .and_then(|m| m.variants.get(self.selected))
            .and_then(|v| v.title.split(" · ").nth(1))
            .unwrap_or("")
            .replace(" / ", " · ")
            .to_uppercase();
        let target = self.target.clone();
        ui.horizontal(|ui| {
            let rating = if self.settings.pilot_rating {
                self.pilot_rating.as_ref()
            } else {
                None
            }
            .filter(|_| {
                target
                    .as_ref()
                    .is_some_and(|t| t.pilot == self.rating_pilot)
            });
            let state = if !self.settings.pilot_rating {
                "Соло-рейтинг выключен в настройках"
            } else if target.as_ref().is_none_or(|t| t.pilot.trim().is_empty()) {
                "Выберите пилота для соло-оценки"
            } else if self.rating_pilot == target.as_ref().map(|t| t.pilot.as_str()).unwrap_or("") {
                self.rating_error.as_deref().unwrap_or(if rating.is_some() {
                    ""
                } else {
                    "Загрузка соло-статистики zKillboard…"
                })
            } else {
                "Загрузка соло-статистики zKillboard…"
            };
            style::pilot_rating(ui, rating, state);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.;
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(
                            target
                                .as_ref()
                                .map(|t| t.ship.to_uppercase())
                                .unwrap_or_else(|| "НЕТ ЦЕЛИ".into()),
                        )
                        .size(23.)
                        .strong(),
                    );
                    if !controls.is_empty() {
                        ui.label(RichText::new(&controls).size(10.).color(style::YELLOW));
                    }
                });
                if let Some(t) = &target
                    && (!t.pilot.is_empty()
                        || t.distance.is_some()
                        || rating.is_some_and(|r| r.group_marker().is_some()))
                {
                    ui.horizontal(|ui| {
                        let mut label = t.pilot.clone();
                        if let Some(r) = t.distance {
                            if !label.is_empty() {
                                label += " · ";
                            }
                            label += &if r < 10000. {
                                format!("{r:.0} м")
                            } else {
                                format!("{:.1} км", r / 1000.).replace('.', ",")
                            };
                        }
                        ui.label(RichText::new(label).size(11.).color(CYAN));
                        if let Some(rating) = rating {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| style::pilot_group(ui, rating),
                            );
                        }
                    });
                }
            });
        });
        ui.add_space(2.);
        style::separator(ui);
        if let Some(a) = &advice {
            let action = if let Some(d) = self.target.as_ref().and_then(|t| t.distance) {
                if d > a.high {
                    "СБЛИЖАТЬСЯ"
                } else if d < a.low {
                    "УВЕЛИЧИВАТЬ ДИСТАНЦИЮ"
                } else {
                    "УДЕРЖИВАТЬ ДИСТАНЦИЮ"
                }
            } else {
                &a.action
            };
            ui.label(RichText::new(action).size(13.).color(CYAN))
                .on_hover_text(&a.reason);
            ui.label(
                RichText::new(tactics::range_text(a.low, a.high))
                    .size(30.)
                    .color(CYAN),
            );
        } else {
            let own = !self.settings.own_fit.is_empty();
            ui.label(
                RichText::new(
                    if own
                        && self
                            .matchup
                            .as_ref()
                            .is_some_and(|m| m.own.is_none() && !m.failures.is_empty())
                    {
                        "ПРОВЕРЬ СВОЙ ФИТ"
                    } else if comparison_error.is_some() {
                        "РАСЧЁТ НЕДОСТУПЕН"
                    } else if own && self.matchup.as_ref().is_some_and(|m| m.variants.is_empty()) {
                        "НЕТ ПОДХОДЯЩИХ ФИТОВ"
                    } else if own {
                        "ОЖИДАНИЕ РАСЧЁТА"
                    } else {
                        "ВЫБЕРИ СВОЙ ФИТ"
                    },
                )
                .size(13.)
                .color(CYAN),
            );
            ui.label(RichText::new("—").size(30.).color(CYAN));
            style::caption(
                ui,
                if let Some(error) = &comparison_error {
                    error
                } else if own
                    && self
                        .matchup
                        .as_ref()
                        .is_some_and(|m| m.pop.report.input == 0 && m.seed.is_some())
                {
                    "В доступном периоде нет потерь этого корпуса. Проверьте период в настройках."
                } else if own
                    && self
                        .matchup
                        .as_ref()
                        .is_some_and(|m| m.pop.report.input > 0 && m.variants.is_empty())
                {
                    "Потери есть в кеше, но не прошли выбранные фильтры. Подробности — в диагностике."
                } else if own {
                    ""
                } else {
                    "Настройки → Мой корабль → вставить фит из игры"
                },
            );
        }
        style::range_ruler(
            ui,
            target.as_ref().and_then(|t| t.distance),
            advice.as_ref().map(|a| (a.low, a.high)),
            &weapon_range,
        );
        style::separator(ui);
        egui::Grid::new("combat-metrics")
            .num_columns(2)
            .min_col_width(96.)
            .max_col_width((ui.available_width()-106.).max(100.))
            .spacing([10., 8.])
            .show(ui, |ui| {
                style::caption(ui, "ЕГО УРОН");
                let percentages=diagnostic.and_then(|r|serde_json::from_value::<[f64;4]>(r["incoming"]["percentages"].clone()).ok());
                ui.scope(|ui|style::damage_mix(ui,percentages,&incoming_label)).response.on_hover_text(&incoming_hint);
                ui.end_row();
                style::caption(ui, "ТВОЙ ЗАРЯД");
                ui.add(
                    egui::Label::new(
                        RichText::new(advice.as_ref().map(|a| a.ammo.as_str()).unwrap_or("—"))
                            .size(13.)
                            .color(CYAN),
                    )
                    .wrap(),
                );
                ui.end_row();
                style::caption(ui,"ТВОЙ УРОН");
                ui.scope(|ui|style::damage_mix(ui,advice.as_ref().and_then(|a|a.outgoing_percentages),"—")).response.on_hover_text("Типы урона своего фита с рекомендованным зарядом, включая выбранные дроны. До резистов и применения урона.");ui.end_row();
            });
        style::separator(ui);
        ui.columns(2, |cols| {
            let neut = neutralizers["label"].as_str().unwrap_or("—");
            let height =
                crate::fit_metrics::speed_rows(&fitted_speed, &own_speed).len() as f32 * 23. + 22.;
            cols[0]
                .scope(|ui| {
                    style::metric_height(
                        ui,
                        "НЕЙТРИКИ",
                        neut,
                        neut != "НЕТ В ФИТЕ" && neut != "—",
                        height,
                    )
                })
                .response
                .on_hover_text(
                    neutralizers["hint"]
                        .as_str()
                        .unwrap_or("Выберите вариант фита"),
                );
            style::speed_comparison(&mut cols[1], &fitted_speed, &own_speed);
        });
        if !approximation_hint.is_empty() {
            style::abyssal_alert(ui, &approximation_hint);
        }
        ui.add_space(4.);
        if let Some(m) = &self.matchup
            && let Some(Some(result)) = m.results.get(self.selected)
        {
            let title = &m.variants[self.selected].title;
            let layer = if title.contains("Armor") {
                "armor"
            } else if title.contains("Shield") {
                "shield"
            } else {
                "structure"
            };
            let r = &result["summary"]["defense"][layer]["resist"];
            let resistances = [n(r, "em"), n(r, "therm"), n(r, "kin"), n(r, "expl")];
            style::caption(
                ui,
                &format!(
                    "РЕЗИСТЫ / {}  ·  {}",
                    match layer {
                        "armor" => "БРОНЯ",
                        "shield" => "ЩИТ",
                        _ => "КОРПУС",
                    },
                    tactics::resistance_note(&resistances)
                ),
            );
            let base = &result["base_defense"][layer]["resist"];
            let base = base.is_object().then(|| {
                [
                    n(base, "em"),
                    n(base, "therm"),
                    n(base, "kin"),
                    n(base, "expl"),
                ]
            });
            style::resistance(ui, resistances, base);
        }
        style::separator(ui);
        style::heading(ui, "СОЛО-ВАРИАНТЫ");
        let mut details = None;
        let mut exclude = None;
        let mut selected = self.selected;
        if let Some(m) = &self.matchup {
            egui::ScrollArea::vertical()
                .id_salt("variants")
                .max_height((ui.clip_rect().bottom() - ui.cursor().top() - 83.).clamp(56., 180.))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (index, v) in m.variants.iter().enumerate() {
                        let r =
                            style::variant(ui, &v.title, v.percent, v.personal, index == selected);
                        if r.clicked() {
                            selected = index;
                        }
                        if r.double_clicked() {
                            details = Some(fits::eft(&v.fit, &self.catalog));
                        }
                        r.context_menu(|ui| {
                            if ui.button("Полный фит").clicked() {
                                details = Some(fits::eft(&v.fit, &self.catalog));
                                ui.close();
                            }
                            if ui.button("Копировать EFT").clicked() {
                                ui.ctx().copy_text(fits::eft(&v.fit, &self.catalog));
                                ui.close();
                            }
                            if ui.button("Исключить этот фит").clicked() {
                                exclude = Some(fits::fingerprint(&v.fit));
                                ui.close();
                            }
                        });
                    }
                });
            if m.pop.other >= 0.05 {
                style::caption(
                    ui,
                    &format!("Другие варианты: {:.1}%", m.pop.other).replace('.', ","),
                );
            }
            if m.variants.is_empty() {
                style::caption(
                    ui,
                    if m.pop.report.input == 0 {
                        "В локальной базе нет потерь этого корпуса за доступный период"
                    } else {
                        "Записи есть; подходящих фитов после фильтрации нет"
                    },
                );
            }
        } else {
            style::caption(ui, "Варианты появятся из локальной базы");
        }
        if self.selected != selected {
            self.selected = selected;
            ui.ctx().request_repaint();
        }
        if let Some(value) = details {
            self.details = Some(value);
        }
        if let Some(key) = exclude {
            self.settings.excluded.push(key);
            let _ = self.settings.save(&self.directory);
            self.recalculate();
        }
        style::separator(ui);
        let own = self
            .matchup
            .as_ref()
            .and_then(|m| m.own.as_ref())
            .map(|v| s(&v["hull"], "name"))
            .unwrap_or("НЕ ВЫБРАН");
        let status = if self.pilot_status.is_empty() {
            self.status.clone()
        } else {
            format!("{} · {}", self.status, self.pilot_status)
        };
        ui.horizontal_top(|ui| {
            let rating_width = if self.settings.pilot_rating { 82. } else { 0. };
            ui.allocate_ui_with_layout(
                Vec2::new((ui.available_width() - rating_width - 8.).max(1.), 30.),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 2.;
                    style::caption(ui, &format!("МОЙ ФИТ / {own}"));
                    ui.add(
                        egui::Label::new(RichText::new(&status).size(9.).color(MUTED)).truncate(),
                    )
                    .on_hover_text(&status);
                },
            );
            if self.settings.pilot_rating {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(82., 30.), egui::Sense::hover());
                let p = ui.painter();
                p.text(
                    rect.right_top(),
                    egui::Align2::RIGHT_TOP,
                    "ТВОЙ SOLO",
                    egui::FontId::proportional(8.),
                    MUTED,
                );
                p.text(
                    rect.right_top() + Vec2::new(0., 12.),
                    egui::Align2::RIGHT_TOP,
                    self.own_rating
                        .as_ref()
                        .and_then(|r| r.score)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".into()),
                    egui::FontId::proportional(12.),
                    style::rating_color(self.own_rating.as_ref()).gamma_multiply(0.85),
                );
                response.on_hover_text(self.own_rating.as_ref().map(|r| r.hint()).unwrap_or_else(
                    || {
                        if self.profile.character_id <= 0 {
                            "Войдите персонажем в EVE SSO для своей соло-оценки".into()
                        } else {
                            self.own_rating_error
                                .clone()
                                .unwrap_or_else(|| "Загрузка соло-статистики zKillboard…".into())
                        }
                    },
                ));
            }
        });
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::Label::new(RichText::new(&self.ocr_status).size(9.).color(MUTED))
                        .truncate(),
                )
                .on_hover_text(&self.ocr_status);
            });
        });
        if self.target.is_some()
            && let Some(remaining) = self
                .gate
                .remaining(Instant::now(), self.settings.target_hold_seconds)
        {
            ui.label(
                RichText::new(format!(
                    "Цель не видна · сохранена ещё {} с",
                    remaining.as_secs_f64().ceil() as u32
                ))
                .size(10.)
                .color(style::AMBER),
            );
        }
        if let Some(error) = &self.own_ship_error {
            ui.label(
                RichText::new(format!("Смена корабля: {error}"))
                    .size(10.)
                    .color(style::AMBER),
            );
        }
    }

    fn settings_view(&mut self, ctx: &egui::Context) {
        if self.draft.is_none() || self.selecting || self.capturing {
            return;
        }
        // Изменения вида видны сразу, но Отмена должна вернуть сохранённые настройки.
        let mut draft = self.draft.take().unwrap();
        let mut response = crate::settings_ui::Response::default();
        let cache_status = self
            .matchup
            .as_ref()
            .map(|m| {
                let mut text = format!(
                    "{}: {} потерь в кеше, {} прошло фильтры",
                    m.target.ship, m.pop.report.input, m.pop.total
                );
                if let Some(seed) = &m.seed {
                    text += &format!(
                        "\nСтартовая база: {} корпусов, {}–{}",
                        seed.known_hulls, seed.period_start, seed.period_end
                    );
                }
                if let Some(error) = &m.seed_error {
                    text += &format!("\nСтартовая база: {error}");
                }
                text
            })
            .unwrap_or_else(|| "Локальная база ещё не прочитана".into());
        let available = (self.monitor_bounds)()
            .map(|rect| rect.size() / ctx.pixels_per_point())
            .unwrap_or(Vec2::new(1920., 1080.));
        let builder = egui::ViewportBuilder::default()
            .with_title("Настройки · EVE Combat Assistant")
            .with_visible(true)
            .with_inner_size([
                760_f32.min(available.x - 30.),
                600_f32.min(available.y - 60.),
            ])
            .with_min_inner_size([380., 350.])
            .with_always_on_top();
        ctx.show_viewport_immediate(ViewportId::from_hash_of("settings"), builder, |ui, _| {
            let current = crate::settings_ui::Window {
                catalog: &self.catalog,
                tab: &mut self.tab,
                dirty: draft != self.settings,
                status: &self.status,
                cache_status: &cache_status,
                sync_progress: self.sync_progress.as_ref(),
                syncing: self.syncing,
                cancel_sync: &self.workers.cancel,
                profile: &self.profile,
                fit_state: &mut self.fit_state,
                account_busy: self.account_busy || self.own_ship_busy,
                fit_busy: self.fit_busy,
                screen_state: &mut self.screen_state,
                ocr_status: &self.ocr_status,
            }
            .show(ui, &mut draft);
            response.action = current.action.or(response.action);
            response.error = current.error.or(response.error.take());
            if ui.ctx().input(|input| input.viewport().close_requested()) {
                response.action = Some(crate::settings_ui::Action::Cancel);
            }

            // eframe/glow не возвращает снимки immediate-окон; тест снимает их через GDI.
            self.settings_frames += 1;
            if let Some(path) = &self.args.settings_screenshot
                && !self.settings_shot
                && self.matchup.is_some()
                && self.settings_frames >= 3
                && platform::save_settings_window(path).is_ok()
            {
                self.settings_shot = true;
                self.shots_done.fetch_add(1, Ordering::Relaxed);
            }
            if self.args.settings_screenshot.is_some() && !self.settings_shot {
                ui.ctx().request_repaint();
            }
        });
        if let Some(error) = response.error {
            self.status = error;
        }
        use crate::settings_ui::Action;
        match response.action {
            Some(Action::OpenDebugFolder) => {
                let path = self.directory.join("debug");
                let result = std::fs::create_dir_all(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|()| {
                        let url = url::Url::from_directory_path(&path)
                            .map_err(|_| "Не удалось открыть папку".to_string())?;
                        webbrowser::open(url.as_str()).map_err(|e| e.to_string())
                    });
                if let Err(error) = result {
                    self.status = error;
                }
                self.draft = Some(draft);
            }
            Some(Action::CheckScreen) => {
                self.ensure_system_report(&draft);
                self.screen_state = crate::settings_ui::ScreenState {
                    busy: true,
                    language: draft.language.clone(),
                    ..Default::default()
                };
                self.draft = Some(draft);
                self.capturing = true;
                self.capture_pending = Some((Instant::now(), CaptureKind::Check));
                hide_capture_windows(ctx);
            }
            Some(Action::Login) | Some(Action::RefreshCharacter) => {
                if !self.account_busy && !self.own_ship_busy {
                    self.account_busy = true;
                    self.account_cancel.store(false, Ordering::Relaxed);
                    let tx = self.tx.clone();
                    let ctx = ctx.clone();
                    let dir = self.directory.clone();
                    let client = draft.sso_client_id.clone();
                    let catalog = self.catalog.clone();
                    let cancel = self.account_cancel.clone();
                    let login = response.action == Some(Action::Login);
                    std::thread::spawn(move || {
                        platform::low_priority();
                        let progress = |text| {
                            let _ = tx.send(Message::Status(text));
                            ctx.request_repaint();
                        };
                        let result = crate::account::synchronize(
                            &dir, &client, login, &catalog, &cancel, &progress,
                        );
                        let _ = tx.send(Message::CharacterReady(result));
                        ctx.request_repaint();
                    });
                }
                self.draft = Some(draft);
            }
            Some(Action::Logout) => {
                match crate::account::logout(&self.directory) {
                    Ok(()) => {
                        self.profile = Default::default();
                        self.status = "Вход удалён с этого компьютера".into();
                    }
                    Err(error) => self.status = error,
                }
                self.draft = Some(draft);
            }
            Some(Action::CalculateFit) => {
                if !self.fit_busy {
                    self.fit_busy = true;
                    let settings = draft.clone();
                    let key = settings
                        .own_request(json!(settings.own_fit), true)
                        .to_string();
                    let catalog = self.catalog.clone();
                    let tx = self.tx.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        platform::low_priority();
                        let result = crate::fit_plan::calculate(&settings, &catalog);
                        let _ = tx.send(Message::FitReady(key, result));
                        ctx.request_repaint();
                    });
                }
                self.draft = Some(draft);
            }
            Some(Action::Sync) => {
                self.sync(draft.clone());
                self.draft = Some(draft);
            }
            Some(Action::Save) => {
                if let Err(error) = self.apply(draft.clone()) {
                    self.status = error;
                    self.draft = Some(draft);
                }
            }
            Some(Action::Cancel) => {
                self.screen_state = Default::default();
                self.draft = None;
                ctx.set_zoom_factor(self.settings.scale / 100.);
                self.configure_ocr();
            }
            Some(Action::SelectRegion) | Some(Action::SelectTargetsRegion) => {
                self.draft = Some(draft);
                self.selecting = true;
                self.selecting_targets = response.action == Some(Action::SelectTargetsRegion);
                self.selection_start = None;
                self.selection_background = None;
                let ppp = ctx.pixels_per_point();
                self.root_before_selection = ctx
                    .input(|input| input.viewport().outer_rect)
                    .map(|rect| egui::Rect::from_min_max(rect.min * ppp, rect.max * ppp));
                self.capturing = true;
                self.capture_pending = Some((Instant::now(), CaptureKind::Region));
                hide_capture_windows(ctx);
            }
            None => self.draft = Some(draft),
        }
    }
    fn region_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some((bounds, background)) = &self.selection_background else {
            return;
        };
        let bounds = *bounds;
        let background = background.clone();
        let ppp = ctx.pixels_per_point();
        ctx.send_viewport_cmd_to(
            ViewportId::ROOT,
            ViewportCommand::OuterPosition(egui::pos2(
                bounds[0] as f32 / ppp,
                bounds[1] as f32 / ppp,
            )),
        );
        ctx.send_viewport_cmd_to(
            ViewportId::ROOT,
            ViewportCommand::InnerSize(egui::vec2(bounds[2] as f32 / ppp, bounds[3] as f32 / ppp)),
        );
        ctx.send_viewport_cmd_to(
            ViewportId::ROOT,
            ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop),
        );
        ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::MousePassthrough(false));
        let mut chosen = None;
        let mut cancel = false;
        let rect = ui.max_rect();
        ui.painter().image(
            background.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
            Color32::WHITE,
        );
        ui.painter()
            .rect_filled(rect, 0., Color32::from_black_alpha(20));
        ui.painter().text(
            rect.left_top() + Vec2::splat(30.),
            egui::Align2::LEFT_TOP,
            if self.selecting_targets {
                "Выделите ники и корпуса со звёздочкой на карточках целей · Esc — отмена"
            } else {
                "Выделите ник в правой панели текущего выбора · Esc — отмена"
            },
            egui::FontId::proportional(20.),
            CYAN,
        );
        let response = ui.interact(rect, ui.scope_id().with("drag-region"), egui::Sense::drag());
        if response.drag_started() {
            self.selection_start = ui.ctx().input(|input| input.pointer.press_origin());
        }
        if let (Some(start), Some(end)) = (self.selection_start, response.interact_pointer_pos()) {
            let selection = egui::Rect::from_two_pos(start, end);
            style::border(ui, selection);
            if response.drag_stopped() && selection.width() > 30. && selection.height() > 15. {
                chosen = selection_rectangle(selection, rect, bounds);
            }
        }
        if ui
            .ctx()
            .input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested())
        {
            cancel = true;
        }
        if chosen.is_some() || cancel {
            if let Some(rect) = chosen
                && let Some(draft) = &mut self.draft
            {
                if self.selecting_targets {
                    draft.targets_region = Some(rect);
                } else {
                    draft.region = Some(rect);
                }
            }
            self.selecting = false;
            self.selection_start = None;
            self.selection_background = None;
            if let Some(rect) = self.root_before_selection.take() {
                ctx.send_viewport_cmd_to(
                    ViewportId::ROOT,
                    ViewportCommand::OuterPosition(rect.min / ppp),
                );
                ctx.send_viewport_cmd_to(
                    ViewportId::ROOT,
                    ViewportCommand::InnerSize(rect.size() / ppp),
                );
            }
            ctx.send_viewport_cmd_to(
                ViewportId::ROOT,
                ViewportCommand::Title("EVE Combat Assistant".into()),
            );
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::CancelClose);
            self.layout = None;
            ctx.request_repaint();
        }
    }
}
impl eframe::App for CombatApp {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if !self.initialized {
            self.initialized = true;
            self.configure_ocr();
            self.recalculate();
        }
        self.messages(ctx);
        self.ensure_system_report(&self.settings.clone());
        self.refresh_own_rating(ctx);
        if !self.args.demo
            && self.draft.is_none()
            && !self.account_busy
            && !self.own_ship_busy
            && self.profile.character_id > 0
            && self.directory.join("sso.dat").is_file()
            && Instant::now() >= self.next_ship_check
        {
            self.own_ship_busy = true;
            self.next_ship_check = Instant::now() + std::time::Duration::from_secs(60);
            let directory = self.directory.clone();
            let client = self.settings.sso_client_id.clone();
            let catalog = self.catalog.clone();
            let cancel = self.account_cancel.clone();
            let character = self.profile.character_id;
            let previous = id(&self.profile.ship, "ship_item_id");
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                platform::low_priority();
                let changed = || {
                    let _ = tx.send(Message::OwnShipChanging(character));
                    ctx.request_repaint();
                };
                let result = crate::account::check_ship(
                    &directory, &client, character, previous, &catalog, &cancel, &changed,
                );
                let _ = tx.send(Message::OwnShipReady(character, result));
                ctx.request_repaint();
            });
        }
        if !self.args.demo && self.profile.character_id > 0 {
            ctx.request_repaint_after(
                self.next_ship_check
                    .saturating_duration_since(Instant::now())
                    .max(std::time::Duration::from_secs(1)),
            );
        }
        if self
            .gate
            .expire(Instant::now(), self.settings.target_hold_seconds)
        {
            self.clear_target_view();
        }
        if self
            .gate
            .remaining(Instant::now(), self.settings.target_hold_seconds)
            .is_some()
        {
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        if let Some((start, kind)) = self.capture_pending {
            if start.elapsed() >= std::time::Duration::from_millis(450) {
                self.capture_pending = None;
                let request = match kind {
                    CaptureKind::Check => self.draft.clone().map(OcrRequest::Check),
                    CaptureKind::Region => Some(OcrRequest::CaptureDesktop),
                };
                if request.is_some_and(|request| self.workers.ocr.send(request).is_err()) {
                    self.selecting = false;
                    self.restore_pending = true;
                    self.screen_state.busy = false;
                    self.status = "Поток OCR недоступен".into();
                    ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Visible(true));
                }
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
        }
        if self.capturing {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        let want = usize::from(self.args.screenshot.is_some())
            + usize::from(self.args.settings_screenshot.is_some());
        if want > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if want > 0
            && (self.shots_done.load(Ordering::Relaxed) >= want
                || self.started.elapsed().as_secs() > 90)
        {
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::Close);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.selecting {
            self.region_ui(ui);
            return;
        }
        let appearance = self.draft.as_ref().unwrap_or(&self.settings).clone();
        let zoom_ready = (ctx.zoom_factor() - appearance.scale / 100.).abs() <= 0.001;
        if !zoom_ready {
            // Масштаб вступает в силу со следующего прохода egui. Размер окна
            // вычисляем после этого, иначе смешиваются старые points и новые pixels.
            ctx.set_zoom_factor(appearance.scale / 100.);
        }
        let work_area = (self.monitor_bounds)().unwrap_or_else(|| {
            egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1920., 1080.))
        });
        let current_layout = WindowLayout {
            on_top: appearance.on_top,
            click_through: appearance.click_through && self.draft.is_none(),
            scale: appearance.scale,
            width: appearance.width,
            work_area,
        };
        if zoom_ready && self.layout.as_ref() != Some(&current_layout) {
            self.layout = Some(current_layout);
            let old_position = ctx.input(|input| input.viewport().outer_rect.map(|rect| rect.min));
            let (size, position) = window_geometry(
                appearance.width,
                work_area,
                ctx.pixels_per_point(),
                old_position,
            );
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::InnerSize(size));
            ctx.send_viewport_cmd_to(ViewportId::ROOT, ViewportCommand::OuterPosition(position));
            ctx.send_viewport_cmd_to(
                ViewportId::ROOT,
                ViewportCommand::WindowLevel(if appearance.on_top {
                    egui::WindowLevel::AlwaysOnTop
                } else {
                    egui::WindowLevel::Normal
                }),
            );
            ctx.send_viewport_cmd_to(
                ViewportId::ROOT,
                ViewportCommand::MousePassthrough(appearance.click_through && self.draft.is_none()),
            );
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgba_unmultiplied(
                        4,
                        13,
                        22,
                        (appearance.background * appearance.opacity / 100. * 2.55) as u8,
                    ))
                    .inner_margin(14.),
            )
            .show(ui, |ui| {
                ui.set_opacity(appearance.opacity / 100.);
                let border = ui.max_rect().expand(13.);
                style::border(ui, border);
                egui::ScrollArea::vertical()
                    .id_salt("main")
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.overlay(ui));
            });
        if self.args.screenshot.is_some() && !self.root_shot && self.matchup.is_some() {
            self.root_shot = true;
            request_shot(
                &ctx,
                self.args.screenshot.clone().unwrap(),
                self.shots_done.clone(),
            );
            if self.args.settings_screenshot.is_some() {
                self.open_settings();
            }
        }
        if self.args.screenshot.is_none()
            && self.args.settings_screenshot.is_some()
            && !self.settings_shot
            && self.matchup.is_some()
        {
            self.open_settings();
        }
        self.settings_view(&ctx);
        if let Some(text) = self
            .details
            .clone()
            .filter(|_| !self.capturing && !self.selecting)
        {
            let mut open = true;
            ctx.show_viewport_immediate(
                ViewportId::from_hash_of("fit-detail"),
                egui::ViewportBuilder::default()
                    .with_title("Полный фит")
                    .with_visible(true)
                    .with_inner_size([570., 620.])
                    .with_always_on_top(),
                |ui, _| {
                    egui::CentralPanel::default().show(ui, |ui| {
                        ui.heading("ФИТ");
                        let mut text = text.clone();
                        ui.add(
                            egui::TextEdit::multiline(&mut text)
                                .code_editor()
                                .desired_width(f32::INFINITY)
                                .desired_rows(23),
                        );
                        if ui.button("Копировать EFT").clicked() {
                            ui.ctx().copy_text(text);
                        }
                        if ui.button("Закрыть").clicked()
                            || ui.ctx().input(|i| i.viewport().close_requested())
                        {
                            open = false;
                        }
                    });
                },
            );
            if !open {
                self.details = None;
            }
        }
        // Сначала убираем скрытые immediate-окна из одного UI-прохода.
        // Иначе egui вызывает их с visible=false и пропускает callback.
        if self.restore_pending {
            self.restore_pending = false;
            self.capturing = false;
            ctx.request_repaint();
        }
    }
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0., 0., 0., 0.]
    }
}
impl Drop for CombatApp {
    fn drop(&mut self) {
        self.account_cancel.store(true, Ordering::Relaxed);
        self.workers.cancel.store(true, Ordering::Relaxed);
        let _ = self.settings.save(&self.directory);
    }
}
fn request_shot(ctx: &egui::Context, path: PathBuf, count: Arc<AtomicUsize>) {
    let context = ctx.clone();
    ctx.request_screenshot(move |image| {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let raw: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|p| p.to_srgba_unmultiplied())
            .collect();
        let _ = image::save_buffer(
            &path,
            &raw,
            image.width() as u32,
            image.height() as u32,
            image::ColorType::Rgba8,
        );
        count.fetch_add(1, Ordering::Relaxed);
        context.request_repaint();
    });
}
pub fn cli(args: &Args, dir: &Path) -> Result<(), String> {
    let catalog = Catalog::load()?;
    let ship = catalog
        .ship_id(&args.ship)
        .ok_or("Неизвестный тип корабля")?;
    if let Some(start) = &args.archive_from {
        let start =
            chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d").map_err(|e| e.to_string())?;
        let end = chrono::NaiveDate::parse_from_str(
            args.archive_to.as_deref().ok_or("Укажи --archive-to")?,
            "%Y-%m-%d",
        )
        .map_err(|e| e.to_string())?;
        let mut source = Sources::new(
            Store::open(dir)?,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            catalog.clone(),
        );
        let count = source.archive(start, end, &|s| println!("{s}"))?;
        println!("Добавлено: {count}");
        return Ok(());
    }
    if args.sync {
        let mut source = Sources::new(
            Store::open(dir)?,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
            catalog.clone(),
        );
        let count = source.sync_ship(ship, args.days, &|s| println!("{s}"))?;
        println!("Добавлено: {count}");
        return Ok(());
    }
    let store = Store::open(dir)?;
    let mut settings = Settings::load(dir);
    settings.days = args.days;
    let pop = fits::population(&store.global_mails(ship)?, &catalog, &settings);
    let mut engine = Engine::default();
    let raw = std::fs::read_to_string(args.fit.as_ref().ok_or("Укажи --fit")?)
        .map_err(|e| e.to_string())?;
    let own = engine.evaluate(settings.own_request(json!(raw), true))?;
    let mut variants = Vec::new();
    for v in pop.variants {
        let result = engine.evaluate(json!({"fit":tactics::fill_charges(&v.fit,&catalog)}));
        match result.and_then(|enemy| {
            tactics::advise(&own, &enemy, &mut engine, &catalog, &settings, None, 0.)
        }) {
            Ok(advice) => variants.push(json!({"fit":v.title,"percent":v.percent,"advice":advice})),
            Err(error) => variants.push(json!({"fit":v.title,"percent":v.percent,"error":error})),
        }
    }
    let report = json!({
        "sample": pop.total,
        "excluded": pop.excluded,
        "unknown": pop.unknown,
        "other": pop.other,
        "variants": variants,
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    Ok(())
}

/// Ограничиваем размер и позицию сразу: окно может находиться на втором мониторе.
/// На входе bounds — физические пиксели, результат — points текущего масштаба egui.
pub(crate) fn window_geometry(
    width: f32,
    bounds: egui::Rect,
    pixels_per_point: f32,
    old_position: Option<egui::Pos2>,
) -> (Vec2, egui::Pos2) {
    let work = bounds / pixels_per_point;
    let size = Vec2::new(
        width.min(work.width() - 12.),
        900_f32.min(work.height() - 12.),
    );
    let position = old_position
        .unwrap_or(work.min)
        .clamp(work.min, work.max - size);
    (size, position)
}

fn hide_capture_windows(ctx: &egui::Context) {
    for id in [
        ViewportId::ROOT,
        ViewportId::from_hash_of("settings"),
        ViewportId::from_hash_of("fit-detail"),
    ] {
        ctx.send_viewport_cmd_to(id, ViewportCommand::Visible(false));
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(450));
}

/// Преобразование выделения на снимке в физические координаты рабочего стола.
/// Не зависит от DPI/масштаба окна и поддерживает отрицательные координаты монитора.
pub(crate) fn selection_rectangle(
    selection: egui::Rect,
    canvas: egui::Rect,
    bounds: [i32; 4],
) -> Option<[i32; 4]> {
    if !selection.is_finite()
        || !canvas.is_finite()
        || !canvas.is_positive()
        || bounds[2] <= 0
        || bounds[3] <= 0
    {
        return None;
    }
    let selection = selection.intersect(canvas);
    let x = |p: f32| ((p - canvas.left()) / canvas.width() * bounds[2] as f32).round() as i32;
    let y = |p: f32| ((p - canvas.top()) / canvas.height() * bounds[3] as f32).round() as i32;
    let (left, top, right, bottom) = (
        x(selection.left()),
        y(selection.top()),
        x(selection.right()),
        y(selection.bottom()),
    );
    (right - left >= 10 && bottom - top >= 10).then_some([
        bounds[0] + left,
        bounds[1] + top,
        right - left,
        bottom - top,
    ])
}
