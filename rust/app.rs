//! Состояние приложения: выбранная цель, ответы потоков и окна интерфейса.

use crate::{
    Args,
    engine::{Catalog, Engine},
    fits,
    model::{Settings, array, n, s},
    ocr::{Gate, Target},
    platform,
    sources::Sources,
    store::Store,
    style::{self, CYAN, MUTED, TEXT},
    tactics,
    worker::{AnalysisRequest, Matchup, Message, NetworkRequest, OcrConfig, OcrRequest, Workers},
};
use eframe::egui::{self, Color32, RichText, Vec2, ViewportCommand, ViewportId};
use serde_json::json;
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
    fit_busy: bool,
    account_cancel: Arc<std::sync::atomic::AtomicBool>,
    status: String,
    tab: usize,
    syncing: bool,
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
    ship_texture: egui::TextureHandle,
    root_shot: bool,
    settings_frames: usize,
    settings_shot: bool,
    shots_done: Arc<AtomicUsize>,
    started: Instant,
    initialized: bool,
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
        let image = image::load_from_memory(include_bytes!("../resources/ships/587.png"))
            .expect("ship image")
            .to_rgba8();
        let texture = cc.egui_ctx.load_texture(
            "Rifter",
            egui::ColorImage::from_rgba_unmultiplied(
                [image.width() as usize, image.height() as usize],
                image.as_raw(),
            ),
            Default::default(),
        );
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
        Self {
            tab: args.settings_tab.min(6),
            draft: args.open_settings.then(|| settings.clone()),
            profile: crate::account::Profile::load(&directory),
            fit_state: Default::default(),
            account_busy: false,
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
            syncing: false,
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
            ship_texture: texture,
            root_shot: false,
            settings_frames: 0,
            settings_shot: false,
            shots_done: Arc::new(AtomicUsize::new(0)),
            started: Instant::now(),
            initialized: false,
            layout: None,
            monitor_bounds,
            #[cfg(windows)]
            _tray: tray,
        }
    }
    fn recalculate(&mut self) {
        if let Some(target) = &self.target {
            self.generation += 1;
            let _ = self.workers.compute.send(AnalysisRequest {
                generation: self.generation,
                settings: self.settings.clone(),
                target: target.clone(),
            });
        }
    }
    fn request_pilot(&self) {
        if !self.settings.auto_pilot_lookup || self.args.demo {
            return;
        }
        if let Some(target) = &self.target
            && !target.pilot.trim().is_empty()
        {
            let _ = self.workers.network.send(NetworkRequest {
                settings: self.settings.clone(),
                target: target.clone(),
                global: false,
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
        self.workers.cancel.store(false, Ordering::Relaxed);
        let target = Target {
            ship: settings.ship.clone(),
            pilot: String::new(),
            distance: None,
            angular: 0.,
        };
        let _ = self.workers.network.send(NetworkRequest {
            settings,
            target,
            global: true,
        });
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
            Engine::default().evaluate(request)?;
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
        self.request_pilot();
        Ok(())
    }
    fn messages(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::CharacterReady(result) => {
                    self.account_busy = false;
                    match result {
                        Ok(profile) => {
                            let settings = self.draft.as_mut().unwrap_or(&mut self.settings);
                            settings.skills = Some(json!(profile.skills));
                            settings.implants = profile.implants.clone();
                            if let Some(fit) = &profile.current_fit {
                                settings.own_fit = fit.clone();
                            }
                            self.status = format!(
                                "{} · загружено навыков: {}",
                                profile.name,
                                profile.skills.len()
                            );
                            self.profile = profile;
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
                Message::Analyzed(generation, result) => {
                    if generation == self.generation {
                        match result {
                            Ok(m) => {
                                self.status = if self.args.demo {
                                    format!("Демонстрация · {} подходящих фитов", m.pop.total)
                                } else {
                                    format!("В выборке: {} соло-фитов", m.pop.total)
                                };
                                if !m.failures.is_empty() {
                                    self.status += " · часть расчётов недоступна";
                                }
                                self.matchup = Some(m);
                                self.selected = 0;
                            }
                            Err(e) => self.status = e,
                        }
                    }
                }
                Message::Status(text) => self.status = text,
                Message::PilotReady(ship, pilot, result) => {
                    if self
                        .target
                        .as_ref()
                        .is_some_and(|t| t.ship == ship && t.pilot == pilot)
                    {
                        match result {
                            Ok(()) => self.recalculate(),
                            Err(e) => self.status = e,
                        }
                    }
                }
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
                Message::Ocr(epoch, result) => {
                    if epoch == self.ocr_epoch {
                        match result {
                            Ok(target) => {
                                self.ocr_status = if target.is_some() {
                                    "OCR работает: цель распознана"
                                } else {
                                    "OCR работает: цель не найдена"
                                }
                                .into();
                                if let Some(change) = self.gate.feed(target) {
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
                                            self.request_pilot();
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                self.ocr_status = format!(
                                    "OCR: {e} · повтор через {:.1} сек.",
                                    self.settings.ocr_interval
                                );
                                self.gate = Gate::default();
                                self.target = None;
                                self.matchup = None;
                                self.generation += 1;
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
            let (dot, _) = ui.allocate_exact_size(Vec2::new(8., 14.), egui::Sense::hover());
            ui.painter().circle_filled(dot.center(), 2.5, CYAN);
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
        ui.add_space(5.);
        style::separator(ui);
    }
    fn overlay(&mut self, ui: &mut egui::Ui) {
        self.header(ui);
        let target = self.target.clone();
        ui.horizontal(|ui| {
            if target.as_ref().is_some_and(|t| t.ship == "Rifter") {
                ui.image((self.ship_texture.id(), Vec2::new(88., 70.)));
            } else {
                let (r, _) = ui.allocate_exact_size(Vec2::new(88., 70.), egui::Sense::hover());
                ui.painter()
                    .circle_stroke(r.center(), 23., egui::Stroke::new(1., style::LINE));
                ui.painter().line_segment(
                    [
                        r.center() - Vec2::new(28., 0.),
                        r.center() + Vec2::new(28., 0.),
                    ],
                    egui::Stroke::new(1., CYAN),
                );
            }
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(
                        target
                            .as_ref()
                            .map(|t| t.ship.to_uppercase())
                            .unwrap_or_else(|| "НЕТ ЦЕЛИ".into()),
                    )
                    .size(26.)
                    .strong(),
                );
                if let Some(t) = &target {
                    if !t.pilot.is_empty() {
                        ui.label(RichText::new(&t.pilot).size(12.).color(MUTED));
                    }
                    if let Some(r) = t.distance {
                        ui.label(
                            RichText::new(format!("{:.1} км", r / 1000.).replace('.', ","))
                                .size(11.)
                                .color(CYAN),
                        );
                    }
                }
            });
        });
        ui.add_space(10.);
        style::separator(ui);
        let advice = self
            .matchup
            .as_ref()
            .and_then(|m| m.advice.get(self.selected))
            .and_then(Option::as_ref)
            .cloned();
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
            ui.label(RichText::new(action).size(17.).color(CYAN));
            ui.label(
                RichText::new(tactics::range_text(a.low, a.high))
                    .size(37.)
                    .color(CYAN),
            );
            ui.label(RichText::new(&a.reason).size(11.).color(MUTED));
        } else {
            let own = !self.settings.own_fit.is_empty();
            ui.label(
                RichText::new(if own {
                    "ОЖИДАНИЕ ДАННЫХ"
                } else {
                    "ВЫБЕРИ СВОЙ ФИТ"
                })
                .size(17.)
                .color(CYAN),
            );
            ui.label(RichText::new("—").size(37.).color(CYAN));
            style::caption(
                ui,
                if own {
                    "Настройки → Данные → обновить статистику"
                } else {
                    "Настройки → Мой корабль → вставить фит из игры"
                },
            );
        }
        ui.add_space(6.);
        style::separator(ui);
        style::heading(ui, "СОЛО-ВАРИАНТЫ");
        let mut details = None;
        let mut exclude = None;
        let mut selected = self.selected;
        if let Some(m) = &self.matchup {
            egui::ScrollArea::vertical()
                .id_salt("variants")
                .max_height(230.)
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
                style::caption(ui, "Нет подходящих фитов в локальной базе");
            }
        } else {
            style::caption(ui, "Варианты появятся из локальной базы");
        }
        self.selected = selected;
        if let Some(value) = details {
            self.details = Some(value);
        }
        if let Some(key) = exclude {
            self.settings.excluded.push(key);
            let _ = self.settings.save(&self.directory);
            self.recalculate();
        }
        ui.add_space(8.);
        ui.columns(2, |cols| {
            style::metric(
                &mut cols[0],
                "ЕГО УРОН",
                advice.as_ref().map(|a| a.damage.as_str()).unwrap_or("—"),
                false,
            );
            style::metric(
                &mut cols[1],
                "ТВОИ БОЕПРИПАСЫ",
                advice.as_ref().map(|a| a.ammo.as_str()).unwrap_or("—"),
                false,
            );
        });
        ui.columns(2, |cols| {
            let neut = advice.as_ref().map(|a| a.neut.as_str()).unwrap_or("—");
            let speed = advice.as_ref().map(|a| a.speed.as_str()).unwrap_or("—");
            style::metric(
                &mut cols[0],
                "НЕЙТРИКИ",
                neut,
                neut != "НЕТ В ФИТЕ" && neut != "—",
            );
            style::metric(&mut cols[1], "СКОРОСТЬ", speed, speed == "ОН БЫСТРЕЕ");
        });
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
            style::caption(
                ui,
                &format!(
                    "РЕЗИСТЫ / {}  ·  СЛАБЕЕ {}",
                    match layer {
                        "armor" => "БРОНЯ",
                        "shield" => "ЩИТ",
                        _ => "КОРПУС",
                    },
                    advice.as_ref().map(|a| a.weak.as_str()).unwrap_or("—")
                ),
            );
            let r = &result["summary"]["defense"][layer]["resist"];
            style::resistance(ui, [n(r, "em"), n(r, "therm"), n(r, "kin"), n(r, "expl")]);
        }
        ui.add_space(7.);
        style::separator(ui);
        let own = self
            .matchup
            .as_ref()
            .and_then(|m| m.own.as_ref())
            .map(|v| s(&v["hull"], "name"))
            .unwrap_or("НЕ ВЫБРАН");
        style::caption(ui, &format!("МОЙ ФИТ / {own}"));
        style::caption(ui, &self.status);
        style::caption(ui, &self.ocr_status);
    }
    fn settings_view(&mut self, ctx: &egui::Context) {
        if self.draft.is_none() || self.selecting || self.capturing {
            return;
        }
        // Изменения вида видны сразу, но Отмена должна вернуть сохранённые настройки.
        let mut draft = self.draft.take().unwrap();
        let mut response = crate::settings_ui::Response::default();
        let available = (self.monitor_bounds)()
            .map(|rect| rect.size() / ctx.pixels_per_point())
            .unwrap_or(Vec2::new(1920., 1080.));
        let builder = egui::ViewportBuilder::default()
            .with_title("Настройки · EVE Combat Assistant")
            .with_visible(true)
            .with_inner_size([
                760_f32.min(available.x - 30.),
                650_f32.min(available.y - 60.),
            ])
            .with_min_inner_size([380., 350.])
            .with_always_on_top();
        ctx.show_viewport_immediate(ViewportId::from_hash_of("settings"), builder, |ui, _| {
            let current = crate::settings_ui::Window {
                catalog: &self.catalog,
                tab: &mut self.tab,
                status: &self.status,
                syncing: self.syncing,
                cancel_sync: &self.workers.cancel,
                profile: &self.profile,
                fit_state: &mut self.fit_state,
                account_busy: self.account_busy,
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
                if !self.account_busy {
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
                if self.catalog.ship_id(&draft.ship).is_some() {
                    self.sync(draft.clone());
                } else {
                    self.status = "Выберите корпус цели".into();
                }
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
            self.request_pilot();
        }
        self.messages(ctx);
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
                    .inner_margin(18.),
            )
            .show(ui, |ui| {
                ui.set_opacity(appearance.opacity / 100.);
                let border = ui.max_rect().expand(17.);
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
        );
        let count = source.archive(start, end, &|s| println!("{s}"))?;
        println!("Добавлено: {count}");
        return Ok(());
    }
    if args.sync {
        let mut source = Sources::new(
            Store::open(dir)?,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
