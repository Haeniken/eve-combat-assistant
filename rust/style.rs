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
pub fn pilot_rating(ui: &mut Ui, rating: Option<&crate::pilot::Rating>, state: &str) {
    let colors = [
        Color32::from_rgb(231, 91, 91),
        Color32::from_rgb(240, 148, 69),
        Color32::from_rgb(231, 210, 96),
        Color32::from_rgb(116, 205, 118),
        Color32::from_rgb(73, 213, 201),
        Color32::from_rgb(189, 131, 239),
    ];
    let (rect, response) = ui.allocate_exact_size(Vec2::new(88., 70.), Sense::hover());
    let center = rect.left_top() + Vec2::new(44., 27.);
    let color = rating
        .and_then(|r| r.tier())
        .map(|i| colors[i])
        .unwrap_or(MUTED);
    let p = ui.painter();
    p.circle_filled(center, 25., color.gamma_multiply(0.08));
    p.circle_stroke(center, 25., Stroke::new(1.8, color));
    p.text(
        center + Vec2::new(0., -11.),
        Align2::CENTER_CENTER,
        "SOLO",
        FontId::proportional(8.),
        MUTED,
    );
    p.text(
        center + Vec2::new(0., 6.),
        Align2::CENTER_CENTER,
        rating
            .and_then(|r| r.score)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".into()),
        FontId::proportional(18.),
        color,
    );
    response.on_hover_text(
        rating
            .map(|r| {
                let mut hint = r.hint();
                if !state.is_empty() {
                    hint += &format!("\n{state}");
                }
                hint
            })
            .unwrap_or_else(|| state.to_string()),
    );
}
/// A historical participation marker, separate from the solo score's color.
/// Deliberately has no hover text: its definitions live in the documentation.
pub fn pilot_group(ui: &mut Ui, rating: &crate::pilot::Rating) {
    let Some(marker) = rating.group_marker() else {
        return;
    };
    let (label, width, color, fill, border) = if marker == "БЛОБ" {
        (
            "БЛОБ 10+",
            78.,
            AMBER,
            Color32::from_rgb(48, 41, 31),
            Color32::from_rgb(114, 84, 58),
        )
    } else {
        (
            "ГРУППА",
            70.,
            Color32::from_rgb(160, 189, 206),
            Color32::from_rgb(22, 41, 52),
            Color32::from_rgb(53, 85, 104),
        )
    };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 18.), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4., fill);
    p.rect_stroke(rect, 4., Stroke::new(1., border), StrokeKind::Inside);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(9.),
        color,
    );
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
pub fn metric_height(ui: &mut Ui, name: &str, value: &str, danger: bool, height: f32) {
    egui::Frame::new()
        .stroke(Stroke::new(1., LINE))
        .inner_margin(10.)
        .show(ui, |ui| {
            ui.set_min_height(height);
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
pub fn incoming(ui: &mut Ui, damage: &str, range: &str) {
    egui::Frame::new()
        .stroke(Stroke::new(1., LINE))
        .inner_margin(10.)
        .show(ui, |ui| {
            ui.set_min_height(86.);
            ui.set_min_width(ui.available_width());
            caption(ui, "ЕГО УРОН");
            ui.add_space(3.);
            ui.label(RichText::new(damage).size(13.).color(CYAN));
            ui.add_space(1.);
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
            ui.painter()
                .line_segment([rect.left_top(), rect.right_top()], Stroke::new(1., LINE));
            ui.label(RichText::new("ДАЛЬНОСТЬ ОРУЖИЯ").size(8.).color(MUTED));
            ui.label(RichText::new(range).size(21.).color(CYAN));
        });
}
pub fn speed_comparison(ui: &mut Ui, enemy: &serde_json::Value, own: &serde_json::Value) {
    let rows = crate::fit_metrics::speed_rows(enemy, own);
    let faster =
        |a: Option<f64>, b: Option<f64>| a.zip(b).is_some_and(|(a, b)| a.round() > b.round());
    let threat = rows.iter().any(|r| faster(r.enemy, r.own));
    egui::Frame::new().stroke(Stroke::new(1.,if threat {AMBER.gamma_multiply(0.65)}else{LINE})).inner_margin(10.).show(ui, |ui| {
        let (rect,response) = ui.allocate_exact_size(Vec2::new(ui.available_width(),rows.len() as f32*28.+28.),Sense::hover());
        let p=ui.painter();
        let origin=rect.left_top();
        let w=rect.width();
        let er=50.+(w-50.)/2.;
        p.text(origin,Align2::LEFT_TOP,"СКОРОСТЬ · М/С",FontId::proportional(10.),MUTED);
        for (x,name) in [(er,"ЕГО"),(w,"ТВОЯ")] {
            p.text(origin+Vec2::new(x,19.),Align2::RIGHT_TOP,name,FontId::proportional(8.),MUTED);
        }
        for (i,row) in rows.iter().enumerate() {
            let y=33.+i as f32*28.;
            p.text(origin+Vec2::new(0.,y+3.),Align2::LEFT_TOP,&row.label,FontId::proportional(if row.label.len()>5 {8.}else{10.}),MUTED);
            for (x,value,highlight,color,width) in [(er,row.enemy,faster(row.enemy,row.own),AMBER,(er-48.).clamp(32.,52.)),(w,row.own,faster(row.own,row.enemy),CYAN,(w-er-4.).min(62.))] {
                if highlight {
                    let cell=Rect::from_min_size(origin+Vec2::new(x-width,y-2.),Vec2::new(width+4.,23.));
                    p.rect_filled(cell,3.,color.gamma_multiply(0.12));
                    if color==AMBER {p.rect_stroke(cell,3.,Stroke::new(1.,AMBER.gamma_multiply(0.6)),StrokeKind::Inside);}
                }
                let label=value.map(|v|format!("{v:.0}")).unwrap_or_else(||"—".into());
                p.text(origin+Vec2::new(x,y),Align2::RIGHT_TOP,label,FontId::proportional(14.),if highlight{color}else{TEXT});
            }
            if i+1<rows.len() {p.line_segment([origin+Vec2::new(0.,y+24.),origin+Vec2::new(w,y+24.)],Stroke::new(1.,LINE));}
        }
        response.on_hover_text("База — двигатели выключены, пассивные бонусы и штрафы фита сохранены. AB/MWD — каждый двигатель отдельно, без внешних сеток и скрамблеров. Его навыки V, твои — загруженные навыки персонажа с выбранными имплантами и бустерами. Если режимы отличаются, подпись слева: его / твой. Янтарный — он быстрее, бирюзовый — ты быстрее.\nЕсли твои навыки не загружены, сравнение недоступно: в колонке «ТВОЯ» стоит прочерк.");
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
/// A flat fitted layer has no effective hole. Only then show a distinct base hint.
fn resistance_holes(values: [f64; 4], base: Option<[f64; 4]>) -> ([bool; 4], bool) {
    let minima = |values: [f64; 4]| {
        let low = values.into_iter().fold(f64::INFINITY, f64::min);
        let high = values.into_iter().fold(f64::NEG_INFINITY, f64::max);
        values.map(|v| high - low >= 0.01 && (v - low).abs() < 0.01)
    };
    let fitted = minima(values);
    if fitted.iter().any(|v| *v) {
        (fitted, false)
    } else {
        (base.map(minima).unwrap_or([false; 4]), true)
    }
}

pub fn abyssal_alert(ui: &mut Ui, details: &str) {
    egui::Frame::new()
        .fill(AMBER.gamma_multiply(0.08))
        .stroke(Stroke::new(1., AMBER.gamma_multiply(0.65)))
        .inner_margin(7.)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("!  Присутствуют Abyssal модули, оценка не точна").size(11.).color(AMBER));
        })
        .response
        .on_hover_text(format!("Исходный модуль и параметры мутации не указаны в публичной потере. Для расчёта подставлен обычный аналог T2 того же размера; для семейств без T2 — существующий обычный аналог.\n{details}"));
}

pub fn resistance(ui: &mut Ui, values: [f64; 4], base: Option<[f64; 4]>) {
    let colors = [
        Color32::from_rgb(100, 175, 214),
        Color32::from_rgb(214, 125, 112),
        Color32::from_rgb(181, 193, 203),
        AMBER,
    ];
    let (holes, base_only) = resistance_holes(values, base);
    ui.columns(4, |cols| {
        for i in 0..4 {
            let (cell, response) = cols[i].allocate_exact_size(Vec2::new(cols[i].available_width(), 74.), Sense::hover());
            let p = cols[i].painter();
            let highlight = if base_only { CYAN } else { AMBER };
            if holes[i] {
                p.rect_filled(cell, 2., highlight.gamma_multiply(0.08));
                p.rect_stroke(cell, 2., Stroke::new(1.5, highlight), StrokeKind::Inside);
                p.text(cell.right_top()+Vec2::new(-5.,7.), Align2::RIGHT_TOP,
                    if base_only {"БАЗА"} else {"ДЫРА"}, FontId::proportional(9.), highlight);
            }
            p.text(cell.left_top()+Vec2::new(5.,7.), Align2::LEFT_TOP,
                ["EM","THERM","KIN","EXP"][i], FontId::proportional(10.), MUTED);
            p.text(cell.left_top()+Vec2::new(5.,24.), Align2::LEFT_TOP,
                format!("{:.0}%",values[i]), FontId::proportional(14.), colors[i]);
            let rect = Rect::from_min_size(cell.left_top()+Vec2::new(5.,46.),Vec2::new((cell.width()-10.).max(0.),5.));
            let segment=|from:f64,to:f64|Rect::from_min_max(egui::pos2(rect.left()+rect.width()*(from/100.).clamp(0.,1.) as f32,rect.top()),egui::pos2(rect.left()+rect.width()*(to/100.).clamp(0.,1.) as f32,rect.bottom()));
            p.rect_filled(rect,0.,LINE);
            if let Some(base)=base {
                let dark=colors[i].gamma_multiply(0.42);
                p.rect_filled(segment(0.,values[i].min(base[i])),0.,dark);
                if values[i]>=base[i] {p.rect_filled(segment(base[i],values[i]),0.,colors[i]);}
                else {
                    let removed=segment(values[i],base[i]);
                    p.rect_stroke(removed,0.,Stroke::new(1.,AMBER),StrokeKind::Inside);
                }
                let base_label=if (base[i]-base[i].round()).abs()<0.005 {format!("{:.0}",base[i])}else{format!("{:.1}",base[i])};
                p.text(cell.left_top()+Vec2::new(5.,57.),Align2::LEFT_TOP,format!("база {}%",base_label.replace('.',",")),FontId::proportional(9.),if holes[i] && base_only {CYAN} else {MUTED});
            }else{p.rect_filled(segment(0.,values[i]),0.,colors[i]);}
            let mut hint=if let Some(base)=base {format!("Итог {:.2}%\nБаза без модулей {:.2}%\nИзменение от модулей {:+.2} п.п.\nНавыки V, без перегрева. Это пример опубликованного фита, а не измерение текущего корабля.",values[i],base[i],values[i]-base[i])}else{format!("Итог {:.2}% · база недоступна",values[i])};
            if holes[i] {hint+=if base_only {"\nБазовая уязвимость. Итоговые резисты равны: этот тип не получает преимущества по фактической защите."} else {"\nДыра: минимальный итоговый резист выбранного слоя."};}
            response.on_hover_text(hint);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn holes_distinguish_fitted_minima_from_base_ties_and_uniform_layers() {
        assert_eq!(
            resistance_holes([20., 40., 20., 60.], Some([0., 20., 40., 50.])),
            ([true, false, true, false], false)
        );
        assert_eq!(
            resistance_holes([65.; 4], Some([0., 20., 40., 50.])),
            ([true, false, false, false], true)
        );
        assert_eq!(
            resistance_holes([65.; 4], Some([27.5; 4])),
            ([false; 4], true)
        );
        assert_eq!(
            resistance_holes([0.; 4], Some([40.; 4])),
            ([false; 4], true)
        );
        assert_eq!(resistance_holes([65.; 4], None), ([false; 4], true));
    }
}
