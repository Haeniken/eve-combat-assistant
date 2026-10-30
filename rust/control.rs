//! Fitted control ranges, accepted-sample alerts and independent visual motion.
use crate::model::{Settings, array, id, n, s};
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, Vec2};
use serde::Serialize;
use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Ranges {
    pub scram: f64,
    pub web: f64,
    pub neut: f64,
    pub own_tackle: f64,
    pub own_scram: bool,
}
fn active(module: &Value) -> bool {
    ["active", "overload"].contains(&s(module, "state"))
}
fn fitted(result: &Value) -> (f64, f64, f64, f64) {
    let (mut scram, mut web, mut neut, mut point) = (0_f64, 0_f64, 0_f64, 0_f64);
    for module in array(&result["modules"]).iter().filter(|m| active(m)) {
        let a = &module["attributes"];
        let optimal = n(a, "maxRange");
        if !optimal.is_finite() || optimal <= 0. {
            continue;
        }
        let name = s(module, "name").to_lowercase();
        if id(module, "group") == 52 {
            if name.contains("scrambler") {
                scram = scram.max(optimal);
            } else if name.contains("disruptor") {
                point = point.max(optimal);
            }
        }
        if id(module, "group") == 65 {
            web = web.max(optimal);
        }
        if id(module, "group") == 71 {
            // Same useful-range convention as the existing neutralizer metric.
            neut = neut.max(optimal + 0.25 * n(a, "falloff").max(0.));
        }
    }
    (scram, web, neut, point)
}
impl Ranges {
    pub fn from_results(enemy: &Value, own: &Value) -> Self {
        let (scram, web, neut, _) = fitted(enemy);
        let (own_scram, _, _, own_point) = fitted(own);
        Self {
            scram,
            web,
            neut,
            own_tackle: own_scram.max(own_point),
            own_scram: own_scram > own_point,
        }
    }
    pub fn maximum(&self) -> f64 {
        self.scram.max(self.web).max(self.neut).max(self.own_tackle)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cue {
    Tackle,
    Neut,
    Point,
}
#[derive(Default)]
pub struct Alerts {
    key: String,
    states: [Option<bool>; 4],
    last_sound: Option<Instant>,
}
impl Alerts {
    pub fn suspend(&mut self) {
        self.states = [None; 4];
    }
    pub fn observe(
        &mut self,
        key: &str,
        distance: f64,
        ranges: Ranges,
        settings: &Settings,
        now: Instant,
    ) -> Option<Cue> {
        if !distance.is_finite() || distance < 0. {
            self.suspend();
            return None;
        }
        let key = format!(
            "{key}:{ranges:?}:{}:{}:{}",
            settings.sound_tackle, settings.sound_neut, settings.sound_point
        );
        if self.key != key {
            self.key = key;
            self.suspend();
        }
        let mut candidates = Vec::new();
        for (i, (reach, enabled, cue, outward)) in [
            (ranges.scram, settings.sound_tackle, Cue::Tackle, false),
            (ranges.web, settings.sound_tackle, Cue::Tackle, false),
            (ranges.neut, settings.sound_neut, Cue::Neut, false),
            (ranges.own_tackle, settings.sound_point, Cue::Point, true),
        ]
        .into_iter()
        .enumerate()
        {
            if !enabled || reach <= 0. {
                self.states[i] = None;
                continue;
            }
            let Some(inside) = self.states[i] else {
                self.states[i] = Some(distance <= reach);
                continue;
            };
            let margin = (reach * 0.02).clamp(100., 500.);
            let next = if outward {
                if inside {
                    distance <= reach
                } else {
                    distance < reach - margin
                }
            } else if inside {
                distance <= reach + margin
            } else {
                distance <= reach
            };
            self.states[i] = Some(next);
            if next != inside && (outward && !next || !outward && next) {
                candidates.push(cue);
            }
        }
        let cue = [Cue::Tackle, Cue::Point, Cue::Neut]
            .into_iter()
            .find(|cue| candidates.contains(cue));
        if cue.is_some()
            && self.last_sound.is_none_or(|last| {
                now.saturating_duration_since(last) >= Duration::from_millis(1400)
            })
        {
            self.last_sound = Some(now);
            cue
        } else {
            None
        }
    }
}
#[derive(Default)]
pub struct Motion {
    key: String,
    from: f64,
    target: f64,
    at: f64,
    duration: f64,
    initialized: bool,
}
impl Motion {
    pub fn settled(&self, now: f64) -> bool {
        !self.initialized || now - self.at >= self.duration
    }
    fn position(&self, now: f64) -> f64 {
        let t = if self.duration > 0. {
            ((now - self.at) / self.duration).clamp(0., 1.)
        } else {
            1.
        };
        self.from + (self.target - self.from) * (1. - (1. - t).powi(3))
    }
    fn sample(&mut self, key: &str, value: f64, now: f64, interval: f64) -> (f64, bool) {
        if !self.initialized || self.key != key {
            self.key = key.into();
            self.from = value;
            self.target = value;
            self.at = now;
            self.duration = 0.;
            self.initialized = true;
        } else if self.target != value {
            self.from = self.position(now);
            self.target = value;
            self.at = now;
            self.duration = (interval * 0.8).clamp(0.02, 0.14);
        }
        (self.position(now), now - self.at < self.duration)
    }
}
pub struct View<'a> {
    pub current: Option<f64>,
    pub desired: Option<(f64, f64)>,
    pub weapon: &'a Value,
    pub ranges: Ranges,
    pub key: &'a str,
    pub interval: f64,
}
fn range_label(value: f64) -> String {
    if value < 1000. {
        format!("{value:.0} м")
    } else {
        format!("{:.1} км", value / 1000.).replace('.', ",")
    }
}
pub fn draw(ui: &mut egui::Ui, view: View<'_>, motion: &mut Motion) {
    use crate::style::{AMBER, CYAN, LINE, MUTED, TEXT, YELLOW};
    const PURPLE: Color32 = Color32::from_rgb(189, 131, 239);
    let reach = view.weapon["max_range_m"]
        .as_f64()
        .filter(|v| v.is_finite() && *v > 0.);
    if view.current.is_none()
        && view.desired.is_none()
        && reach.is_none()
        && view.ranges.maximum() <= 0.
    {
        return;
    }
    let (max, step) = crate::style::range_axis(
        view.current,
        view.desired,
        Some(reach.unwrap_or(0.).max(view.ranges.maximum())),
    );
    let rows = [
        ("скрам", view.ranges.scram, Color32::from_rgb(237, 139, 124)),
        ("сетка", view.ranges.web, AMBER),
        ("нейт", view.ranges.neut, YELLOW),
    ];
    let visible_rows = rows.iter().filter(|(_, v, _)| *v > 0.).count();
    let axis_offset = 22. + visible_rows as f32 * 12.;
    let height = axis_offset
        + if view.ranges.own_tackle > 0. {
            47.
        } else {
            35.
        };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
    let p = ui.painter();
    let left = rect.left() + 91.;
    let right = rect.right() - 10.;
    let axis = rect.top() + axis_offset;
    let project = |v: f64| left + (right - left) * (v / max).clamp(0., 1.) as f32;
    // Open HUD corner brackets with a recessed highlight, no enclosing card.
    for (a, b) in [
        (rect.left(), rect.left() + 12.),
        (rect.right() - 12., rect.right()),
    ] {
        p.line_segment(
            [
                egui::pos2(a, rect.top() + 10.),
                egui::pos2(b, rect.top() + 10.),
            ],
            Stroke::new(0.7, LINE),
        );
        p.line_segment(
            [
                egui::pos2(a, rect.bottom() - 2.),
                egui::pos2(b, rect.bottom() - 2.),
            ],
            Stroke::new(0.7, LINE),
        );
    }
    for (row, (label, value, color)) in rows.into_iter().filter(|(_, v, _)| *v > 0.).enumerate() {
        let y = rect.top() + 17. + row as f32 * 12.;
        p.text(
            egui::pos2(left - 8., y),
            Align2::RIGHT_CENTER,
            format!("{label} · {}", range_label(value)),
            FontId::proportional(8.),
            color,
        );
        p.line_segment(
            [egui::pos2(left, y + 1.), egui::pos2(project(value), y + 1.)],
            Stroke::new(2.2, Color32::BLACK.gamma_multiply(0.45)),
        );
        p.line_segment(
            [egui::pos2(left, y), egui::pos2(project(value), y)],
            Stroke::new(1.2, color.gamma_multiply(0.85)),
        );
        p.circle_stroke(egui::pos2(project(value), y), 1.6, Stroke::new(0.65, color));
    }
    if let Some((a, b)) = view.desired {
        let band = Rect::from_min_max(
            egui::pos2(project(a), axis - 7.),
            egui::pos2(project(b), axis + 7.),
        );
        p.rect_filled(band, 0., CYAN.gamma_multiply(0.12));
        p.line_segment(
            [band.left_top(), band.right_top()],
            Stroke::new(0.7, CYAN.gamma_multiply(0.65)),
        );
        p.line_segment(
            [band.left_top(), band.left_top() + Vec2::new(0., 4.)],
            Stroke::new(0.7, CYAN),
        );
        p.line_segment(
            [band.right_top(), band.right_top() + Vec2::new(0., 4.)],
            Stroke::new(0.7, CYAN),
        );
    }
    p.line_segment(
        [egui::pos2(left, axis + 1.), egui::pos2(right, axis + 1.)],
        Stroke::new(2., Color32::BLACK.gamma_multiply(0.6)),
    );
    p.line_segment(
        [egui::pos2(left, axis), egui::pos2(right, axis)],
        Stroke::new(0.8, CYAN.gamma_multiply(0.65)),
    );
    for i in 0..=25 {
        let v = step * i as f64 / 5.;
        let x = project(v);
        let major = i % 5 == 0;
        p.line_segment(
            [
                egui::pos2(x, axis - if major { 3.5 } else { 1.5 }),
                egui::pos2(x, axis + if major { 3.5 } else { 1.5 }),
            ],
            Stroke::new(0.6, MUTED.gamma_multiply(if major { 1. } else { 0.45 })),
        );
        if major {
            let mut label = if step >= 1000. {
                format!("{:.0}", v / 1000.)
            } else {
                format!("{:.1}", v / 1000.).replace('.', ",")
            };
            if i == 25 {
                label += " км";
            }
            p.text(
                egui::pos2(x, axis + 8.),
                if i == 25 {
                    Align2::RIGHT_TOP
                } else {
                    Align2::CENTER_TOP
                },
                label,
                FontId::proportional(7.),
                MUTED,
            );
        }
    }
    if view.ranges.own_tackle > 0. {
        let y = axis + 26.;
        let end = project(view.ranges.own_tackle);
        p.text(
            egui::pos2(left - 8., y),
            Align2::RIGHT_CENTER,
            format!(
                "твой {} · {}",
                if view.ranges.own_scram {
                    "скрам"
                } else {
                    "поинт"
                },
                range_label(view.ranges.own_tackle)
            ),
            FontId::proportional(7.5),
            CYAN,
        );
        p.line_segment(
            [egui::pos2(left, y), egui::pos2(end, y)],
            Stroke::new(1.2, CYAN.gamma_multiply(0.65)),
        );
        p.line_segment(
            [egui::pos2(end, y - 3.), egui::pos2(end, y + 3.)],
            Stroke::new(1., CYAN),
        );
    }
    if let Some(v) = reach {
        let x = project(v);
        p.line_segment(
            [egui::pos2(x, axis - 6.), egui::pos2(x, axis + 6.)],
            Stroke::new(1.2, AMBER),
        );
        p.text(
            egui::pos2(right, rect.bottom() - 3.),
            Align2::RIGHT_BOTTOM,
            format!(
                "{} {}",
                if view.weapon["kind"] == "drones" {
                    "ДРОНЫ"
                } else {
                    "ОРУЖИЕ"
                },
                view.weapon["label"]
                    .as_str()
                    .unwrap_or("—")
                    .trim_start_matches("до ")
            ),
            FontId::proportional(7.5),
            AMBER,
        );
    }
    if let Some(value) = view.current.filter(|v| v.is_finite() && *v >= 0.) {
        let now = ui.input(|i| i.time);
        // Interpolate the displayed fraction, including a change of axis scale.
        let (position, moving) = motion.sample(view.key, value / max, now, view.interval);
        let center = egui::pos2(left + (right - left) * position.clamp(0., 1.) as f32, axis);
        p.text(
            egui::pos2(center.x.clamp(left + 34., right - 34.), rect.top()),
            Align2::CENTER_TOP,
            format!("ЦЕЛЬ {}", range_label(value)),
            FontId::proportional(8.),
            TEXT,
        );
        p.line_segment(
            [
                egui::pos2(center.x, rect.top() + 11.),
                center - Vec2::new(0., 10.),
            ],
            Stroke::new(0.65, PURPLE.gamma_multiply(0.5)),
        );
        if ui.is_rect_visible(rect) {
            // Only the halo breathes; radius and marker position stay meaningful.
            let pulse = (0.5 - 0.5 * (now * std::f64::consts::TAU / 3.6).cos()) as f32;
            for (radius, alpha) in [(14., 0.025), (11., 0.04), (8., 0.06)] {
                p.circle_filled(
                    center,
                    radius,
                    PURPLE.gamma_multiply(alpha * (0.6 + 0.4 * pulse)),
                );
            }
            ui.ctx()
                .request_repaint_after(Duration::from_millis(if moving { 16 } else { 50 }));
        }
        p.circle_stroke(center, 8., Stroke::new(0.6, PURPLE.gamma_multiply(0.65)));
        for d in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
            p.line_segment(
                [center + d * 9., center + d * 11.],
                Stroke::new(0.6, PURPLE.gamma_multiply(0.8)),
            );
        }
        p.circle_filled(
            center + Vec2::new(0.8, 2.8),
            5.5,
            Color32::BLACK.gamma_multiply(0.55),
        );
        p.circle_filled(center, 4.7, Color32::from_rgb(60, 31, 93));
        for i in 0..12 {
            let t = i as f32 / 11.;
            p.circle_filled(
                center + Vec2::new(-1.25, -1.55) * t,
                4.5 * (1. - 0.72 * t),
                Color32::from_rgb(104, 61, 164).lerp_to_gamma(Color32::from_rgb(221, 188, 255), t),
            );
        }
        p.circle_filled(
            center + Vec2::new(-1.8, -2.5),
            0.9,
            Color32::WHITE.gamma_multiply(0.75),
        );
    } else {
        motion.initialized = false;
    }
    response.on_hover_text("Скрам, сетка и нейтрики — по выбранной сборке противника. Нейтрики: optimal + ¼ falloff, эффект за optimal снижается. Бирюзовая граница — твой поинт или скрам. Фиолетовая бусинка — распознанная дистанция. Оружие — расчётная дальность выбранного фита.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn alerts_use_fresh_crossings_hysteresis_and_reset_on_gaps_or_fit_changes() {
        let mut alerts = Alerts::default();
        let now = Instant::now();
        let settings = Settings {
            sound_tackle: true,
            sound_point: true,
            ..Default::default()
        };
        let ranges = Ranges {
            scram: 9000.,
            own_tackle: 24000.,
            ..Default::default()
        };
        assert_eq!(alerts.observe("a", 18000., ranges, &settings, now), None);
        assert_eq!(
            alerts.observe("a", 8900., ranges, &settings, now),
            Some(Cue::Tackle)
        );
        assert_eq!(alerts.observe("a", 8500., ranges, &settings, now), None);
        assert_eq!(
            alerts.observe("a", 8800., ranges, &settings, now + Duration::from_secs(2)),
            None
        );
        alerts.suspend();
        assert_eq!(
            alerts.observe("a", 25000., ranges, &settings, now + Duration::from_secs(4)),
            None
        );
        assert_eq!(
            alerts.observe("a", 18000., ranges, &settings, now + Duration::from_secs(5)),
            None
        );
        assert_eq!(
            alerts.observe("a", 25000., ranges, &settings, now + Duration::from_secs(6)),
            Some(Cue::Point)
        );
        assert_eq!(
            alerts.observe(
                "new fit",
                8500.,
                ranges,
                &settings,
                now + Duration::from_secs(8)
            ),
            None
        );
    }
    #[test]
    fn motion_retargets_without_jump_and_snaps_on_new_target() {
        let mut m = Motion::default();
        assert_eq!(m.sample("a", 10000., 0., 1.5).0, 10000.);
        assert_eq!(m.sample("a", 20000., 1., 1.5).0, 10000.);
        let half = m.sample("a", 20000., 1.07, 1.5).0;
        assert!(half > 10000. && half < 20000.);
        assert!((m.sample("a", 15000., 1.07, 1.5).0 - half).abs() < 0.001);
        assert!((m.sample("a", 15000., 1.3, 1.5).0 - 15000.).abs() < 0.001);
        assert_eq!(m.sample("b", 40000., 1.31, 1.5).0, 40000.);
    }
}
