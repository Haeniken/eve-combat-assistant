//! Independent pilot lookup on the main HUD. Enter or explicit paste, without a hull.
use crate::{pilot::Rating, style};
use eframe::egui::{self, RichText};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Debug)]
pub struct Profile {
    pub name: String,
    pub rating: Rating,
}
#[derive(Clone, Debug)]
pub struct Loss {
    pub killmail_id: i64,
    pub happened: String,
    pub ship: String,
    pub description: Option<String>,
    pub eft: Option<String>,
    pub fit: Option<serde_json::Value>,
}
pub struct Request {
    pub generation: u64,
    pub name: String,
    pub debug: bool,
    pub cancel: Arc<AtomicBool>,
}
pub enum Event {
    Profile(Result<Profile, String>),
    Loss(Result<Option<Loss>, String>),
    Activity(Rating),
}
#[derive(Default)]
pub struct State {
    pub input: String,
    pub generation: u64,
    pub profile: Option<Profile>,
    pub loss: Option<Loss>,
    pub error: Option<String>,
    pub loss_error: Option<String>,
    pub profile_pending: bool,
    pub loss_pending: bool,
    pub expanded: bool,
    focus_requested: bool,
    cancel: Option<Arc<AtomicBool>>,
}
impl State {
    pub fn clear(&mut self) {
        self.input.clear();
        self.invalidate();
        self.expanded = false;
        self.focus_requested = false;
    }
    fn invalidate(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.generation = self.generation.wrapping_add(1);
        self.profile = None;
        self.loss = None;
        self.error = None;
        self.loss_error = None;
        self.profile_pending = false;
        self.loss_pending = false;
    }
    pub fn toggle(&mut self) {
        self.expanded = !self.expanded;
        self.focus_requested = self.expanded;
    }
    pub fn start(&mut self, debug: bool) -> Result<Request, String> {
        self.invalidate();
        self.expanded = true;
        let name = self.input.trim();
        if name.is_empty()
            || name.chars().count() > 100
            || !name.chars().any(char::is_alphanumeric)
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || " '-_.’".contains(c))
        {
            return Err("Введите ник пилота целиком".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.profile_pending = true;
        self.loss_pending = true;
        Ok(Request {
            generation: self.generation,
            name: name.into(),
            debug,
            cancel,
        })
    }
    pub fn apply(&mut self, generation: u64, event: Event) {
        if generation != self.generation {
            return;
        }
        match event {
            Event::Profile(result) => {
                self.profile_pending = false;
                match result {
                    Ok(profile) => {
                        self.input = profile.name.clone();
                        self.profile = Some(profile);
                    }
                    Err(error) => {
                        self.error = Some(error);
                        self.loss_pending = false;
                    }
                }
            }
            Event::Loss(result) => {
                self.loss_pending = false;
                match result {
                    Ok(loss) => self.loss = loss,
                    Err(error) => self.loss_error = Some(error),
                }
            }
            Event::Activity(rating) => {
                if let Some(profile) = &mut self.profile
                    && profile.rating.character_id == rating.character_id
                {
                    profile.rating = rating;
                }
            }
        }
    }
}
pub enum Action {
    Submit,
    OpenFit(String),
    SelectLoss(Box<Profile>, Box<Loss>),
}

pub fn local_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%d.%m.%Y %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| "—".into())
}

/// Consume paste only in the main window, including while its search field is hidden.
fn take_paste(
    ctx: &egui::Context,
    allowed: bool,
    read: impl FnOnce() -> Result<String, String>,
) -> Option<Result<String, String>> {
    if !allowed || !ctx.input(|input| input.focused) {
        return None;
    }
    let (paste, shortcut) = ctx.input_mut(|input| {
        if let Some(index) = input
            .events
            .iter()
            .position(|event| matches!(event, egui::Event::Paste(_)))
        {
            let egui::Event::Paste(text) = input.events.remove(index) else {
                unreachable!()
            };
            return (Some(text), false);
        }
        (
            None,
            input.consume_key(egui::Modifiers::CTRL, egui::Key::V)
                || input.consume_key(egui::Modifiers::COMMAND, egui::Key::V),
        )
    });
    paste.map(Ok).or_else(|| shortcut.then(read))
}
pub fn popup(
    ui: &mut egui::Ui,
    state: &mut State,
    anchor: egui::Rect,
    allowed: bool,
) -> Option<Action> {
    if !allowed {
        return None;
    }
    let id = egui::Id::unique("pilot_lookup_input");
    let paste = take_paste(ui.ctx(), true, || {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| clipboard.get_text())
            .map_err(|error| error.to_string())
    });
    let mut submit = false;
    if let Some(result) = paste {
        state.invalidate();
        state.expanded = true;
        state.focus_requested = true;
        match result {
            Ok(text) => {
                state.input = text.trim().into();
                submit = true;
            }
            Err(error) => state.error = Some(format!("Не удалось вставить ник: {error}")),
        }
    }
    let mut action = None;
    if state.expanded {
        let width = ui.available_width().clamp(200., 360.);
        let position = egui::pos2(
            (anchor.right() - width).max(ui.max_rect().left()),
            anchor.bottom() + 10.,
        );
        let popup = egui::Area::new(id.with("result"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(ui.ctx(), |ui| {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(12, 32, 43))
                    .stroke(egui::Stroke::new(1., style::LINE))
                    .corner_radius(3.)
                    .inner_margin(9)
                    .show(ui, |ui| {
                        ui.set_width(width - 18.);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.;
                            let edit = ui.add_sized(
                                [(ui.available_width() - 50.).max(80.), 22.],
                                egui::TextEdit::singleline(&mut state.input)
                                    .id(id)
                                    .frame(egui::Frame::NONE)
                                    .font(egui::TextStyle::Body)
                                    .hint_text("Проверить пилота"),
                            );
                            if edit.changed() && !submit {
                                state.invalidate();
                            }
                            if edit.lost_focus()
                                && ui.input(|input| input.key_pressed(egui::Key::Enter))
                            {
                                submit = true;
                            }
                            if state.focus_requested || submit {
                                edit.request_focus();
                                state.focus_requested = false;
                            }
                            if style::search(ui, false)
                                .on_hover_text("Найти пилота")
                                .clicked()
                            {
                                submit = true;
                            }
                            if !state.input.is_empty() && ui.small_button("×").clicked() {
                                state.input.clear();
                                state.invalidate();
                                state.focus_requested = true;
                            }
                        });
                        if state.profile_pending {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                style::caption(ui, "Поиск пилота…");
                            });
                        }
                        if let Some(error) = &state.error {
                            ui.colored_label(style::AMBER, error);
                        }
                        if let Some(profile) = &state.profile {
                            ui.add_space(6.);
                            style::separator(ui);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&profile.name).color(style::TEXT));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let rating_response = ui
                                            .label(
                                                RichText::new(
                                                    profile
                                                        .rating
                                                        .score
                                                        .map(|s| s.to_string())
                                                        .unwrap_or_else(|| "—".into()),
                                                )
                                                .size(19.)
                                                .color(style::rating_color(Some(&profile.rating))),
                                            )
                                            .on_hover_text(profile.rating.hint());
                                        style::pilot_link(
                                            rating_response,
                                            profile.rating.character_id,
                                        );
                                    },
                                );
                            });
                            if !profile.rating.stats_available {
                                style::caption(ui, "Статистика zKillboard пока недоступна");
                            }
                            style::pilot_group(ui, &profile.rating);
                            ui.add_space(3.);
                            style::caption(ui, "ПОСЛЕДНЯЯ PVP-ПОТЕРЯ");
                            if state.loss_pending {
                                style::caption(ui, "Загрузка сведений о потере…");
                            } else if let Some(error) = &state.loss_error {
                                ui.colored_label(style::AMBER, error);
                            } else if let Some(loss) = &state.loss {
                                let card = egui::Frame::NONE
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(&loss.ship).size(18.).color(style::TEXT),
                                        );
                                        if let Some(description) = &loss.description {
                                            ui.label(
                                                RichText::new(description)
                                                    .size(11.)
                                                    .color(style::MUTED),
                                            );
                                        }
                                        style::caption(ui, &local_time(&loss.happened));
                                    })
                                    .response;
                                let card = ui
                                    .interact(card.rect, id.with("loss"), egui::Sense::click())
                                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                                if card.hovered() {
                                    ui.painter().hline(
                                        card.rect.x_range(),
                                        card.rect.bottom(),
                                        egui::Stroke::new(1., style::CYAN),
                                    );
                                }
                                if card.clicked() {
                                    action = Some(Action::SelectLoss(
                                        Box::new(profile.clone()),
                                        Box::new(loss.clone()),
                                    ));
                                }
                                ui.horizontal(|ui| {
                                    if ui.link("В помощник").clicked() {
                                        action = Some(Action::SelectLoss(
                                            Box::new(profile.clone()),
                                            Box::new(loss.clone()),
                                        ));
                                    }
                                    if let Some(eft) = &loss.eft {
                                        if ui.link("EFT").clicked() {
                                            action = Some(Action::OpenFit(eft.clone()));
                                        }
                                    } else {
                                        style::caption(ui, "Модули в записи не указаны");
                                    }
                                    ui.hyperlink_to(
                                        "zKillboard",
                                        format!(
                                            "https://zkillboard.com/kill/{}/",
                                            loss.killmail_id
                                        ),
                                    );
                                });
                            } else {
                                style::caption(
                                    ui,
                                    "PvP-потерь на кораблях в доступной выборке нет",
                                );
                            }
                        }
                    });
            });
        if ui.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|pos| !anchor.contains(pos) && !popup.response.rect.contains(pos))
        }) {
            state.expanded = false;
        }
    }
    if submit {
        action = Some(Action::Submit);
    }
    if matches!(action, Some(Action::SelectLoss(..))) {
        state.expanded = false;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    #[test]
    fn loss_time_is_local_formatted_and_equivalent_offsets_match() {
        let formatted = local_time("2026-10-18T17:52:57Z");
        assert_eq!(formatted, local_time("2026-10-18T20:52:57+03:00"));
        assert_eq!(formatted.len(), 19);
        for (index, character) in [(2, '.'), (5, '.'), (10, ' '), (13, ':'), (16, ':')] {
            assert_eq!(formatted.chars().nth(index), Some(character));
        }
        assert!(!formatted.contains('T') && !formatted.ends_with('Z'));
        assert_eq!(local_time("invalid"), "—");
    }
    #[test]
    fn lookup_enter_and_paste_replace_name_cancel_previous_and_ignore_stale_result() {
        let context = egui::Context::default();
        let mut state = State::default();
        state.toggle();
        let mut field = egui::Id::NULL;
        let mut time = 0.;
        let mut frame = |state: &mut State, events: Vec<egui::Event>| {
            time += 0.1;
            let mut action = None;
            let mut output = context.run_ui(
                egui::RawInput {
                    time: Some(time),
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(440., 800.),
                    )),
                    ..Default::default()
                },
                |ui| {
                    field = egui::Id::unique("pilot_lookup_input");
                    action = popup(
                        ui,
                        state,
                        egui::Rect::from_min_size(egui::pos2(350., 0.), egui::vec2(24., 24.)),
                        true,
                    );
                },
            );
            output.textures_delta.clear();
            (field, action)
        };
        let (field, _) = frame(&mut state, vec![]);
        context.memory_mut(|m| m.request_focus(field));
        let (_, action) = frame(
            &mut state,
            vec![
                egui::Event::Text("Exo9".into()),
                key(egui::Key::Enter, egui::Modifiers::NONE),
            ],
        );
        assert!(matches!(action, Some(Action::Submit)));
        let first = state.start(false).unwrap();
        assert_eq!(first.name, "Exo9");
        let (_, action) = frame(
            &mut state,
            vec![egui::Event::Paste("  Charlot theHarlot  ".into())],
        );
        assert!(matches!(action, Some(Action::Submit)));
        let next = state.start(false).unwrap();
        assert_eq!(next.name, "Charlot theHarlot");
        assert!(first.cancel.load(Ordering::Relaxed));
        state.apply(first.generation, Event::Profile(Err("old reply".into())));
        assert!(state.error.is_none() && state.profile_pending);
        state.apply(next.generation, Event::Profile(Err("not found".into())));
        assert_eq!(state.error.as_deref(), Some("not found"));
        assert!(!state.profile_pending && !state.loss_pending);
    }
    #[test]
    fn lookup_ctrl_v_reads_clipboard_only_in_the_allowed_focused_window() {
        let ctx = egui::Context::default();
        let id = egui::Id::unique("lookup-paste-test");
        let reads = std::cell::Cell::new(0);
        for focused in [false, true] {
            let mut result = None;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events: vec![key(egui::Key::V, egui::Modifiers::CTRL)],
                    ..Default::default()
                },
                |ui| {
                    if focused {
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                    result = take_paste(ui.ctx(), focused, || {
                        reads.set(reads.get() + 1);
                        Ok("4erv94ok".into())
                    });
                },
            );
            output.textures_delta.clear();
            if focused {
                assert_eq!(result.unwrap().unwrap(), "4erv94ok");
            } else {
                assert!(result.is_none());
            }
        }
        assert_eq!(reads.get(), 1);
    }
    #[test]
    fn lookup_keeps_exact_names_and_rejects_multiline_or_empty_input() {
        for name in ["4erv94ok", "12345", "Ke'on Ishaq", "Charlot theHarlot"] {
            let mut state = State {
                input: name.into(),
                ..Default::default()
            };
            assert_eq!(state.start(false).unwrap().name, name);
        }
        for name in ["", "  ", "One\nTwo", "https://example.org", "---"] {
            let mut state = State {
                input: name.into(),
                ..Default::default()
            };
            assert!(state.start(false).is_err());
        }
    }
}
