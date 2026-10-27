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
    SelectRegion,
    Sync,
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
}

impl Window<'_> {
    pub fn show(&mut self, ui: &mut Ui, draft: &mut Settings) -> Response {
        let mut response = Response::default();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(9, 19, 29))
                    .inner_margin(24.),
            )
            .show(ui, |ui| {
                ui.label(RichText::new("НАСТРОЙКИ").size(22.).color(CYAN));
                ui.add_space(8.);
                ui.horizontal(|ui| {
                    for (index, title) in ["Вид", "Мой корабль", "Цель", "Экран", "Данные"]
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
                        3 => response.action = screen(ui, draft),
                        _ => response.action = data(ui, draft, self.syncing, self.cancel_sync),
                    });
                ui.add_space(10.);
                style::separator(ui);
                ui.label(RichText::new(self.status).size(11.).color(MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Сохранить").clicked() {
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
            .code_editor()
            .desired_width(f32::INFINITY)
            .desired_rows(13)
            .hint_text("[Tristan, Solo]\n…"),
    );
    ui.horizontal(|ui| {
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

fn screen(ui: &mut Ui, draft: &mut Settings) -> Option<Action> {
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
    let select = ui.button("Выделить область экрана").clicked();
    if let Some(region) = &mut draft.region {
        ui.label("Координаты в пикселях");
        ui.horizontal(|ui| {
            for (index, title) in ["X", "Y", "Ширина", "Высота"].iter().enumerate() {
                ui.label(*title);
                ui.add(egui::DragValue::new(&mut region[index]).speed(1.));
            }
        });
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
        "Выдели панель Selected Item: она обновляется при смене цели. Должны быть видны ник и тип корпуса.",
    );
    select.then_some(Action::SelectRegion)
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
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!syncing, egui::Button::new("Обновить общую статистику"))
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
