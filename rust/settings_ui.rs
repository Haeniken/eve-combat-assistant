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
    SyncSolo,
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
    pub dirty: bool,
    pub status: &'a str,
    pub cache_status: &'a str,
    pub sync_progress: Option<&'a crate::sources::UpdateProgress>,
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
        let save_shortcut = ui.ctx().input_mut(|i| {
            i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::CTRL,
                egui::Key::Enter,
            ))
        });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(9, 21, 30))
                    .inner_margin(0.),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(8., 5.);
                ui.spacing_mut().button_padding = egui::vec2(10., 6.);
                ui.spacing_mut().interact_size.y = 28.;
                ui.style_mut().override_font_id = Some(egui::FontId::proportional(12.));
                let visuals = &mut ui.style_mut().visuals;
                visuals.extreme_bg_color = Color32::from_rgb(10, 29, 40);
                for widget in [
                    &mut visuals.widgets.inactive,
                    &mut visuals.widgets.noninteractive,
                ] {
                    widget.bg_fill = Color32::from_rgb(13, 32, 44);
                    widget.weak_bg_fill = Color32::from_rgb(13, 32, 44);
                    widget.bg_stroke = egui::Stroke::new(1., style::LINE);
                    widget.corner_radius = egui::CornerRadius::same(2);
                }
                let bounds = ui.max_rect();
                let header = 64.;
                let footer = 58.;
                let nav = if bounds.width() < 600. { 112. } else { 150. };
                let top = egui::Rect::from_min_max(
                    bounds.min,
                    bounds.left_top() + egui::vec2(bounds.width(), header),
                );
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(top.shrink2(egui::vec2(20., 10.))),
                    |ui| {
                        ui.label(RichText::new("НАСТРОЙКИ").size(21.).color(CYAN));
                        style::caption(ui, "EVE COMBAT ASSISTANT");
                    },
                );
                ui.painter().hline(
                    bounds.x_range(),
                    bounds.top() + header,
                    egui::Stroke::new(1., style::LINE),
                );
                let side = egui::Rect::from_min_max(
                    bounds.left_top() + egui::vec2(0., header),
                    egui::pos2(bounds.left() + nav, bounds.bottom() - footer),
                );
                ui.painter()
                    .rect_filled(side, 0., Color32::from_rgb(10, 25, 35));
                ui.painter().vline(
                    side.right(),
                    side.y_range(),
                    egui::Stroke::new(1., style::LINE),
                );
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(side.shrink2(egui::vec2(9., 16.))),
                    |ui| {
                        ui.set_clip_rect(side.intersect(ui.clip_rect()));
                        egui::ScrollArea::vertical()
                            .id_salt("settings-nav")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                for (i, title) in [
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
                                    if style::settings_nav(ui, i, title, *self.tab == i).clicked() {
                                        *self.tab = i;
                                    }
                                }
                            });
                    },
                );
                let body = egui::Rect::from_min_max(
                    bounds.left_top() + egui::vec2(nav + 20., header + 14.),
                    bounds.right_bottom() - egui::vec2(20., footer + 12.),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                    ui.set_clip_rect(body.intersect(ui.clip_rect()));
                    egui::ScrollArea::vertical()
                        .id_salt(("settings_scroll", *self.tab))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let (title, subtitle) = match *self.tab {
                                0 => ("Внешний вид", "Размер, прозрачность и поведение оверлея"),
                                1 => ("Мой корабль", "Фит, заряды и навыки для расчёта"),
                                2 => ("Цель", "Ручной выбор корабля и пилота"),
                                3 => ("Распознавание", "Области экрана и диагностика"),
                                4 => ("Данные", "Локальная база и правила отбора сборок"),
                                5 => ("Фит", "Импланты, бустеры и подготовка к вылету"),
                                _ => ("Персонаж EVE", "Навыки, корабль и сохранённые фиты"),
                            };
                            ui.label(RichText::new(title).size(20.).color(style::TEXT));
                            style::caption(ui, subtitle);
                            style::separator(ui);
                            match *self.tab {
                                0 => appearance(ui, draft),
                                1 => response.error = own_fit(ui, draft),
                                2 => {
                                    target(ui, draft, self.catalog);
                                }
                                3 => {
                                    response.action =
                                        screen(ui, draft, self.screen_state, self.ocr_status)
                                }
                                4 => {
                                    response.action = data(
                                        ui,
                                        draft,
                                        self.syncing,
                                        self.cancel_sync,
                                        self.cache_status,
                                        self.sync_progress,
                                    )
                                }
                                5 => {
                                    response.action = fit_plan(
                                        ui,
                                        draft,
                                        self.catalog,
                                        self.fit_state,
                                        self.fit_busy,
                                    )
                                }
                                _ => {
                                    response.action = account(
                                        ui,
                                        draft,
                                        self.profile,
                                        self.catalog,
                                        self.account_busy,
                                    )
                                }
                            }
                        });
                });
                let bottom = egui::Rect::from_min_max(
                    egui::pos2(bounds.left(), bounds.bottom() - footer),
                    bounds.max,
                );
                ui.painter()
                    .rect_filled(bottom, 0., Color32::from_rgb(10, 25, 35));
                ui.painter().hline(
                    bottom.x_range(),
                    bottom.top(),
                    egui::Stroke::new(1., style::LINE),
                );
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
                        egui::pos2(body.left(), bottom.top() + 8.),
                        bottom.right_bottom() - egui::vec2(20., 8.),
                    )),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_sized(
                                            [108., 28.],
                                            egui::Button::new(
                                                RichText::new("Сохранить").color(CYAN),
                                            )
                                            .fill(Color32::from_rgb(22, 58, 72)),
                                        )
                                        .clicked()
                                        || save_shortcut
                                    {
                                        response.action = Some(Action::Save);
                                    }
                                    if ui
                                        .add_sized([85., 28.], egui::Button::new("Отмена"))
                                        .clicked()
                                    {
                                        response.action = Some(Action::Cancel);
                                    }
                                    let (message, color) = if let Some(e) = &response.error {
                                        (e.as_str(), AMBER)
                                    } else if self.dirty {
                                        ("Изменения не сохранены", MUTED)
                                    } else {
                                        ("Нет изменений", MUTED)
                                    };
                                    ui.allocate_ui_with_layout(
                                        egui::vec2(ui.available_width(), 28.),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(message).size(10.).color(color),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(
                                                response.error.as_deref().unwrap_or(self.status),
                                            );
                                        },
                                    );
                                },
                            );
                        });
                        ui.label(
                            RichText::new("Ctrl+Enter — сохранить")
                                .size(9.)
                                .color(MUTED),
                        );
                    },
                );
            });
        response
    }
}
fn section(ui: &mut Ui, title: &str) {
    ui.add_space(3.);
    style::heading(ui, title);
    style::separator(ui);
}
fn setting_row(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui)) {
    if ui.available_width() >= 420. {
        ui.horizontal(|ui| {
            let label_width = ui.available_width() * 0.43;
            ui.allocate_ui_with_layout(
                egui::vec2(label_width, 28.),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_width(label_width);
                    ui.label(label);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 28.),
                egui::Layout::left_to_right(egui::Align::Center),
                control,
            );
        });
    } else {
        ui.label(label);
        control(ui);
    }
}

fn slider(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    suffix: &str,
    step: f64,
) {
    setting_row(ui, label, |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 70.).max(60.);
        ui.visuals_mut().widgets.inactive.fg_stroke = egui::Stroke::new(1., CYAN);
        ui.visuals_mut().selection.bg_fill = Color32::from_rgb(70, 139, 158);
        ui.add(
            egui::Slider::new(value, range)
                .suffix(suffix)
                .step_by(step)
                .handle_shape(egui::style::HandleShape::Circle)
                .trailing_fill(true),
        );
    });
}
fn number<N: egui::emath::Numeric>(
    ui: &mut Ui,
    label: &str,
    value: &mut N,
    range: std::ops::RangeInclusive<N>,
    suffix: &str,
) {
    ui.label(RichText::new(label).size(11.));
    ui.add_sized(
        [ui.available_width(), 28.],
        egui::DragValue::new(value).range(range).suffix(suffix),
    );
}
fn appearance(ui: &mut Ui, draft: &mut Settings) {
    section(ui, "ОФОРМЛЕНИЕ");
    slider(
        ui,
        "Непрозрачность фона",
        &mut draft.background,
        0.0..=100.,
        "%",
        1.,
    );
    slider(
        ui,
        "Непрозрачность окна",
        &mut draft.opacity,
        20.0..=100.,
        "%",
        1.,
    );
    slider(
        ui,
        "Масштаб интерфейса",
        &mut draft.scale,
        75.0..=175.,
        "%",
        5.,
    );
    style::caption(
        ui,
        "Изменения вида сразу видны в оверлее. «Отмена» вернёт прежний вид.",
    );
    section(ui, "РАЗМЕР ОКНА");
    slider(
        ui,
        "Ширина оверлея",
        &mut draft.width,
        380.0..=700.,
        " px",
        10.,
    );
    section(ui, "ПОВЕДЕНИЕ");
    ui.checkbox(&mut draft.on_top, "Держать поверх игры");
    ui.checkbox(&mut draft.click_through, "Пропускать клики в игру");
    style::caption(ui, "Настройки доступны через значок в системном трее.");
    section(ui, "ЗВУКОВЫЕ СИГНАЛЫ");
    ui.checkbox(&mut draft.sound_tackle, "Вход в дальность скрама или сетки");
    ui.checkbox(&mut draft.sound_neut, "Вход в дальность нейтрализатора");
    ui.checkbox(
        &mut draft.sound_point,
        "Выход из дальности своего поинта / скрама",
    );
}

fn own_fit(ui: &mut Ui, draft: &mut Settings) -> Option<String> {
    let mut error = None;
    style::caption(
        ui,
        "Вставь фит из EVE / Pyfa, включая заряды и боеприпасы в трюме.",
    );
    ui.horizontal_wrapped(|ui| {
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
    egui::ScrollArea::vertical()
        .id_salt("eft-editor")
        .max_height(180.)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::multiline(&mut draft.own_fit)
                    .id(egui::Id::unique("own_fit_editor"))
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(10)
                    .hint_text("[Tristan, Solo]\n…"),
            );
        });
    section(ui, "НАВЫКИ");
    setting_row(ui, "Уровень всех навыков", |ui| {
        ui.spacing_mut().slider_width = (ui.available_width() - 65.).max(60.);
        ui.add_enabled(
            draft.skills.is_none(),
            egui::Slider::new(&mut draft.skill_level, 0..=5),
        );
    });
    ui.horizontal_wrapped(|ui| {
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

fn target(ui: &mut Ui, draft: &mut Settings, catalog: &Catalog) -> egui::Response {
    ui.horizontal(|ui| {
        ui.label("Источник цели");
        ui.radio_value(&mut draft.ocr, true, "Экран");
        ui.radio_value(&mut draft.ocr, false, "Вручную");
    });
    section(ui, "КОРАБЛЬ");
    let query_id = ui.make_persistent_id("target_ship_query");
    let popup = egui::ComboBox::from_id_salt("target_ship")
        .selected_text(&draft.ship)
        .width(ui.available_width())
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let previous = ui.data_mut(|data| data.get_temp::<String>(query_id));
            let mut query = previous.clone().unwrap_or_default();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut query)
                    .id(query_id)
                    .desired_width(f32::INFINITY)
                    .hint_text("Название корабля…"),
            );
            if previous.is_none() {
                edit.request_focus();
            }
            ui.data_mut(|data| data.insert_temp(query_id, query.clone()));
            let filter = query.trim().to_lowercase();
            let matches: Vec<_> = catalog
                .ships
                .iter()
                .filter(|(_, name)| name.to_lowercase().contains(&filter))
                .collect();
            if (edit.has_focus() || edit.lost_focus())
                && ui.input(|input| input.key_pressed(egui::Key::Enter))
            {
                let chosen = matches
                    .iter()
                    .find(|(_, name)| name.eq_ignore_ascii_case(query.trim()))
                    .or_else(|| {
                        if matches.len() == 1 {
                            matches.first()
                        } else {
                            None
                        }
                    });
                if let Some((_, name)) = chosen {
                    draft.ship = name.clone();
                    draft.ocr = false;
                    ui.close();
                } else {
                    edit.request_focus();
                }
            }
            egui::ScrollArea::vertical()
                .id_salt("target_ship_results")
                .max_height(180.)
                .show(ui, |ui| {
                    if matches.is_empty() {
                        style::caption(ui, "Корабль не найден");
                    }
                    for (_, name) in matches {
                        if ui.selectable_label(*name == draft.ship, name).clicked() {
                            draft.ship = name.clone();
                            draft.ocr = false;
                            ui.close();
                        }
                    }
                });
        });
    if popup.inner.is_none() {
        ui.data_mut(|data| data.remove::<String>(query_id));
    }
    style::caption(ui, "Введите название, чтобы найти корпус в каталоге.");
    section(ui, "ПИЛОТ");
    ui.label("Пилот — необязательно");
    if ui
        .add(
            egui::TextEdit::singleline(&mut draft.pilot)
                .id(ui.make_persistent_id("target_pilot"))
                .desired_width(f32::INFINITY),
        )
        .changed()
    {
        draft.ocr = false;
    }
    ui.add_space(10.);
    style::caption(
        ui,
        "После сохранения ручной цели расчёт начнётся по локальной базе. Фит пилота добавится после загрузки.",
    );
    popup.response
}

fn screen(
    ui: &mut Ui,
    draft: &mut Settings,
    state: &mut ScreenState,
    status: &str,
) -> Option<Action> {
    let mut action = None;
    ui.checkbox(&mut draft.ocr, "Распознавать выбранную цель с экрана");
    ui.checkbox(
        &mut draft.pause_outside_eve,
        "Пауза, когда окно EVE неактивно",
    );
    setting_row(ui, "Интервал чтения", |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::DragValue::new(&mut draft.ocr_interval)
                    .range(0.5..=5.)
                    .speed(0.5)
                    .suffix(" сек."),
            );
        });
    });
    setting_row(ui, "Хранить исчезнувшую цель", |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::DragValue::new(&mut draft.target_hold_seconds)
                    .range(0..=600)
                    .suffix(" сек."),
            );
        });
    });
    style::caption(
        ui,
        "При смене цели старые данные снимаются сразу. 0 — сбрасывать без задержки.",
    );
    section(ui, "ОБЛАСТИ ЭКРАНА");
    for (title, rect, kind) in [
        ("Текущий выбор — справа", draft.region, Action::SelectRegion),
        (
            "Карточки целей — слева",
            draft.targets_region,
            Action::SelectTargetsRegion,
        ),
    ] {
        egui::Frame::new()
            .fill(Color32::from_rgb(11, 29, 40))
            .stroke(egui::Stroke::new(1., style::LINE))
            .inner_margin(8.)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(title);
                        style::caption(
                            ui,
                            &rect
                                .map(|r| {
                                    format!("X {} · Y {} · {} × {} px", r[0], r[1], r[2], r[3])
                                })
                                .unwrap_or_else(|| "Область не выбрана".into()),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Выделить").clicked() {
                            action = Some(kind);
                        }
                    });
                });
            });
    }
    ui.collapsing("Координаты и язык",|ui| {
        for (name,region) in [("Текущий выбор",&mut draft.region),("Карточки",&mut draft.targets_region)] {
            if let Some(rect)=region {ui.label(name);ui.horizontal_wrapped(|ui| {for (i,label) in ["X","Y","Ширина","Высота"].iter().enumerate(){ui.label(*label);ui.add(egui::DragValue::new(&mut rect[i]));}});}
        }
        if draft.targets_region.is_some()&&ui.small_button("Отключить вторую область").clicked(){draft.targets_region=None;}
        egui::ComboBox::from_id_salt("ocr_lang").selected_text(if draft.language=="rus"{"Русский"}else{"English"}).show_ui(ui,|ui| {ui.selectable_value(&mut draft.language,"eng".into(),"English");ui.selectable_value(&mut draft.language,"rus".into(),"Русский");});
        style::caption(ui,"Справа — ник выбора. Слева — ники и названия корпусов без анимированных колец. Звёздочка необязательна.");
    });
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(!state.busy, egui::Button::new("Проверить сейчас"))
            .clicked()
        {
            action = Some(Action::CheckScreen);
        }
        ui.add(egui::Label::new(RichText::new(status).size(10.).color(MUTED)).truncate())
            .on_hover_text(status);
    });
    ui.horizontal_wrapped(|ui| {
        ui.checkbox(&mut draft.ocr_debug, "Подробная диагностика").on_hover_text("Снимки, OCR, фиты, кеш и трассировка запросов. До 512 снимков / 64 МиБ; журналы имеют ограничение размера.");
        if ui.button("Открыть папку").clicked() {
            action = Some(Action::OpenDebugFolder);
        }
    });
    if state.busy {
        ui.label("Проверка экрана…");
    }
    if state.check.is_some() {
        egui::CollapsingHeader::new("Результат проверки")
            .open(state.scroll_to_result.then_some(true))
            .show(ui, |ui| {
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
                            ui.colored_label(
                                CYAN,
                                format!("Найдено: {} · {}", target.pilot, target.ship),
                            );
                            let matched = crate::ocr::pilot_match(
                                &check.selected.parsed_text,
                                &check.selected.retries,
                                target,
                            );
                            if let Some(distance) = target.distance {
                                style::caption(ui, &format!("Дистанция: {distance:.0} м"));
                            } else {
                                style::caption(ui, "Дистанция: не прочитана");
                            }
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
                            let label = if retry.method.starts_with("distance_") {
                                "Повтор дистанции"
                            } else if retry.method.starts_with("cards_") {
                                "Повтор карточек"
                            } else {
                                "Повтор ника"
                            };
                            style::caption(
                                ui,
                                &format!(
                                    "{label} ({}): {}",
                                    retry.method,
                                    retry.error.as_ref().unwrap_or(&retry.text)
                                ),
                            );
                        }
                        if !region.parsed_text.is_empty() && region.parsed_text != region.text {
                            style::caption(
                                ui,
                                &format!("Для сопоставления: {}", region.parsed_text),
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
            });
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
    cache_status: &str,
    progress: Option<&crate::sources::UpdateProgress>,
) -> Option<Action> {
    let mut action = None;
    section(ui, "ОБНОВЛЕНИЕ БАЗЫ");
    style::caption(ui, cache_status);
    if let Some(p) = progress {
        ui.add(
            egui::ProgressBar::new(p.fraction().unwrap_or(0.))
                .animate(p.fraction().is_none())
                .text(
                    p.fraction()
                        .filter(|_| p.total_days > 0)
                        .map(|f| {
                            format!(
                                "{} / {} дней · {:.0}%",
                                p.completed_days,
                                p.total_days,
                                f * 100.
                            )
                        })
                        .unwrap_or_else(|| p.phase.clone()),
                )
                .desired_width(ui.available_width()),
        );
        style::caption(ui, &p.caption());
        if let Some(error) = &p.error {
            ui.colored_label(AMBER, error);
        }
    }
    if syncing {
        if ui.button("Остановить").clicked() {
            cancel_sync.store(true, Ordering::Relaxed);
        }
        if cancel_sync.load(Ordering::Relaxed) {
            style::caption(ui, "Остановка обновления…");
        }
    } else if ui.button("Обновить базу всех кораблей").clicked() {
        action = Some(Action::Sync);
    }
    if !syncing {
        let confirmation = egui::Id::unique("solo-model-confirmation");
        if ui.button("Обновить SOLO-рейтинг").clicked() {
            ui.memory_mut(|m| m.data.insert_temp(confirmation, true));
        }
        if ui.memory(|m| m.data.get_temp::<bool>(confirmation).unwrap_or(false)) {
            egui::Frame::new().stroke(egui::Stroke::new(1.,AMBER)).inner_margin(10.).show(ui,|ui|{
                ui.colored_label(AMBER,"Длительная операция");
                ui.label("Загрузится недостающая история за пять лет и пересчитаются рейтинги всех пилотов. Это может занять несколько часов и потребовать значительного объёма загрузки. Операцию можно остановить: уже загруженные дни сохранятся для продолжения.");
                ui.horizontal(|ui|{
                    if ui.button("Начать обновление SOLO").clicked(){action=Some(Action::SyncSolo);ui.memory_mut(|m|m.data.insert_temp(confirmation,false));}
                    if ui.button("Отмена").clicked(){ui.memory_mut(|m|m.data.insert_temp(confirmation,false));}
                });
            });
        }
    }
    section(ui, "ВЫБОРКА СБОРОК");
    ui.columns(if ui.available_width() < 380. { 1 } else { 2 }, |cols| {
        let n = cols.len();
        cols[0].label("Период статистики");
        egui::ComboBox::from_id_salt("period")
            .width(cols[0].available_width())
            .selected_text(if draft.days == 0 {
                "Весь архив".into()
            } else {
                format!("{} дней", draft.days)
            })
            .show_ui(&mut cols[0], |ui| {
                for (days, label) in [
                    (30, "30 дней"),
                    (90, "90 дней"),
                    (180, "180 дней"),
                    (0, "Весь архив"),
                ] {
                    ui.selectable_value(&mut draft.days, days, label);
                }
            });
        number(
            &mut cols[1 % n],
            "Максимум вариантов",
            &mut draft.max_variants,
            1..=12,
            "",
        );
        number(
            &mut cols[0],
            "Покрытие выборки",
            &mut draft.coverage,
            50.0..=100.,
            "%",
        );
        number(
            &mut cols[1 % n],
            "Минимальная доля",
            &mut draft.min_percent,
            0.0..=20.,
            "%",
        );
        number(
            &mut cols[0],
            "Максимум атакующих",
            &mut draft.max_attackers,
            1..=5,
            "",
        );
        number(
            &mut cols[1 % n],
            "Поиск фита пилота",
            &mut draft.pilot_days,
            1..=180,
            " дней",
        );
    });
    ui.add_space(5.);
    ui.checkbox(
        &mut draft.auto_pilot_lookup,
        "Дозагружать сведения выбранного пилота",
    );
    ui.checkbox(
        &mut draft.pilot_rating,
        "Показывать соло-рейтинг zKillboard",
    );
    ui.collapsing("Дополнительно: повторная загрузка и исключения",|ui| {
        ui.checkbox(&mut draft.force_refresh,"Повторно скачать весь выбранный период");
        style::caption(ui,"Обычно достаточно сверки новых и изменившихся дней. Общая база обновляется только вручную.");
        if ui.button(format!("Сбросить исключения ({})",draft.excluded.len())).clicked(){draft.excluded.clear();}
    });
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
    section(ui, "ПЕРСОНАЖ");
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
    ui.collapsing("Подключение приложения", |ui| {
        ui.label("Client ID приложения EVE SSO");
        ui.add(egui::TextEdit::singleline(&mut draft.sso_client_id).desired_width(f32::INFINITY));
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
    });
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
    ui.add(
        egui::TextEdit::singleline(&mut state.search)
            .desired_width(f32::INFINITY)
            .hint_text("Начните вводить название…"),
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ship_search_accepts_typing_and_clicks_without_changing_selection_until_confirmed() {
        let context = egui::Context::default();
        let catalog = Catalog::load().unwrap();
        let mut draft = Settings {
            ship: "Drake Navy Issue".into(),
            ocr: true,
            ..Default::default()
        };
        let mut time = 0.;
        let mut combo_id = egui::Id::NULL;
        let mut query_id = egui::Id::NULL;
        let mut frame = |draft: &mut Settings, events: Vec<egui::Event>| {
            time += 0.1;
            let mut output = context.run_ui(
                egui::RawInput {
                    time: Some(time),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(650., 480.),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    query_id = ui.make_persistent_id("target_ship_query");
                    combo_id = target(ui, draft, &catalog).id;
                },
            );
            output.textures_delta.clear();
            (combo_id, query_id)
        };
        let (combo, query) = frame(&mut draft, vec![]);
        let button = context.read_response(combo).unwrap().rect.center();
        let click = |pos, pressed| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        frame(&mut draft, click(button, true));
        frame(&mut draft, click(button, false));
        frame(&mut draft, vec![egui::Event::Text("Gnosis".into())]);
        assert_eq!(draft.ship, "Drake Navy Issue");
        assert_eq!(
            context
                .data_mut(|data| data.get_temp::<String>(query))
                .as_deref(),
            Some("Gnosis")
        );
        let field = context.read_response(query).unwrap().rect.center();
        frame(&mut draft, click(field, true));
        frame(&mut draft, click(field, false));
        assert!(egui::ComboBox::is_open(&context, combo));
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut draft, vec![key(egui::Key::Enter)]);
        frame(&mut draft, vec![]);
        assert_eq!(draft.ship, "Gnosis");
        assert!(!draft.ocr);
        assert!(!egui::ComboBox::is_open(&context, combo));
        frame(&mut draft, click(button, true));
        frame(&mut draft, click(button, false));
        frame(&mut draft, vec![egui::Event::Paste("unknown hull".into())]);
        assert_eq!(draft.ship, "Gnosis");
        assert_eq!(
            context
                .data_mut(|data| data.get_temp::<String>(query))
                .as_deref(),
            Some("unknown hull")
        );
        frame(&mut draft, vec![key(egui::Key::Escape)]);
        frame(&mut draft, vec![]);
        assert_eq!(draft.ship, "Gnosis");
        assert!(!egui::ComboBox::is_open(&context, combo));
    }
}
