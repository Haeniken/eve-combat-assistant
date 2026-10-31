//! Joint regularized player/hull/matchup fit; runs only on the manual update worker.
use super::{Duel, Model};
use chrono::{NaiveDate, TimeDelta};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    sync::atomic::{AtomicBool, Ordering},
};

struct Row {
    indices: [u32; 5],
    pair_sign: i8,
    weight: f64,
}
struct History {
    step: Vec<f64>,
    difference: Vec<f64>,
    inverse: f64,
}
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Relaxed) {
        Err("Пересчёт SOLO остановлен; загруженная история сохранена".into())
    } else {
        Ok(())
    }
}
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}
fn evaluate(
    parameters: &[f64],
    scale: &[f64],
    penalty: &[f64],
    rows: &[Row],
    cancel: &AtomicBool,
) -> Result<(f64, Vec<f64>), String> {
    let physical: Vec<_> = parameters.iter().zip(scale).map(|(x, s)| x * s).collect();
    let mut gradient: Vec<_> = physical.iter().zip(penalty).map(|(x, p)| x * p).collect();
    let mut loss = physical
        .iter()
        .zip(penalty)
        .map(|(x, p)| 0.5 * p * x * x)
        .sum::<f64>();
    for (n, row) in rows.iter().enumerate() {
        if n % 32768 == 0 {
            check(cancel)?;
        }
        let [a, b, h, k, p] = row.indices.map(|i| i as usize);
        let sign = row.pair_sign as f64;
        let z = physical[a] - physical[b] + physical[h] - physical[k]
            + if sign == 0. { 0. } else { sign * physical[p] };
        loss += row.weight
            * if z >= 0. {
                (-z).exp().ln_1p()
            } else {
                -z + z.exp().ln_1p()
            };
        let probability = if z >= 0. {
            1. / (1. + (-z).exp())
        } else {
            let e = z.exp();
            e / (1. + e)
        };
        let residual = (probability - 1.) * row.weight;
        gradient[a] += residual;
        gradient[b] -= residual;
        gradient[h] += residual;
        gradient[k] -= residual;
        if sign != 0. {
            gradient[p] += sign * residual;
        }
    }
    for (g, s) in gradient.iter_mut().zip(scale) {
        *g *= s;
    }
    if !loss.is_finite() || gradient.iter().any(|g| !g.is_finite()) {
        return Err("Неустойчивый расчёт модели SOLO".into());
    }
    Ok((loss, gradient))
}
fn optimize(
    initial: Vec<f64>,
    scale: &[f64],
    penalty: &[f64],
    rows: &[Row],
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> Result<(Vec<f64>, usize, f64), String> {
    let mut x = initial;
    let (mut loss, mut gradient) = evaluate(&x, scale, penalty, rows, cancel)?;
    let mut history: VecDeque<History> = VecDeque::new();
    for iteration in 0..400 {
        check(cancel)?;
        progress(iteration);
        let maximum = gradient.iter().fold(0_f64, |a, b| a.max(b.abs()));
        if maximum <= 1e-5 {
            return Ok((x, iteration, maximum));
        }
        let mut direction = gradient.clone();
        let mut alphas = Vec::with_capacity(history.len());
        for h in history.iter().rev() {
            let alpha = h.inverse * dot(&h.step, &direction);
            alphas.push(alpha);
            for (d, y) in direction.iter_mut().zip(&h.difference) {
                *d -= alpha * y;
            }
        }
        let gamma = history
            .back()
            .map(|h| dot(&h.step, &h.difference) / dot(&h.difference, &h.difference))
            .unwrap_or(1.);
        for d in &mut direction {
            *d *= gamma;
        }
        for (index, h) in history.iter().enumerate() {
            let beta = h.inverse * dot(&h.difference, &direction);
            let alpha = alphas[history.len() - index - 1];
            for (d, s) in direction.iter_mut().zip(&h.step) {
                *d += s * (alpha - beta);
            }
        }
        for d in &mut direction {
            *d = -*d;
        }
        let mut slope = dot(&gradient, &direction);
        if slope >= 0. {
            for (d, g) in direction.iter_mut().zip(&gradient) {
                *d = -*g;
            }
            slope = -dot(&gradient, &gradient);
            history.clear();
        }
        let mut step = 1.;
        let mut accepted = None;
        for _ in 0..30 {
            let candidate: Vec<_> = x
                .iter()
                .zip(&direction)
                .map(|(x, d)| x + step * d)
                .collect();
            let (value, g) = evaluate(&candidate, scale, penalty, rows, cancel)?;
            if value <= loss + 1e-4 * step * slope {
                accepted = Some((candidate, value, g));
                break;
            }
            step *= 0.5;
        }
        let (candidate, value, new_gradient) =
            accepted.ok_or("Не удалось завершить оптимизацию SOLO; прежняя модель сохранена")?;
        let displacement: Vec<_> = candidate.iter().zip(&x).map(|(a, b)| a - b).collect();
        let difference: Vec<_> = new_gradient
            .iter()
            .zip(&gradient)
            .map(|(a, b)| a - b)
            .collect();
        let curvature = dot(&displacement, &difference);
        if curvature > 1e-12 {
            if history.len() == 8 {
                history.pop_front();
            }
            history.push_back(History {
                step: displacement,
                difference,
                inverse: 1. / curvature,
            });
        }
        let relative = (loss - value).abs() / (1. + loss.abs());
        x = candidate;
        loss = value;
        gradient = new_gradient;
        let maximum = gradient.iter().fold(0_f64, |a, b| a.max(b.abs()));
        if relative <= 1e-12 && maximum < 1e-3 {
            return Ok((x, iteration + 1, maximum));
        }
    }
    Err("Модель SOLO не сошлась; прежняя оценка сохранена".into())
}

pub(super) fn train(
    raw: Vec<Duel>,
    template: &Model,
    end: NaiveDate,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize),
) -> Result<Model, String> {
    check(cancel)?;
    if raw.is_empty() {
        return Err("Нет подходящих соло-исходов".into());
    }
    let count = raw.len();
    let mut players: Vec<_> = raw.iter().flat_map(|r| [r.winner, r.loser]).collect();
    players.sort_unstable();
    players.dedup();
    players.shrink_to_fit();
    let mut hulls: Vec<_> = raw
        .iter()
        .flat_map(|r| [r.winner_hull, r.loser_hull])
        .collect();
    hulls.sort_unstable();
    hulls.dedup();
    hulls.shrink_to_fit();
    let mut pairs: Vec<_> = raw
        .iter()
        .filter(|r| r.winner_hull != r.loser_hull)
        .map(|r| {
            (
                r.winner_hull.min(r.loser_hull),
                r.winner_hull.max(r.loser_hull),
            )
        })
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    pairs.shrink_to_fit();
    let p = players.len();
    let h = hulls.len();
    let dimension = p + h + pairs.len();
    if dimension > u32::MAX as usize {
        return Err("Слишком большая модель SOLO".into());
    }
    let mut penalty = vec![template.specification.player_penalty; p];
    penalty.extend(vec![template.specification.hull_penalty; h]);
    penalty.extend(vec![template.specification.pair_penalty; pairs.len()]);
    let mut physical: Vec<_> = players
        .iter()
        .map(|id| template.player(*id).map(|p| p.1).unwrap_or(0.))
        .collect();
    physical.extend(hulls.iter().map(|id| template.hull(*id)));
    physical.extend(pairs.iter().map(|key| {
        template
            .pairs
            .binary_search_by_key(key, |r| (r.0, r.1))
            .ok()
            .map(|i| template.pairs[i].2)
            .unwrap_or(0.)
    }));
    let mut stats = vec![(0_u64, 0_u64, 0_u32, 0_i64); p];
    let mut repeats = Vec::with_capacity(count);
    let mut rows = Vec::with_capacity(count);
    let mut digest = Sha256::new();
    for (n, r) in raw.iter().enumerate() {
        if n % 32768 == 0 {
            check(cancel)?;
        }
        let winner = players.binary_search(&r.winner).unwrap();
        let loser = players.binary_search(&r.loser).unwrap();
        stats[winner].0 += 1;
        stats[loser].1 += 1;
        stats[winner].3 = stats[winner].3.max(r.at);
        stats[loser].3 = stats[loser].3.max(r.at);
        repeats.push((
            r.winner.min(r.loser),
            r.winner.max(r.loser),
            r.at.div_euclid(86400),
            n,
        ));
        let key = (
            r.winner_hull.min(r.loser_hull),
            r.winner_hull.max(r.loser_hull),
        );
        let pair = if key.0 == key.1 {
            0
        } else {
            p + h + pairs.binary_search(&key).unwrap()
        };
        rows.push(Row {
            indices: [
                winner as u32,
                loser as u32,
                (p + hulls.binary_search(&r.winner_hull).unwrap()) as u32,
                (p + hulls.binary_search(&r.loser_hull).unwrap()) as u32,
                pair as u32,
            ],
            pair_sign: (r.loser_hull - r.winner_hull).signum() as i8,
            weight: 1.,
        });
        digest.update(r.id.to_le_bytes());
        digest.update(r.at.to_le_bytes());
    }
    drop(raw);
    repeats.sort_unstable();
    let mut begin = 0;
    let mut previous_pair = None;
    while begin < repeats.len() {
        if begin % 32768 == 0 {
            check(cancel)?;
        }
        let (a, b, day, _) = repeats[begin];
        if previous_pair != Some((a, b)) {
            stats[players.binary_search(&a).unwrap()].2 += 1;
            stats[players.binary_search(&b).unwrap()].2 += 1;
            previous_pair = Some((a, b));
        }
        let mut end = begin + 1;
        while end < repeats.len() && (repeats[end].0, repeats[end].1, repeats[end].2) == (a, b, day)
        {
            end += 1;
        }
        let weight = (template.repeat_pair_day_cap as f64 / (end - begin) as f64).min(1.);
        for row in &repeats[begin..end] {
            rows[row.3].weight = weight;
        }
        begin = end;
    }
    drop(repeats);
    let mut diagonal = penalty.clone();
    for row in &rows {
        for index in &row.indices[..4] {
            diagonal[*index as usize] += 0.25 * row.weight;
        }
        if row.pair_sign != 0 {
            diagonal[row.indices[4] as usize] += 0.25 * row.weight;
        }
    }
    let scale: Vec<_> = diagonal.into_iter().map(|d| 1. / d.sqrt()).collect();
    let initial = physical.iter().zip(&scale).map(|(x, s)| x / s).collect();
    drop(physical);
    let (parameters, iterations, gradient) =
        optimize(initial, &scale, &penalty, &rows, cancel, progress)?;
    let parameters: Vec<_> = parameters
        .iter()
        .zip(&scale)
        .map(|(x, s)| (x * s * 1e8).round() / 1e8)
        .collect();
    drop(rows);
    let mut updated = template.clone();
    updated.players = players
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let (w, l, o, t) = stats[i];
            (*id, parameters[i], w, l, o, t)
        })
        .collect();
    updated.hulls = hulls
        .iter()
        .enumerate()
        .map(|(i, id)| (*id, parameters[p + i]))
        .collect();
    updated.pairs = pairs
        .iter()
        .enumerate()
        .map(|(i, (a, b))| (*a, *b, parameters[p + h + i]))
        .collect();
    let mut strengths: Vec<_> = updated
        .players
        .iter()
        .filter(|p| p.2 + p.3 >= updated.minimum_battles && p.4 >= updated.minimum_opponents)
        .map(|p| p.1)
        .collect();
    strengths.sort_unstable_by(f64::total_cmp);
    if strengths.len() < 100 {
        return Err("Недостаточно пилотов для калибровки SOLO".into());
    }
    let cohort = strengths.len();
    let mut knots = Vec::new();
    let mut i = 0;
    while i < cohort {
        let mut j = i + 1;
        while j < cohort && strengths[j] == strengths[i] {
            j += 1;
        }
        knots.push([
            strengths[i],
            (i as f64 + (j - i - 1) as f64 / 2.) / (cohort - 1) as f64,
        ]);
        i = j;
    }
    updated.calibration_knots = knots;
    let start = end - TimeDelta::days(updated.window_days - 1);
    updated.period_start = start.to_string();
    updated.period_end = end.to_string();
    updated.anchor = end.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp();
    for value in &parameters {
        digest.update(value.to_le_bytes());
    }
    updated.version = format!(
        "local-pairwise-v1-{}-{:x}",
        end.format("%Y%m%d"),
        digest.finalize()
    );
    updated
        .extra
        .insert("calibration_players".into(), serde_json::json!(cohort));
    updated.extra.insert("evidence".into(),serde_json::json!({"filtered_outcomes":count,"source_days":updated.window_days,"source":"client_local_complete_history","fit":{"success":true,"iterations":iterations,"gradient_scaled_maxabs":gradient}}));
    check(cancel)?;
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn analytic_gradient_matches_player_hull_and_pair_likelihood() {
        let flag = AtomicBool::new(false);
        let rows = [
            Row {
                indices: [0, 1, 2, 3, 4],
                pair_sign: 1,
                weight: 0.3,
            },
            Row {
                indices: [1, 0, 3, 2, 4],
                pair_sign: -1,
                weight: 1.,
            },
            Row {
                indices: [0, 1, 2, 2, 0],
                pair_sign: 0,
                weight: 1.,
            },
        ];
        let parameters = [0.4, -0.3, 0.2, -0.1, 0.7];
        let scale = [0.3, 0.4, 0.2, 0.1, 0.5];
        let penalty = [4., 4., 8., 8., 16.];
        let (_, gradient) = evaluate(&parameters, &scale, &penalty, &rows, &flag).unwrap();
        for i in 0..parameters.len() {
            let mut plus = parameters;
            let mut minus = parameters;
            plus[i] += 1e-5;
            minus[i] -= 1e-5;
            let numerical = (evaluate(&plus, &scale, &penalty, &rows, &flag).unwrap().0
                - evaluate(&minus, &scale, &penalty, &rows, &flag).unwrap().0)
                / 2e-5;
            assert!((numerical - gradient[i]).abs() < 1e-6);
        }
    }
    #[test]
    fn local_solver_recovers_symmetric_results_and_respects_cancellation() {
        let rows: Vec<_> = (0..100)
            .map(|i| Row {
                indices: if i % 2 == 0 {
                    [0, 1, 2, 2, 0]
                } else {
                    [1, 0, 2, 2, 0]
                },
                pair_sign: 0,
                weight: 1.,
            })
            .collect();
        let flag = AtomicBool::new(false);
        let (result, _, _) = optimize(
            vec![1., -1., 0.],
            &[1.; 3],
            &[4.; 3],
            &rows,
            &flag,
            &mut |_| {},
        )
        .unwrap();
        assert!(result.iter().all(|x| x.abs() < 1e-5));
        assert!(
            optimize(
                vec![1., -1., 0.],
                &[1.; 3],
                &[4.; 3],
                &rows,
                &flag,
                &mut |_| flag.store(true, Ordering::Relaxed)
            )
            .is_err()
        );
        flag.store(true, Ordering::Relaxed);
        assert!(optimize(vec![0.; 3], &[1.; 3], &[4.; 3], &rows, &flag, &mut |_| {}).is_err());
    }
}
