#!/usr/bin/env python3
"""Check a fixed SOLO model on a final month excluded from its training."""
import argparse
import datetime as dt
import json
from pathlib import Path

import numpy as np

from compare_pilot_rating_holdouts import day_bootstrap
from evaluate_pilot_ratings import established_players, score
from pilot_rating_models import (EloModel, PairwiseModel, SPECIFICATIONS,
                                 load_window, metrics, repeat_weights)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True, type=Path)
    parser.add_argument('--start', required=True)
    parser.add_argument('--end', required=True)
    parser.add_argument('--test-days', type=int, default=31)
    parser.add_argument('--minimum-battles', type=int, default=5)
    parser.add_argument('--minimum-opponents', type=int, default=3)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    begin, end = dt.date.fromisoformat(args.start), dt.date.fromisoformat(args.end)
    test_start = end - dt.timedelta(days=args.test_days-1)
    if not begin < test_start <= end or min(args.minimum_battles, args.minimum_opponents) < 1:
        parser.error('Invalid period or evidence threshold')
    rows = load_window(args.directory/'history.sqlite3', args.start, args.end)
    train = rows[rows['day'] < int(test_start.strftime('%Y%m%d'))]
    test = rows[rows['day'] >= int(test_start.strftime('%Y%m%d'))]
    eligible = established_players(train, args.minimum_battles, args.minimum_opponents)
    mask = np.isin(test['winner'], eligible) & np.isin(test['loser'], eligible)
    if not mask.any():
        raise ValueError('No eligible held-out outcomes')
    specification = next(s for s in SPECIFICATIONS if s.name == 'players_hulls_pairs16')
    models = {'five_year_pairwise': PairwiseModel(specification), 'five_year_elo_k48': EloModel(48)}
    report = {'period_start': args.start, 'period_end': args.end,
              'training_end': str(test_start-dt.timedelta(days=1)), 'test_start': str(test_start),
              'minimum_battles': args.minimum_battles, 'minimum_opponents': args.minimum_opponents,
              'test_outcomes': len(test), 'eligible_test_outcomes': int(mask.sum()), 'models': {}}
    predictions = {}
    for name, model in models.items():
        model.fit(train)
        if hasattr(model, 'result') and not model.result['success']:
            raise ValueError(model.result)
        predictions[name] = model.predict(test)[mask]
        result = score(model, test, train)
        result['five_year_eligible_repeat_capped'] = metrics(predictions[name], repeat_weights(test[mask]))
        if hasattr(model, 'result'):
            result['fit'] = model.result
        report['models'][name] = result
    report['comparison'] = day_bootstrap(test[mask], predictions['five_year_pairwise'], predictions['five_year_elo_k48'])
    report['passed'] = report['comparison']['ci95'][0] > 0
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2)+'\n')
    print(json.dumps({'passed': report['passed'], 'comparison': report['comparison']}), flush=True)


if __name__ == '__main__':
    main()
