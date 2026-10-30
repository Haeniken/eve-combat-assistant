#!/usr/bin/env python3
"""Offline solo-rating experiments; requires NumPy and SciPy on the research host.

The player and ship effects are learned jointly, with independent Gaussian
priors. This separates persistent player results from observed hull advantages;
it cannot recover hidden support or battles without a recorded destruction.
"""
from dataclasses import dataclass
import datetime as dt
import math
import json
from pathlib import Path
import sqlite3

import numpy as np
from scipy.optimize import minimize
from scipy.special import expit


DTYPE = np.dtype([(key, '<i8') for key in ('id', 'day', 'at', 'winner', 'loser', 'winner_hull', 'loser_hull')])


def load_window(database, start, end):
    """Only complete days, armed victims, no dominant NPC damage or known corp mates."""
    db = sqlite3.connect(f'file:{database}?mode=ro', uri=True)
    begin, finish = start.replace('-', ''), end.replace('-', '')
    count = db.execute("SELECT count(*) FROM days WHERE day BETWEEN ? AND ? AND policy='pilot-evidence-v3'", (begin, finish)).fetchone()[0]
    expected = (dt.date.fromisoformat(end) - dt.date.fromisoformat(start)).days + 1
    if count != expected:
        raise ValueError(f'Incomplete research window {start}..{end}: {count}/{expected} days')
    sql = '''SELECT id,CAST(day AS INTEGER),at,winner,loser,winner_hull,loser_hull
      FROM duels WHERE day BETWEEN ? AND ? AND victim_armed=1
      AND player_damage>0 AND npc_damage*9<=player_damage
      AND (winner_corp=0 OR loser_corp=0 OR winner_corp!=loser_corp)
      ORDER BY at,id'''
    catalog=json.loads((Path(database).parent/'annual-rating-catalog.json').read_text())
    excluded=sorted(int(k) for k,v in catalog['ships'].items() if v['group'] in [547,659])
    # Fighters are not present in the pinned fitting catalog. Do not retain
    # carrier wins while silently dropping their apparently "unarmed" losses.
    exclusion=','.join('?' for _ in excluded)
    sql=sql.replace('ORDER BY at,id',f'AND winner_hull NOT IN ({exclusion}) AND loser_hull NOT IN ({exclusion}) ORDER BY at,id')
    rows = np.fromiter(db.execute(sql, (begin, finish,*excluded,*excluded)), dtype=DTYPE)
    db.close()
    return rows


def repeat_weights(rows):
    """Cap repeated same-pair destructions to three effective outcomes per UTC day."""
    keys = np.rec.fromarrays([np.minimum(rows['winner'], rows['loser']),
                              np.maximum(rows['winner'], rows['loser']), rows['day']])
    _, inverse, counts = np.unique(keys, return_inverse=True, return_counts=True)
    return np.minimum(1., 3. / counts[inverse])


def metrics(probability, weights=None):
    p = np.clip(probability, 1e-9, 1. - 1e-9)
    return {'outcomes': len(p), 'log_loss': float(np.average(-np.log(p), weights=weights)),
            'brier': float(np.average((1. - p)**2, weights=weights)),
            'accuracy': float(np.average((p > .5) + .5 * (p == .5), weights=weights))}


def positions(keys, values):
    index = np.searchsorted(keys, values)
    safe = np.minimum(index, len(keys)-1)
    return safe, (index < len(keys)) & (keys[safe] == values)


@dataclass(frozen=True)
class Specification:
    name: str
    player_penalty: float = 4.
    hull_penalty: float = 8.
    pair_penalty: float = 32.
    hulls: bool = False
    pairs: bool = False
    half_life_days: float = 0.
    player_effects: bool = True


SPECIFICATIONS = [
    Specification('players_ridge1', player_penalty=1.),
    Specification('players_ridge4'),
    Specification('players_ridge16', player_penalty=16.),
    Specification('players_hulls', hulls=True),
    Specification('players_hulls_pairs16', hulls=True, pairs=True, pair_penalty=16.),
    Specification('players_hulls_pairs64', hulls=True, pairs=True, pair_penalty=64.),
    Specification('players_hulls_pairs64_decay180', hulls=True, pairs=True, pair_penalty=64., half_life_days=180.),
    Specification('hulls_only_pairs16', hulls=True, pairs=True, pair_penalty=16., player_effects=False),
]


class PairwiseModel:
    def __init__(self, specification):
        self.specification = specification

    def fit(self, rows):
        s = self.specification
        self.players = np.unique(np.concatenate((rows['winner'], rows['loser'])))
        self.hulls = np.unique(np.concatenate((rows['winner_hull'], rows['loser_hull'])))
        p = len(self.players) if s.player_effects else 0
        h = len(self.hulls) if s.hulls else 0
        low = np.minimum(rows['winner_hull'], rows['loser_hull'])
        high = np.maximum(rows['winner_hull'], rows['loser_hull'])
        codes = low * 2**32 + high
        self.pairs = np.unique(codes[low != high]) if s.pairs else np.array([], dtype=np.int64)
        dimension = p + h + len(self.pairs)
        winner = np.searchsorted(self.players, rows['winner'])
        loser = np.searchsorted(self.players, rows['loser'])
        wh = np.searchsorted(self.hulls, rows['winner_hull']) if h else None
        lh = np.searchsorted(self.hulls, rows['loser_hull']) if h else None
        if len(self.pairs):
            pair = np.searchsorted(self.pairs, codes).clip(0, len(self.pairs)-1)
            sign = np.sign(rows['loser_hull'] - rows['winner_hull'])
        else:
            pair = sign = None
        weight = repeat_weights(rows)
        if s.half_life_days:
            weight *= np.exp2(-(rows['at'].max() - rows['at']) / (s.half_life_days * 86400))
        penalty = np.concatenate((np.full(p, s.player_penalty), np.full(h, s.hull_penalty),
                                  np.full(len(self.pairs), s.pair_penalty)))
        diagonal = penalty.copy()
        if p:
            diagonal[:p] += .25 * (np.bincount(winner, weight, minlength=p) + np.bincount(loser, weight, minlength=p))
        if h:
            diagonal[p:p+h] += .25 * (np.bincount(wh, weight, minlength=h) + np.bincount(lh, weight, minlength=h))
        if pair is not None:
            diagonal[p+h:] += .25 * np.bincount(pair, weight * sign**2, minlength=len(self.pairs))
        scale = 1. / np.sqrt(diagonal)

        def evaluate(parameters):
            z = parameters[winner] - parameters[loser] if p else np.zeros(len(rows))
            if h:
                z += parameters[p+wh] - parameters[p+lh]
            if pair is not None:
                z += sign * parameters[p+h+pair]
            residual = (expit(z) - 1.) * weight
            gradient = penalty * parameters
            if p:
                gradient[:p] += np.bincount(winner, residual, minlength=p) - np.bincount(loser, residual, minlength=p)
            if h:
                gradient[p:p+h] += np.bincount(wh, residual, minlength=h) - np.bincount(lh, residual, minlength=h)
            if pair is not None:
                gradient[p+h:] += np.bincount(pair, sign * residual, minlength=len(self.pairs))
            loss = np.dot(weight, np.logaddexp(0., -z)) + .5 * np.dot(penalty * parameters, parameters)
            return loss, gradient

        def scaled(parameters):
            loss, gradient = evaluate(parameters * scale)
            return loss, gradient * scale

        optimum = minimize(scaled, np.zeros(dimension), jac=True, method='L-BFGS-B',
                           options={'maxiter': 800, 'ftol': 1e-12, 'gtol': 1e-5, 'maxls': 30})
        self.parameters = optimum.x * scale
        _, gradient = evaluate(self.parameters)
        self.result = {'success': bool(optimum.success), 'iterations': int(optimum.nit),
                       'message': str(optimum.message), 'objective': float(optimum.fun),
                       'gradient_maxabs': float(np.max(np.abs(gradient))),
                       'players': p, 'hulls': h, 'pairs': len(self.pairs)}
        if not np.isfinite(self.parameters).all():
            raise ValueError('Non-finite rating model')
        return self

    def predict(self, rows):
        p = len(self.players) if self.specification.player_effects else 0
        h = len(self.hulls) if self.specification.hulls else 0
        wi, known_w = positions(self.players, rows['winner'])
        li, known_l = positions(self.players, rows['loser'])
        z = self.parameters[wi] * known_w - self.parameters[li] * known_l if p else np.zeros(len(rows))
        if h:
            wi, known_w = positions(self.hulls, rows['winner_hull'])
            li, known_l = positions(self.hulls, rows['loser_hull'])
            z += self.parameters[p+wi] * known_w - self.parameters[p+li] * known_l
        if len(self.pairs):
            low = np.minimum(rows['winner_hull'], rows['loser_hull'])
            high = np.maximum(rows['winner_hull'], rows['loser_hull'])
            pair, known = positions(self.pairs, low * 2**32 + high)
            sign = np.sign(rows['loser_hull'] - rows['winner_hull'])
            z += sign * self.parameters[p+h+pair] * known
        return expit(z)


class AggregateModel:
    def fit(self, rows):
        self.players = np.unique(np.concatenate((rows['winner'], rows['loser'])))
        wins = np.bincount(np.searchsorted(self.players, rows['winner']), minlength=len(self.players))
        losses = np.bincount(np.searchsorted(self.players, rows['loser']), minlength=len(self.players))
        strength = (wins + 5.) / (wins + losses + 150.)
        # The old index has no probability model; this is an explicit BT proxy
        # from its positive intermediate strengths, without percentile colors.
        self.logits = np.log(strength)
        self.default = math.log(5. / 150.)
        return self

    def predict(self, rows):
        wi, known_w = positions(self.players, rows['winner'])
        li, known_l = positions(self.players, rows['loser'])
        return expit(np.where(known_w, self.logits[wi], self.default)
                     - np.where(known_l, self.logits[li], self.default))


class SmoothedWinRateModel(AggregateModel):
    def fit(self, rows):
        self.players = np.unique(np.concatenate((rows['winner'], rows['loser'])))
        wins = np.bincount(np.searchsorted(self.players, rows['winner']), minlength=len(self.players))
        losses = np.bincount(np.searchsorted(self.players, rows['loser']), minlength=len(self.players))
        self.logits = np.log((wins + 5.) / (losses + 5.))
        self.default = 0.
        return self


class EloModel:
    def __init__(self, k=24.):
        self.k = k

    def fit(self, rows):
        self.players = np.unique(np.concatenate((rows['winner'], rows['loser'])))
        winner = np.searchsorted(self.players, rows['winner'])
        loser = np.searchsorted(self.players, rows['loser'])
        strength = [0.] * len(self.players)
        for w, l, weight in zip(winner, loser, repeat_weights(rows)):
            probability = 1. / (1. + math.exp(max(-30., min(30., strength[l]-strength[w]))))
            change = self.k * math.log(10.) / 400. * weight * (1.-probability)
            strength[w] += change
            strength[l] -= change
        self.logits = np.asarray(strength)
        self.default = 0.
        return self

    predict = AggregateModel.predict
