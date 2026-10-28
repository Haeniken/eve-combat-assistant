//! Содержимое окна настроек. Здесь меняется черновик; сохранением занимается CombatApp.

use crate::{
    engine::Catalog,
    model::{Settings, array},
    platform,
    style::{self, AMBER, CYAN, MUTED},
};
use eframe::egui::{self, Color32, RichText, Ui};
use serde_json::Value;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Save,
    Cancel,
    CheckScreen,
    OpenDebugFolder,
    SelectRegion,
    SelectTargetsRegion,
    Sync,
    Login,
    RefreshCharacter,
    Logout,
    CalculateFit,
}

#[derive(Default)]
pub struct Response {
    pub action: Option<Action>,
    pub error: Option<String>,
}

pub struct Window<'a> {
    pub catalog: &'a Catalog,
    pub tab: &'a mut usize,
    pub status: &'a str,
    pub syncing: bool,
    pub cancel_sync: &'a AtomicBool,
    pub profile: &'a crate::account::Profile,
    pub fit_state: &'a mut FitState,
    pub account_busy: bool,
    pub fit_busy: bool,
    pub screen_state: &'a mut ScreenState,
    pub ocr_status: &'a str,
}

#[derive(Default)]
pub struct FitState {
    pub search: String,
    pub plan: Option<crate::fit_plan::Plan>,
    pub plan_key: String,
}

impl Window<'_> {
    pub fn show(&mut self, ui: &mut Ui, draft: &mut Settings) -> Response {
        let mut response = Response::default();
        let save_shortcut = ui.ctx().input_mut(|input| {
            input.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL,
                egui::Key::Enter,
            ))
        });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(9, 19, 29))
                    .inner_margin(24.),
            )
            .show(ui, |ui| {
                ui.label(RichText::new("НАСТРОЙКИ").size(22.).color(CYAN));
                ui.add_space(8.);
                ui.horizontal_wrapped(|ui| {
                    for (index, title) in [
                        "Вид",
                        "Мой корабль",
                        "Цель",
                        "Экран",
                        "Данные",
                        "Фит",
                        "EVE SSO",
                    ]
                    .iter()
                    .enumerate()
                    {
                        if ui.selectable_label(*self.tab == index, *title).clicked() {
                            *self.tab = index;
                        }
                    }
                });
                style::separator(ui);

                // Нижняя строка с кнопками остаётся доступна и при большом масштабе.
                let height = (ui.available_height() - 105.).max(100.);
                egui::ScrollArea::vertical()
                    .id_salt("settings_scroll")
                    .max_height(height)
                    .show(ui, |ui| match *self.tab {
                        0 => appearance(ui, draft),
                        1 => response.error = own_fit(ui, draft),
                        2 => target(ui, draft, self.catalog),
                        3 => {
                            response.action = screen(ui, draft, self.screen_state, self.ocr_status)
                        }
                        4 => response.action = data(ui, draft, self.syncing, self.cancel_sync),
                        5 => {
                            response.action =
                                fit_plan(ui, draft, self.catalog, self.fit_state, self.fit_busy)
                        }
                        _ => {
                            response.action =
                                account(ui, draft, self.profile, self.catalog, self.account_busy)
                        }
                    });
                ui.add_space(10.);
                style::separator(ui);
                ui.label(RichText::new(self.status).size(11.).color(MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Сохранить").on_hover_text("Ctrl+Enter").clicked() || save_shortcut
                    {
                        response.action = Some(Action::Save);
                    }
                    if ui.button("Отмена").clicked() {
                        response.action = Some(Action::Cancel);
                    }
                });
                if let Some(error) = &response.error {
                    ui.label(RichText::new(error).color(AMBER));
                }
            });
        response
    }
}

fn appearance(ui: &mut Ui, draft: &mut Settings) {
    ui.add(egui::Slider::new(&mut draft.background, 0.0..=100.).text("Непрозрачность фона, %"));
    ui.add(egui::Slider::new(&mut draft.opacity, 20.0..=100.).text("Непрозрачность окна, %"));
    ui.add(
        egui::Slider::new(&mut draft.scale, 75.0..=175.)
            .step_by(5.)
            .text("Масштаб интерфейса, %"),
    );
    ui.add_space(8.);
    style::caption(
        ui,
        "Фон 0% — прозрачная подложка. Окно 100% — чёткие текст и иконки.",
    );
    ui.checkbox(&mut draft.on_top, "Держать поверх игры");
    ui.checkbox(&mut draft.click_through, "Пропускать клики в игру");
    style::caption(ui, "Настройки доступны через значок в системном трее.");
    ui.add(
        egui::Slider::new(&mut draft.width, 380.0..=700.)
            .step_by(10.)
            .text("Ширина оверлея"),
    );
}

fn own_fit(ui: &mut Ui, draft: &mut Settings) -> Option<String> {
    let mut error = None;
    style::caption(
        ui,
        "Вставь фит из EVE / Pyfa, включая заряды и боеприпасы в трюме.",
    );
    ui.add(
        egui::TextEdit::multiline(&mut draft.own_fit)
            .id(egui::Id::unique("own_fit_editor"))
            .code_editor()
            .desired_width(f32::INFINITY)
            .desired_rows(13)
            .hint_text("[Tristan, Solo]\n…"),
    );
    ui.horizontal(|ui| {
        if ui.button("Вставить из буфера").clicked() {
            // Кнопка заменяет весь фит и сообщает об ошибке системного буфера.
            // Она не зависит от положения курсора или раскладки клавиатуры.
            match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
                Ok(text) if !text.trim().is_empty() => draft.own_fit = text.replace("\r\n", "\n"),
                Ok(_) => error = Some("В буфере обмена нет текста фита".into()),
                Err(_) => {
                    error =
                        Some("Не удалось прочитать буфер обмена. Повторите копирование фита".into())
                }
            }
        }
        if ui.button("Открыть EFT / TXT").clicked()
            && let Some(path) = platform::choose_file()
        {
            match std::fs::read_to_string(path) {
                Ok(text) => draft.own_fit = text,
                Err(cause) => error = Some(cause.to_string()),
            }
        }
        if ui.button("Пример Tristan").clicked() {
            draft.own_fit = include_str!("../resources/example-tristan.eft").into();
        }
    });
    ui.add(egui::Slider::new(&mut draft.skill_level, 0..=5).text("Уровень навыков"));
    ui.horizontal(|ui| {
        if ui.button("Импорт навыков JSON").clicked()
            && let Some(path) = platform::choose_file()
        {
            match load_skills(&path) {
                Ok(skills) => draft.skills = Some(skills),
                Err(cause) => error = Some(cause),
            }
        }
        if ui.button("Сбросить навыки").clicked() {
            draft.skills = None;
        }
    });
    style::caption(
        ui,
        if draft.skills.is_some() {
            "Используются импортированные навыки персонажа"
        } else {
            "Используется выбранный уровень всех навыков"
        },
    );
    error
}

fn target(ui: &mut Ui, draft: &mut Settings, catalog: &Catalog) {
    ui.label("Тип корабля");
    ui.text_edit_singleline(&mut draft.ship);
    let filter = draft.ship.to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("ships")
        .max_height(180.)
        .show(ui, |ui| {
            for (_, name) in catalog
                .ships
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&filter))
                .take(30)
            {
                if ui.selectable_label(*name == draft.ship, name).clicked() {
                    draft.ship = name.clone();
                }
            }
        });
    ui.label("Пилот — необязательно");
    ui.text_edit_singleline(&mut draft.pilot);
    ui.add_space(10.);
    style::caption(
        ui,
        "Основные варианты берутся с диска. Фит пилота добавляется сверху после загрузки.",
    );
}

fn screen(
    ui: &mut Ui,
    draft: &mut Settings,
    state: &mut ScreenState,
    status: &str,
) -> Option<Action> {
    ui.label(status);
    ui.checkbox(&mut draft.ocr, "Читать выбранную цель с экрана");
    ui.checkbox(
        &mut draft.pause_outside_eve,
        "Приостанавливать вне активного окна EVE",
    );
    ui.add(
        egui::Slider::new(&mut draft.ocr_interval, 0.5..=5.)
            .step_by(0.5)
            .text("Интервал чтения, сек."),
    );
    style::caption(
        ui,
        "Неизменившаяся область не отправляется на повторное OCR.",
    );
    let mut action = ui
        .button("Выделить панель текущего выбора — справа")
        .clicked()
        .then_some(Action::SelectRegion);
    if let Some(region) = &mut draft.region {
        ui.label("Координаты в пикселях");
        ui.horizontal(|ui| {
            for (index, title) in ["X", "Y", "Ширина", "Высота"].iter().enumerate() {
                ui.label(*title);
                ui.add(egui::DragValue::new(&mut region[index]).speed(1.));
            }
        });
    }
    style::separator(ui);
    if ui
        .button("Выделить карточки захваченных целей — слева")
        .clicked()
    {
        action = Some(Action::SelectTargetsRegion);
    }
    if let Some(region) = &mut draft.targets_region {
        ui.label("Область ников и корпусов со звёздочкой");
        ui.horizontal(|ui| {
            for (index, title) in ["X", "Y", "Ширина", "Высота"].iter().enumerate() {
                ui.label(*title);
                ui.add(egui::DragValue::new(&mut region[index]).speed(1.));
            }
        });
        if ui.small_button("Отключить вторую область").clicked() {
            draft.targets_region = None;
        }
    }
    egui::ComboBox::from_id_salt("ocr_lang")
        .selected_text(if draft.language == "rus" {
            "Русский"
        } else {
            "English"
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut draft.language, "eng".into(), "English");
            ui.selectable_value(&mut draft.language, "rus".into(), "Русский");
        });
    style::caption(
        ui,
        "Справа читается ник текущего выбора. Если корпус там не виден, выделите слева всю полосу ников и названий со звёздочкой, например Orca*. Позиции карточек могут меняться; изображения и анимированные кольца в область включать не нужно.",
    );
    style::separator(ui);
    ui.checkbox(
        &mut draft.ocr_debug,
        "Расширенное логирование OCR (текст и снимки областей)",
    );
    style::caption(
        ui,
        "Журнал включает координаты, язык, паузы, время обработки, текст, позиции слов и результат выбора корпуса. До 80 снимков / 64 МиБ и 4 журналов по 4 МиБ. Настройка применяется после сохранения; ручная проверка использует галочку сразу.",
    );
    if ui.button("Открыть папку диагностики").clicked() {
        action = Some(Action::OpenDebugFolder);
    }
    if ui
        .add_enabled(!state.busy, egui::Button::new("Проверить области сейчас"))
        .clicked()
    {
        action = Some(Action::CheckScreen);
    }
    style::caption(
        ui,
        "Проверка использует координаты и язык выше без сохранения. Окна помощника на время захвата скрываются. При расширенном логировании снимки также сохраняются в папку диагностики.",
    );
    if state.busy {
        ui.label("Проверка экрана…");
    }
    if let Some(check) = &state.check {
        if check.selected.rect != draft.region
            || check.cards.rect != draft.targets_region
            || state.language != draft.language
        {
            ui.colored_label(AMBER, "Области или язык изменены — повторите проверку.");
        }
        if state.scroll_to_result {
            ui.scroll_to_cursor(Some(egui::Align::TOP));
            state.scroll_to_result = false;
        }
        match &check.result {
            Ok(target) => {
                ui.colored_label(CYAN, format!("Найдено: {} · {}", target.pilot, target.ship));
                let matched =
                    crate::ocr::pilot_match(&check.selected.text, &check.selected.retries, target);
                if matched.method == "closest" {
                    style::caption(
                        ui,
                        &format!(
                            "Выбрана ближайшая карточка по нику; различий: {}",
                            matched.edit_distance
                        ),
                    );
                }
            }
            Err(error) => {
                ui.colored_label(AMBER, error);
            }
        }
        for (index, title, region) in [
            (0, "Текущий выбор — справа", &check.selected),
            (1, "Карточки — слева", &check.cards),
        ] {
            ui.label(title);
            if let Some(rect) = region.rect {
                ui.monospace(format!(
                    "X={} Y={} · {} × {} px",
                    rect[0], rect[1], rect[2], rect[3]
                ));
            } else {
                style::caption(ui, "Область не выбрана");
            }
            if let Some(texture) = &state.textures[index] {
                ui.add(
                    egui::Image::new(texture)
                        .max_width(ui.available_width())
                        .max_height(220.)
                        .shrink_to_fit(),
                );
            }
            for retry in &region.retries {
                style::caption(
                    ui,
                    &format!(
                        "Повтор строки ника ({}): {}",
                        retry.method,
                        retry.error.as_ref().unwrap_or(&retry.text)
                    ),
                );
            }
            if let Some(error) = &region.error {
                ui.colored_label(AMBER, error);
            }
            if region.rect.is_some() {
                let mut text = if region.text.is_empty() {
                    "(OCR не вернул текст)".into()
                } else {
                    region.text.clone()
                };
                ui.add(
                    egui::TextEdit::multiline(&mut text)
                        .desired_width(f32::INFINITY)
                        .desired_rows(3)
                        .interactive(false),
                );
            }
        }
    }
    action
}

#[derive(Default)]
pub struct ScreenState {
    pub busy: bool,
    pub check: Option<crate::ocr::ScreenCheck>,
    pub textures: [Option<egui::TextureHandle>; 2],
    pub language: String,
    pub scroll_to_result: bool,
}
impl ScreenState {
    pub fn set(&mut self, ctx: &egui::Context, mut check: crate::ocr::ScreenCheck) {
        self.busy = false;
        self.scroll_to_result = true;
        for (index, region) in [&mut check.selected, &mut check.cards]
            .into_iter()
            .enumerate()
        {
            self.textures[index] = region.image.take().map(|image| {
                ctx.load_texture(
                    format!("screen-check-{index}"),
                    egui::ColorImage::from_rgba_unmultiplied(
                        [image.width() as usize, image.height() as usize],
                        image.as_raw(),
                    ),
                    Default::default(),
                )
            });
        }
        self.check = Some(check);
    }
}

fn data(
    ui: &mut Ui,
    draft: &mut Settings,
    syncing: bool,
    cancel_sync: &AtomicBool,
) -> Option<Action> {
    let mut action = None;
    egui::ComboBox::from_id_salt("period")
        .selected_text(if draft.days == 0 {
            "Весь архив".into()
        } else {
            format!("{} дней", draft.days)
        })
        .show_ui(ui, |ui| {
            for (days, title) in [
                (30, "30 дней"),
                (90, "90 дней"),
                (180, "180 дней"),
                (0, "Весь архив"),
            ] {
                ui.selectable_value(&mut draft.days, days, title);
            }
        });
    ui.add(egui::Slider::new(&mut draft.max_variants, 1..=12).text("Максимум вариантов"));
    ui.add(egui::Slider::new(&mut draft.coverage, 50.0..=100.).text("Покрытие выборки, %"));
    ui.add(egui::Slider::new(&mut draft.min_percent, 0.0..=20.).text("Минимальная доля, %"));
    ui.add(egui::Slider::new(&mut draft.max_attackers, 1..=5).text("Максимум атакующих"));
    ui.add(
        egui::Slider::new(&mut draft.pilot_days, 1..=180).text("Период поиска фита пилота, дней"),
    );
    ui.checkbox(
        &mut draft.auto_pilot_lookup,
        "Дозагружать сведения выбранного пилота",
    );
    style::caption(
        ui,
        "Общая статистика обновляется только вручную. Во время игры используется локальная база.",
    );
    egui::ComboBox::from_id_salt("sync_scope")
        .selected_text(match draft.sync_scope {
            1 => "Все загруженные корпуса",
            2 => "Все корпуса — дневной архив",
            _ => "Выбранный корпус цели",
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut draft.sync_scope, 0, "Выбранный корпус цели");
            ui.selectable_value(&mut draft.sync_scope, 1, "Все загруженные корпуса");
            ui.selectable_value(&mut draft.sync_scope, 2, "Все корпуса — дневной архив");
        });
    ui.checkbox(
        &mut draft.force_refresh,
        "Принудительно обновить, обходя локальный кеш статистики",
    );
    if draft.sync_scope == 2 {
        style::caption(
            ui,
            "Дневной архив загружает все корпуса за выбранный период, до вчерашнего дня. Большой период может занять много времени. Загрузку можно остановить и продолжить.",
        );
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!syncing, egui::Button::new("Обновить статистику сейчас"))
            .clicked()
        {
            action = Some(Action::Sync);
        }
        if syncing && ui.button("Остановить").clicked() {
            cancel_sync.store(true, Ordering::Relaxed);
        }
    });
    if ui
        .button(format!("Сбросить исключения ({})", draft.excluded.len()))
        .clicked()
    {
        draft.excluded.clear();
    }
    action
}

fn account(
    ui: &mut Ui,
    draft: &mut Settings,
    profile: &crate::account::Profile,
    catalog: &Catalog,
    busy: bool,
) -> Option<Action> {
    let mut action = None;
    ui.label(RichText::new("ПЕРСОНАЖ").color(CYAN));
    if profile.character_id > 0 {
        ui.label(format!(
            "{} · {} навыков",
            profile.name,
            profile.skills.len()
        ));
        ui.label(format!(
            "Корабль: {}",
            catalog.name(crate::model::id(&profile.ship, "ship_type_id") as i32)
        ));
    }
    ui.label("Client ID приложения EVE SSO");
    ui.text_edit_singleline(&mut draft.sso_client_id);
    ui.collapsing("Как подключить", |ui| {
        ui.hyperlink_to(
            "Зарегистрировать приложение",
            "https://developers.eveonline.com/applications",
        );
        ui.label("Тип: Authentication & API Access. Используется вход PKCE без Client Secret.");
        ui.label("Callback URL:");
        ui.monospace(crate::account::CALLBACK);
        ui.label("Разрешения:");
        ui.monospace(crate::account::SCOPES.replace(' ', "\n"));
        style::caption(
            ui,
            "Скопируйте Client ID в поле выше. Пароль вводится только на сайте EVE.",
        );
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!busy, egui::Button::new("Войти через EVE Online"))
            .clicked()
        {
            action = Some(Action::Login);
        }
        if ui
            .add_enabled(
                !busy && profile.character_id > 0,
                egui::Button::new("Обновить персонажа"),
            )
            .clicked()
        {
            action = Some(Action::RefreshCharacter);
        }
        if ui
            .add_enabled(
                !busy && profile.character_id > 0,
                egui::Button::new("Выйти"),
            )
            .clicked()
        {
            action = Some(Action::Logout);
        }
    });
    if busy {
        ui.label("Ожидание EVE API…");
    }
    style::caption(
        ui,
        "Навыки и импланты загружаются по кнопке. Модули и карго текущего корабля читаются из assets с задержкой ESI; при отсутствии данных остаётся ручной EFT.",
    );
    if let Some(eft) = &profile.current_fit
        && ui.button("Взять фит текущего корабля из ESI").clicked()
    {
        draft.own_fit = eft.clone();
    }
    if !profile.fittings.is_empty() {
        ui.collapsing("Сохранённые фиты EVE", |ui| {
            for fit in &profile.fittings {
                if ui.button(crate::model::s(fit, "name")).clicked() {
                    match crate::account::items_to_eft(
                        crate::model::id(fit, "ship_type_id") as i32,
                        crate::model::s(fit, "name"),
                        array(&fit["items"]),
                        catalog,
                    ) {
                        Ok(text) => draft.own_fit = text,
                        Err(error) => {
                            ui.label(error);
                        }
                    }
                }
            }
        });
    }
    for warning in &profile.warnings {
        ui.label(RichText::new(warning).color(AMBER));
    }
    action
}

fn fit_plan(
    ui: &mut Ui,
    draft: &mut Settings,
    catalog: &Catalog,
    state: &mut FitState,
    busy: bool,
) -> Option<Action> {
    let mut action = None;
    style::caption(
        ui,
        "Импланты установлены в клоне; запасные импланты можно добавить в план. Бустер учитывается в расчёте только после выбора.",
    );
    for booster in [false, true] {
        ui.label(
            RichText::new(if booster {
                "ВЫБРАННЫЕ БУСТЕРЫ"
            } else {
                "ИМПЛАНТЫ"
            })
            .color(CYAN),
        );
        let types = if booster {
            &mut draft.boosters
        } else {
            &mut draft.implants
        };
        let mut remove = None;
        for tid in types.iter() {
            ui.horizontal_wrapped(|ui| {
                ui.label(format!(
                    "{} · слот {}",
                    catalog.name(*tid),
                    crate::fit_plan::slot(catalog, *tid, booster)
                ));
                if ui.small_button("×").clicked() {
                    remove = Some(*tid);
                }
            });
        }
        if let Some(tid) = remove {
            types.retain(|id| *id != tid);
        }
        if types.is_empty() {
            style::caption(ui, "Не выбраны");
        }
    }
    for tid in &draft.boosters {
        for effect in array(&catalog.get(*tid)["side_effects"]) {
            let eid = crate::model::id(effect, "id") as i32;
            let mut enabled = draft.booster_effects.contains(&eid);
            if ui
                .checkbox(
                    &mut enabled,
                    format!("Побочный эффект: {}", crate::model::s(effect, "name")),
                )
                .changed()
            {
                if enabled {
                    draft.booster_effects.push(eid);
                } else {
                    draft.booster_effects.retain(|id| *id != eid);
                }
            }
        }
    }
    ui.label("Добавить имплант или бустер по названию");
    ui.text_edit_singleline(&mut state.search);
    if state.search.chars().count() >= 3 {
        let search = state.search.to_lowercase();
        let mut matches: Vec<_> = catalog
            .types
            .iter()
            .filter(|(_, t)| {
                t["category"] == 20 && crate::model::s(t, "name").to_lowercase().contains(&search)
            })
            .collect();
        matches.sort_by_key(|(_, t)| crate::model::s(t, "name"));
        for (tid, t) in matches.into_iter().take(12) {
            if ui
                .small_button(format!("+ {}", crate::model::s(t, "name")))
                .clicked()
            {
                let booster = crate::fit_plan::slot(catalog, *tid, true) > 0;
                let types = if booster {
                    &mut draft.boosters
                } else {
                    &mut draft.implants
                };
                crate::fit_plan::select(types, *tid, booster, catalog);
            }
        }
    }
    style::separator(ui);
    if ui
        .add_enabled(!busy, egui::Button::new("Рассчитать фит и рекомендации"))
        .clicked()
    {
        action = Some(Action::CalculateFit);
    }
    if busy {
        ui.label("Расчёт…");
    }
    if let Some(plan) = &state.plan {
        let current_key = draft
            .own_request(serde_json::json!(draft.own_fit), true)
            .to_string();
        if state.plan_key != current_key {
            style::caption(ui, "Фит или профиль изменён: пересчитайте рекомендации.");
            return action;
        }
        ui.label(format!(
            "{} · {:.0} DPS · {:.0} м/с",
            crate::model::s(&plan.result["hull"], "name"),
            crate::model::n(&plan.result["summary"]["offense"], "dps"),
            crate::model::n(&plan.result["hull"]["attributes"], "maxVelocity")
        ));
        for warning in &plan.warnings {
            ui.label(RichText::new(warning).color(AMBER));
        }
        ui.label("Бустеры в карго");
        if plan.cargo_boosters.is_empty() {
            style::caption(
                ui,
                "Нет в импортированном карго. Можно добавить вручную через поиск выше.",
            );
        }
        for tid in &plan.cargo_boosters {
            let mut enabled = draft.boosters.contains(tid);
            if ui.checkbox(&mut enabled, catalog.name(*tid)).changed() {
                if enabled {
                    crate::fit_plan::select(&mut draft.boosters, *tid, true, catalog);
                } else {
                    draft.boosters.retain(|id| id != tid);
                }
            }
        }
        ui.label(RichText::new("ЧТО ВЗЯТЬ С СОБОЙ").color(CYAN));
        style::caption(
            ui,
            "Каждый вариант сравнивается с текущим набором отдельно; предмет того же слота заменяется. Имплант надо установить в клона до вылета. Подбор ограничен распространёнными PvP-вариантами и не учитывает цены.",
        );
        for suggestion in &plan.suggestions {
            ui.horizontal_wrapped(|ui| {
                if ui.small_button("В план").clicked() {
                    let types = if suggestion.booster {
                        &mut draft.boosters
                    } else {
                        &mut draft.implants
                    };
                    crate::fit_plan::select(types, suggestion.type_id, suggestion.booster, catalog);
                }
                ui.label(catalog.name(suggestion.type_id));
            });
            style::caption(ui, &suggestion.benefit);
        }
        if plan.suggestions.is_empty() {
            style::caption(ui, "Среди проверенных вариантов улучшений нет.");
        }
    }
    action
}

fn load_skills(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut value: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| error.to_string())?;
    // Принимаем и ответ ESI, и простой объект «type_id: уровень» для ручного экспорта.
    if value.get("skills").is_some() {
        value = Value::Object(
            array(&value["skills"])
                .iter()
                .map(|skill| {
                    (
                        skill["skill_id"].to_string(),
                        skill["active_skill_level"].clone(),
                    )
                })
                .collect(),
        );
    }
    let skills = value
        .as_object()
        .ok_or("Ожидается JSON навыков ESI или объект skill_id → уровень")?;
    if skills.iter().any(|(id, level)| {
        id.parse::<i32>().is_err() || level.as_u64().is_none_or(|level| level > 5)
    }) {
        return Err("Неверный формат навыков".into());
    }
    Ok(value)
}
