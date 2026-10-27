//! Элементы HUD: цвета, рамки, карточки вариантов и шкалы резистов.

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};
pub const CYAN: Color32 = Color32::from_rgb(137, 220, 239);
pub const TEXT: Color32 = Color32::from_rgb(215, 229, 237);
pub const MUTED: Color32 = Color32::from_rgb(119, 148, 166);
pub const AMBER: Color32 = Color32::from_rgb(242, 181, 107);
pub const LINE: Color32 = Color32::from_rgb(43, 72, 89);
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(10.).color(MUTED));
}
pub fn heading(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.).color(CYAN).strong());
}
pub fn separator(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
    ui.painter()
        .line_segment([rect.left_top(), rect.right_top()], Stroke::new(1., LINE));
    ui.add_space(7.);
}
pub fn gear(ui: &mut Ui) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.), Sense::click());
    let p = ui.painter();
    let center = rect.center();
    let color = if response.hovered() { TEXT } else { CYAN };
    p.circle_stroke(center, 6., Stroke::new(1.4, color));
    p.circle_stroke(center, 2., Stroke::new(1.2, color));
    for i in 0..8 {
        let d = Vec2::angled(i as f32 * std::f32::consts::TAU / 8.);
        p.line_segment([center + d * 7., center + d * 10.], Stroke::new(2., color));
    }
    response.on_hover_text("Настройки")
}
pub fn border(ui: &mut Ui, rect: Rect) {
    ui.painter()
        .rect_stroke(rect, 0., Stroke::new(1., LINE), StrokeKind::Inside);
    let p = ui.painter();
    for (x, y, dx, dy) in [
        (rect.left(), rect.top(), 1., 1.),
        (rect.right(), rect.top(), -1., 1.),
        (rect.left(), rect.bottom(), 1., -1.),
        (rect.right(), rect.bottom(), -1., -1.),
    ] {
        p.line_segment(
            [Pos2::new(x, y), Pos2::new(x + dx * 16., y)],
            Stroke::new(1., CYAN),
        );
        p.line_segment(
            [Pos2::new(x, y), Pos2::new(x, y + dy * 16.)],
            Stroke::new(1., CYAN),
        );
    }
}
pub fn metric(ui: &mut Ui, name: &str, value: &str, danger: bool) {
    egui::Frame::new()
        .stroke(Stroke::new(1., LINE))
        .inner_margin(10.)
        .show(ui, |ui| {
            ui.set_min_height(52.);
            ui.set_min_width(ui.available_width());
            caption(ui, name);
            ui.add_space(5.);
            ui.label(
                RichText::new(value)
                    .size(13.)
                    .color(if danger { AMBER } else { CYAN }),
            );
        });
}
pub fn variant(
    ui: &mut Ui,
    title: &str,
    percent: Option<f64>,
    personal: bool,
    active: bool,
) -> Response {
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), if personal { 64. } else { 55. }),
        Sense::click(),
    );
    let p = ui.painter();
    if active || response.hovered() {
        p.rect_filled(
            rect,
            0.,
            Color32::from_rgba_unmultiplied(78, 160, 190, if active { 28 } else { 13 }),
        );
    }
    if active {
        p.line_segment(
            [
                rect.left_top() + Vec2::new(0., 5.),
                rect.left_bottom() - Vec2::new(0., 5.),
            ],
            Stroke::new(2., CYAN),
        );
    }
    let parts: Vec<_> = title.split(" · ").collect();
    let first = parts.first().copied().unwrap_or(title);
    let cap = ((rect.width() - 90.) / 7.).max(8.) as usize;
    let name = if first.chars().count() > cap {
        format!("{}…", first.chars().take(cap - 1).collect::<String>())
    } else {
        first.into()
    };
    let top = rect.top() + if personal { 26. } else { 9. };
    if personal {
        p.text(
            rect.left_top() + Vec2::new(11., 7.),
            Align2::LEFT_TOP,
            "ФИТ ПИЛОТА",
            FontId::proportional(10.),
            CYAN,
        );
    }
    p.text(
        Pos2::new(rect.left() + 11., top),
        Align2::LEFT_TOP,
        name,
        FontId::proportional(12.),
        if active { CYAN } else { TEXT },
    );
    if let Some(percent) = percent {
        p.text(
            rect.right_top() + Vec2::new(-8., 9.),
            Align2::RIGHT_TOP,
            format!("{percent:.1}%").replace('.', ","),
            FontId::monospace(13.),
            CYAN,
        );
        let bar = Rect::from_min_size(
            rect.left_bottom() + Vec2::new(11., -7.),
            Vec2::new(rect.width() - 22., 2.),
        );
        p.rect_filled(bar, 0., LINE);
        p.rect_filled(
            Rect::from_min_size(bar.min, Vec2::new(bar.width() * percent as f32 / 100., 2.)),
            0.,
            CYAN,
        );
    }
    if parts.len() > 1 {
        p.text(
            Pos2::new(rect.left() + 11., top + 19.),
            Align2::LEFT_TOP,
            parts[1],
            FontId::proportional(10.),
            MUTED,
        );
    }
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1., LINE),
    );
    response.on_hover_text(format!("{title}\nДвойной щелчок — полный фит"))
}
pub fn resistance(ui: &mut Ui, values: [f64; 4]) {
    let colors = [
        Color32::from_rgb(100, 175, 214),
        Color32::from_rgb(214, 125, 112),
        Color32::from_rgb(181, 193, 203),
        AMBER,
    ];
    ui.columns(4, |cols| {
        for i in 0..4 {
            caption(&mut cols[i], ["EM", "THERM", "KIN", "EXP"][i]);
            cols[i].label(
                RichText::new(format!("{:.0}%", values[i]))
                    .size(12.)
                    .color(colors[i]),
            );
            let (rect, _) = cols[i]
                .allocate_exact_size(Vec2::new(cols[i].available_width(), 3.), Sense::hover());
            cols[i].painter().rect_filled(rect, 0., LINE);
            cols[i].painter().rect_filled(
                Rect::from_min_size(
                    rect.min,
                    Vec2::new(rect.width() * (values[i] / 100.).clamp(0., 1.) as f32, 3.),
                ),
                0.,
                colors[i],
            );
        }
    });
}
