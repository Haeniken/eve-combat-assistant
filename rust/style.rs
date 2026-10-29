//! Элементы HUD: цвета, рамки, карточки вариантов и шкалы резистов.

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui,
    Vec2,
};
pub const CYAN: Color32 = Color32::from_rgb(137, 220, 239);
pub const TEXT: Color32 = Color32::from_rgb(215, 229, 237);
pub const MUTED: Color32 = Color32::from_rgb(119, 148, 166);
pub const YELLOW: Color32 = Color32::from_rgb(231, 210, 96);
pub const AMBER: Color32 = Color32::from_rgb(242, 181, 107);
pub const LINE: Color32 = Color32::from_rgb(43, 72, 89);
pub const DAMAGE_COLORS: [Color32; 4] = [
    Color32::from_rgb(100, 175, 214),
    Color32::from_rgb(214, 125, 112),
    Color32::from_rgb(181, 193, 203),
    AMBER,
];
pub fn damage_mix(ui: &mut Ui, percentages: Option<[f64; 4]>, fallback: &str) {
    if let Some(values) = percentages {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(7., 0.);
            for (i, value) in values.iter().enumerate().filter(|(_, v)| **v > 0.001) {
                ui.label(
                    RichText::new(format!("{} {:.0}%", crate::tactics::DAMAGE[i].0, value))
                        .size(13.)
                        .color(DAMAGE_COLORS[i]),
                );
            }
        });
    } else {
        ui.label(RichText::new(fallback).size(13.).color(MUTED));
    }
}
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
pub fn rating_color(rating: Option<&crate::pilot::Rating>) -> Color32 {
    let colors = [
        Color32::from_rgb(231, 91, 91),
        Color32::from_rgb(240, 148, 69),
        Color32::from_rgb(231, 210, 96),
        Color32::from_rgb(116, 205, 118),
        Color32::from_rgb(73, 213, 201),
        Color32::from_rgb(189, 131, 239),
    ];
    rating
        .and_then(|r| r.tier())
        .map(|i| colors[i])
        .unwrap_or(MUTED)
}
pub fn pilot_rating(ui: &mut Ui, rating: Option<&crate::pilot::Rating>, state: &str) {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(58., 58.), Sense::hover());
    let center = rect.center();
    let color = rating_color(rating);
    let tier = rating.and_then(|r| r.tier());
    let p = ui.painter();
    p.circle_filled(center, 24., Color32::from_rgb(9, 22, 30));
    p.circle_stroke(center, 21., Stroke::new(0.7, LINE));
    for segment in 0..6 {
        let start = (-85. + segment as f32 * 60.).to_radians();
        let points: Vec<_> = (0..=12)
            .map(|i| {
                let angle = start + (50_f32.to_radians() * i as f32 / 12.);
                center + Vec2::angled(angle) * 26.
            })
            .collect();
        let stroke = if tier.is_some_and(|tier| segment <= tier) {
            color
        } else {
            LINE
        };
        p.add(egui::Shape::line(points, Stroke::new(2.2, stroke)));
    }
    p.text(
        center + Vec2::new(0., -8.),
        Align2::CENTER_CENTER,
        "SOLO",
        FontId::proportional(6.),
        MUTED,
    );
    p.text(
        center + Vec2::new(0., 6.),
        Align2::CENTER_CENTER,
        rating
            .and_then(|r| r.score)
            .map(|s| s.to_string())
            .unwrap_or_else(|| "—".into()),
        FontId::proportional(16.),
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
        .inner_margin(8.)
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
pub fn speed_comparison(ui: &mut Ui, enemy: &serde_json::Value, own: &serde_json::Value) {
    let rows = crate::fit_metrics::speed_rows(enemy, own);
    let faster =
        |a: Option<f64>, b: Option<f64>| a.zip(b).is_some_and(|(a, b)| a.round() > b.round());
    let threat = rows.iter().any(|r| faster(r.enemy, r.own));
    egui::Frame::new().stroke(Stroke::new(1.,if threat {AMBER.gamma_multiply(0.65)}else{LINE})).inner_margin(8.).show(ui, |ui| {
        let (rect,response) = ui.allocate_exact_size(Vec2::new(ui.available_width(),rows.len() as f32*23.+22.),Sense::hover());
        let p=ui.painter();
        let origin=rect.left_top();
        let w=rect.width();
        let er=50.+(w-50.)/2.;
        p.text(origin,Align2::LEFT_TOP,"СКОРОСТЬ · М/С",FontId::proportional(10.),MUTED);
        for (x,name) in [(er,"ЕГО"),(w,"ТВОЯ")] {
            p.text(origin+Vec2::new(x,16.),Align2::RIGHT_TOP,name,FontId::proportional(8.),MUTED);
        }
        for (i,row) in rows.iter().enumerate() {
            let y=28.+i as f32*23.;
            p.text(origin+Vec2::new(0.,y+3.),Align2::LEFT_TOP,&row.label,FontId::proportional(if row.label.len()>5 {8.}else{10.}),MUTED);
            for (x,value,highlight,color,width) in [(er,row.enemy,faster(row.enemy,row.own),AMBER,(er-48.).clamp(32.,52.)),(w,row.own,faster(row.own,row.enemy),CYAN,(w-er-4.).min(62.))] {
                if highlight {
                    let cell=Rect::from_min_size(origin+Vec2::new(x-width,y-2.),Vec2::new(width+4.,20.));
                    p.rect_filled(cell,3.,color.gamma_multiply(0.12));
                    if color==AMBER {p.rect_stroke(cell,3.,Stroke::new(1.,AMBER.gamma_multiply(0.6)),StrokeKind::Inside);}
                }
                let label=value.map(|v|format!("{v:.0}")).unwrap_or_else(||"—".into());
                p.text(origin+Vec2::new(x,y),Align2::RIGHT_TOP,label,FontId::proportional(14.),if highlight{color}else{TEXT});
            }
            if i+1<rows.len() {p.line_segment([origin+Vec2::new(0.,y+21.),origin+Vec2::new(w,y+21.)],Stroke::new(1.,LINE));}
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
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::click());
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
                rect.left_top() + Vec2::new(0., 3.),
                rect.left_bottom() - Vec2::new(0., 3.),
            ],
            Stroke::new(2., CYAN),
        );
    }
    let core = title.split(" · ").next().unwrap_or(title);
    let parts: Vec<_> = core.split(" / ").collect();
    let weapon = parts.first().copied().unwrap_or(core);
    let prop = parts.get(1).copied().unwrap_or("");
    let tank = parts.get(2..).unwrap_or_default().join(" / ");
    let w = rect.width();
    let weapon_width = (w * 0.47 - 14.).max(50.);
    let font = FontId::proportional(11.);
    let mut name = weapon.to_string();
    while p.layout_no_wrap(name.clone(), font.clone(), TEXT).size().x > weapon_width
        && name.chars().count() > 2
    {
        name = name
            .trim_end_matches('…')
            .chars()
            .take(name.trim_end_matches('…').chars().count() - 1)
            .collect::<String>()
            + "…";
    }
    p.text(
        rect.left_top() + Vec2::new(10., 8.),
        Align2::LEFT_TOP,
        name,
        font,
        if active { CYAN } else { TEXT },
    );
    p.text(
        rect.left_top() + Vec2::new(w * 0.49, 9.),
        Align2::LEFT_TOP,
        prop,
        FontId::proportional(9.),
        MUTED,
    );
    let tank_rect = Rect::from_min_max(
        rect.left_top() + Vec2::new(w * 0.66, 0.),
        rect.right_bottom() - Vec2::new(53., 0.),
    );
    p.with_clip_rect(tank_rect).text(
        rect.left_top() + Vec2::new(w * 0.66, 9.),
        Align2::LEFT_TOP,
        tank,
        FontId::proportional(9.),
        MUTED,
    );
    p.text(
        rect.right_top() + Vec2::new(-7., 7.),
        Align2::RIGHT_TOP,
        if personal {
            "ПИЛОТ".into()
        } else {
            percent
                .map(|v| format!("{v:.1}%").replace('.', ","))
                .unwrap_or_else(|| "—".into())
        },
        FontId::proportional(if personal { 9. } else { 12. }),
        CYAN,
    );
    p.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1., LINE),
    );
    response.on_hover_text(format!("{title}\nДвойной щелчок — полный фит"))
}

/// Uniform scale with a fixed number of ticks, including very near/far targets.
pub fn range_axis(
    current: Option<f64>,
    desired: Option<(f64, f64)>,
    weapon: Option<f64>,
) -> (f64, f64) {
    let maximum = current
        .into_iter()
        .chain(weapon)
        .chain(desired.into_iter().flat_map(|(a, b)| [a, b]))
        .filter(|v| v.is_finite() && *v > 0.)
        .fold(1000., f64::max)
        * 1.12;
    let raw = maximum / 5.;
    let power = 10_f64.powf(raw.log10().floor());
    let step = [1., 2., 5., 10.]
        .into_iter()
        .map(|v| v * power)
        .find(|v| *v >= raw)
        .unwrap_or(power * 10.);
    (step * 5., step)
}
pub fn range_ruler(
    ui: &mut Ui,
    current: Option<f64>,
    desired: Option<(f64, f64)>,
    weapon: &serde_json::Value,
) {
    let reach = weapon["max_range_m"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.);
    if current.is_none() && desired.is_none() && reach.is_none() {
        return;
    }
    let (max, step) = range_axis(current, desired, reach);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 82.), Sense::hover());
    let p = ui.painter();
    let left = rect.left() + 10.;
    let right = rect.right() - 10.;
    let y = rect.top() + 32.;
    let project = |v: f64| left + (right - left) * (v / max).clamp(0., 1.) as f32;
    if let Some((a, b)) = desired {
        p.rect_filled(
            Rect::from_min_max(
                egui::pos2(project(a), y - 8.),
                egui::pos2(project(b), y + 8.),
            ),
            0.,
            CYAN.gamma_multiply(0.14),
        );
    }
    p.line_segment(
        [egui::pos2(left, y), egui::pos2(right, y)],
        Stroke::new(1., CYAN),
    );
    for i in 0..=5 {
        let v = step * i as f64;
        let x = project(v);
        p.line_segment(
            [egui::pos2(x, y - 4.), egui::pos2(x, y + 5.)],
            Stroke::new(1., MUTED),
        );
        let mut label = if step >= 1000. {
            format!("{:.0}", v / 1000.)
        } else {
            format!("{:.1}", v / 1000.).replace('.', ",")
        };
        if i == 5 {
            label += " км";
        }
        p.text(
            egui::pos2(x, y + 11.),
            if i == 5 {
                Align2::RIGHT_TOP
            } else {
                Align2::CENTER_TOP
            },
            label,
            FontId::proportional(8.),
            MUTED,
        );
    }
    if let Some(v) = current.filter(|v| v.is_finite() && *v >= 0.) {
        let x = project(v);
        p.line_segment(
            [egui::pos2(x, y - 16.), egui::pos2(x, y + 7.)],
            Stroke::new(1.4, TEXT),
        );
        p.text(
            egui::pos2(x.clamp(left + 35., right - 35.), rect.top()),
            Align2::CENTER_TOP,
            if v < 1000. {
                format!("ЦЕЛЬ {v:.0} м")
            } else {
                format!("ЦЕЛЬ {:.1} км", v / 1000.).replace('.', ",")
            },
            FontId::proportional(9.),
            TEXT,
        );
    }
    if let Some(v) = reach {
        let x = project(v);
        p.line_segment(
            [egui::pos2(x, y - 14.), egui::pos2(x, y + 7.)],
            Stroke::new(1.5, AMBER),
        );
        let label = format!(
            "ОРУЖИЕ {}",
            weapon["label"]
                .as_str()
                .unwrap_or("—")
                .trim_start_matches("до ")
        );
        p.text(
            egui::pos2(x.clamp(left + 75., right - 75.), y + 34.),
            Align2::CENTER_TOP,
            label,
            FontId::proportional(9.),
            AMBER,
        );
    }
    response.on_hover_text(
        weapon["hint"]
            .as_str()
            .unwrap_or("Бирюзовый диапазон — рекомендация. Белая метка — текущая дистанция."),
    );
}

pub fn settings_nav(ui: &mut Ui, index: usize, title: &str, active: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.), Sense::click());
    let p = ui.painter();
    if active || response.hovered() {
        p.rect_filled(rect, 2., Color32::from_rgb(20, 47, 60));
    }
    if active {
        p.line_segment(
            [
                rect.left_top() + Vec2::new(0., 3.),
                rect.left_bottom() - Vec2::new(0., 3.),
            ],
            Stroke::new(2., CYAN),
        );
    }
    let c = if active { CYAN } else { MUTED };
    let origin = rect.left_top() + Vec2::new(10., 9.);
    match index {
        0 => {
            for d in [
                Vec2::ZERO,
                Vec2::new(8., 0.),
                Vec2::new(0., 8.),
                Vec2::new(8., 8.),
            ] {
                p.rect_stroke(
                    Rect::from_min_size(origin + d, Vec2::splat(5.)),
                    0.,
                    Stroke::new(1., c),
                    StrokeKind::Inside,
                );
            }
        }
        1 => {
            for (a, b) in [
                (Vec2::new(7., 0.), Vec2::new(14., 14.)),
                (Vec2::new(14., 14.), Vec2::new(7., 10.)),
                (Vec2::new(7., 10.), Vec2::new(0., 14.)),
                (Vec2::new(0., 14.), Vec2::new(7., 0.)),
            ] {
                p.line_segment([origin + a, origin + b], Stroke::new(1., c));
            }
        }
        2 => {
            p.circle_stroke(origin + Vec2::splat(7.), 6., Stroke::new(1., c));
            p.circle_filled(origin + Vec2::splat(7.), 2., c);
        }
        3 => {
            p.rect_stroke(
                Rect::from_min_size(origin, Vec2::new(14., 10.)),
                0.,
                Stroke::new(1., c),
                StrokeKind::Inside,
            );
            p.line_segment(
                [origin + Vec2::new(3., 14.), origin + Vec2::new(11., 14.)],
                Stroke::new(1., c),
            );
        }
        4 => {
            for y in [0., 5., 10.] {
                p.rect_stroke(
                    Rect::from_min_size(origin + Vec2::new(0., y), Vec2::new(14., 5.)),
                    2.,
                    Stroke::new(1., c),
                    StrokeKind::Inside,
                );
            }
        }
        5 => {
            for y in [1., 7., 13.] {
                p.line_segment(
                    [origin + Vec2::new(0., y), origin + Vec2::new(14., y)],
                    Stroke::new(1., c),
                );
            }
        }
        _ => {
            p.circle_stroke(origin + Vec2::new(7., 3.), 4., Stroke::new(1., c));
            p.line_segment(
                [origin + Vec2::new(0., 14.), origin + Vec2::new(14., 14.)],
                Stroke::new(1., c),
            );
        }
    }
    p.text(
        rect.left_top() + Vec2::new(33., 17.),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(11.),
        c,
    );
    response
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
    let colors = DAMAGE_COLORS;
    let (holes, base_only) = resistance_holes(values, base);
    ui.columns(4, |cols| {
        for i in 0..4 {
            let (cell, response) = cols[i].allocate_exact_size(Vec2::new(cols[i].available_width(), 68.), Sense::hover());
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
            let rect = Rect::from_min_size(cell.left_top()+Vec2::new(5.,43.),Vec2::new((cell.width()-10.).max(0.),5.));
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
                p.text(cell.left_top()+Vec2::new(5.,53.),Align2::LEFT_TOP,format!("база {}%",base_label.replace('.',",")),FontId::proportional(9.),if holes[i] && base_only {CYAN} else {MUTED});
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
