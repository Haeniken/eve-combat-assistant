#!/usr/bin/env python3
"""Chronological model selection for a complete 365-day solo research window.

The first 304 days train, the next 30 select a candidate, and the final 31 are
held out. Candidate validation never uses later results or all-year ship stats.
"""
import argparse
from dataclasses import asdict
import datetime as dt
import json
from pathlib import Path
import time

import numpy as np

from pilot_rating_models import (AggregateModel, EloModel, PairwiseModel,
                                 SmoothedWinRateModel, SPECIFICATIONS,
                                 load_window, metrics, repeat_weights)


def pair_codes(rows):
    return ((np.minimum(rows['winner'], rows['loser']).astype(np.uint64) << np.uint64(32))
            | np.maximum(rows['winner'], rows['loser']).astype(np.uint64))


def established_players(rows, minimum_battles=20, minimum_opponents=5):
    players, games = np.unique(np.concatenate((rows['winner'], rows['loser'])), return_counts=True)
    pairs = np.unique(pair_codes(rows))
    neighbors = np.concatenate(((pairs >> np.uint64(32)).astype(np.int64),
                                (pairs & np.uint64(2**32-1)).astype(np.int64)))
    counts = np.bincount(np.searchsorted(players, neighbors), minlength=len(players))
    return players[(games >= minimum_battles) & (counts >= minimum_opponents)]


def calibration(rows, prediction):
    winner_is_first = rows['winner'] < rows['loser']
    predicted = np.where(winner_is_first, prediction, 1.-prediction)
    bins = np.minimum((predicted * 10).astype(int), 9)
    return [{'bin': i, 'n': int((bins == i).sum()),
             'prediction': float(predicted[bins == i].mean()),
             'observed': float(winner_is_first[bins == i].mean())}
            for i in range(10) if (bins == i).any()]


def score(model, rows, training):
    prediction = model.predict(rows)
    train_players = np.unique(np.concatenate((training['winner'], training['loser'])))
    train_pairs = np.unique(pair_codes(training))
    both_known = np.isin(rows['winner'], train_players) & np.isin(rows['loser'], train_players)
    unseen_pair = ~np.isin(pair_codes(rows), train_pairs)
    same_hull = rows['winner_hull'] == rows['loser_hull']
    established = established_players(training)
    established_pair = np.isin(rows['winner'], established) & np.isin(rows['loser'], established)
    result = {'all': metrics(prediction), 'repeat_capped': metrics(prediction, repeat_weights(rows)),
              'calibration': calibration(rows, prediction)}
    for key, mask in [('known_players', both_known), ('unseen_pairing', unseen_pair), ('same_hull', same_hull),
                      ('established_players', established_pair), ('established_same_hull', established_pair & same_hull)]:
        if mask.any():
            result[key] = metrics(prediction[mask])
            if key == 'established_players':
                result['established_repeat_capped'] = metrics(prediction[mask], repeat_weights(rows[mask]))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True, type=Path)
    parser.add_argument('--end', required=True)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--include-test', action='store_true', help='Also evaluate final held-out results')
    parser.add_argument('--model', action='append', help='Run only named candidates')
    args = parser.parse_args()
    end = dt.date.fromisoformat(args.end)
    start = end - dt.timedelta(days=364)
    rows = load_window(args.directory / 'history.sqlite3', start.isoformat(), end.isoformat())
    valid_begin = int((start + dt.timedelta(days=304)).strftime('%Y%m%d'))
    test_begin = int((start + dt.timedelta(days=334)).strftime('%Y%m%d'))
    train = rows[rows['day'] < valid_begin]
    valid = rows[(rows['day'] >= valid_begin) & (rows['day'] < test_begin)]
    test = rows[rows['day'] >= test_begin]
    report = {'window_start': start.isoformat(), 'window_end': end.isoformat(),
              'training_outcomes': len(train), 'validation_outcomes': len(valid),
              'test_outcomes': len(test), 'validation_start': str(valid_begin),
              'test_start': str(test_begin), 'candidates': {}, 'numpy': np.__version__}
    args.output.mkdir(parents=True, exist_ok=True)
    candidates = [('old_strength_proxy', AggregateModel()), ('smoothed_win_loss', SmoothedWinRateModel()),
                  ('elo_k24', EloModel(24.)), ('elo_k48', EloModel(48.))]
    candidates += [(s.name, PairwiseModel(s)) for s in SPECIFICATIONS]
    if args.model:
        candidates = [(name, model) for name, model in candidates if name in args.model]
        if len(candidates) != len(set(args.model)):
            parser.error('Unknown candidate model')
    for name, model in candidates:
        path = args.output / (name + '.json')
        if path.exists():
            old = json.loads(path.read_text())
            if old.get('protocol') == 4 and old.get('window_end') == args.end and (not args.include_test or 'test' in old):
                report['candidates'][name] = old
                continue
        began = time.monotonic()
        model.fit(train)
        result = {'protocol': 4, 'window_end': args.end, 'validation': score(model, valid, train),
                  'fit_seconds': round(time.monotonic()-began, 2)}
        if hasattr(model, 'specification'):
            result['specification'] = asdict(model.specification)
            result['fit'] = model.result
        if args.include_test:
            result['test'] = score(model, test, train)
        path.write_text(json.dumps(result, indent=2) + '\n')
        report['candidates'][name] = result
        print(json.dumps({'event': 'model_evaluated', 'model': name,
                          'validation_log_loss': result['validation']['repeat_capped']['log_loss'],
                          'validation_brier': result['validation']['repeat_capped']['brier'],
                          'seconds': result['fit_seconds']}), flush=True)
        (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    first = next(iter(report['candidates'].values()))['validation']
    subset = 'established_repeat_capped' if first.get('established_players', {}).get('outcomes', 0) >= 1000 else 'repeat_capped'
    report['selection_subset'] = subset
    rankers = [n for n in report['candidates'] if not n.startswith('hulls_only')]
    report['best_validation'] = min(rankers, key=lambda n: report['candidates'][n]['validation'][subset]['log_loss']) if rankers else None
    (args.output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'event': 'validation_complete', 'end': args.end,
                      'best': report['best_validation'], 'test_evaluated': args.include_test}), flush=True)


if __name__ == '__main__':
    main()
