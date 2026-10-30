#!/usr/bin/env python3
"""Refit a validated pairwise model on a verified period and export its compact snapshot."""
import argparse
from dataclasses import asdict
import datetime as dt
import gzip
import hashlib
import json
from pathlib import Path
import sqlite3

import numpy as np

from evaluate_pilot_ratings import established_players
from pilot_rating_models import PairwiseModel, SPECIFICATIONS, load_window, repeat_weights


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True, type=Path)
    parser.add_argument('--end', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--start', help='First complete UTC day; defaults to a 365-day window')
    parser.add_argument('--minimum-battles', type=int, default=20)
    parser.add_argument('--minimum-opponents', type=int, default=5)
    args = parser.parse_args()
    specification = next((s for s in SPECIFICATIONS if s.name == args.model), None)
    if specification is None or not specification.player_effects:
        parser.error('Only validated pairwise models can be exported')
    end = dt.date.fromisoformat(args.end)
    start = dt.date.fromisoformat(args.start) if args.start else end-dt.timedelta(days=364)
    window_days = (end-start).days+1
    if window_days < 1 or args.minimum_battles < 1 or args.minimum_opponents < 1:
        parser.error('Invalid period or minimum evidence')
    rows = load_window(args.directory/'history.sqlite3', start.isoformat(), end.isoformat())
    model = PairwiseModel(specification).fit(rows)
    if not model.result['success']:
        raise ValueError(f'Model did not converge: {model.result}')
    eligible = established_players(rows, args.minimum_battles, args.minimum_opponents)
    p = len(model.players)
    h = len(model.hulls) if specification.hulls else 0
    strengths = np.round(model.parameters[:p], 8)
    wi = np.searchsorted(model.players, rows['winner'])
    li = np.searchsorted(model.players, rows['loser'])
    wins = np.bincount(wi, minlength=p)
    losses = np.bincount(li, minlength=p)
    last = np.zeros(p, dtype=np.int64)
    np.maximum.at(last, wi, rows['at']);np.maximum.at(last, li, rows['at'])
    lo = np.minimum(rows['winner'], rows['loser']).astype(np.uint64)
    hi = np.maximum(rows['winner'], rows['loser']).astype(np.uint64)
    distinct = np.unique((lo << np.uint64(32)) | hi)
    endpoints = np.concatenate(((distinct >> np.uint64(32)).astype(np.int64),
                                 (distinct & np.uint64(2**32-1)).astype(np.int64)))
    opponents = np.bincount(np.searchsorted(model.players, endpoints), minlength=p)
    cohort_strengths = np.sort(strengths[np.isin(model.players, eligible)])
    unique, indices, counts = np.unique(cohort_strengths, return_index=True, return_counts=True)
    percentile = (indices + (counts-1)/2) / (len(cohort_strengths)-1)
    knots = [[float(mu), float(rank)] for mu, rank in zip(unique, percentile)]
    if len(knots) < 100:
        raise ValueError('Insufficient established-player calibration cohort')
    catalog = json.loads((args.directory/'annual-rating-catalog.json').read_text())
    excluded=sorted(int(k) for k,v in catalog['ships'].items() if v['group'] in [547,659])
    source = sqlite3.connect(f'file:{args.directory / "history.sqlite3"}?mode=ro', uri=True)
    coverage = source.execute('SELECT day,actual,duels,archive_sha256 FROM days WHERE day BETWEEN ? AND ? ORDER BY day',
                              (start.strftime('%Y%m%d'),end.strftime('%Y%m%d'))).fetchall()
    source.close()
    digest = hashlib.sha256(json.dumps(coverage,separators=(',',':')).encode()).hexdigest()
    configuration = hashlib.sha256(json.dumps({'specification':asdict(specification),'excluded_hulls':excluded,'window_days':window_days,'minimum_battles':args.minimum_battles,'minimum_opponents':args.minimum_opponents},sort_keys=True).encode()).hexdigest()[:8]
    fitted = hashlib.sha256(np.round(model.parameters,8).tobytes()).hexdigest()[:8]
    document = {
        'schema': 1, 'version': f'annual-pairwise-v1-{end:%Y%m%d}-{digest[:8]}-{configuration}-{fitted}',
        'policy': 'pilot-evidence-v3', 'period_start': start.isoformat(), 'period_end': end.isoformat(),
        'anchor': int(rows['at'].max()), 'specification': asdict(specification),
        'window_days': window_days, 'minimum_battles': args.minimum_battles, 'minimum_opponents': args.minimum_opponents,
        'repeat_pair_day_cap': 3, 'minimum_player_damage_fraction': .9,
        'calibration_players': len(cohort_strengths), 'calibration_knots': knots,
        'players': [[int(identity), float(mu), int(w), int(l), int(o), int(t)]
                    for identity,mu,w,l,o,t in zip(model.players,strengths,wins,losses,opponents,last)],
        'hulls': [[int(identity),round(float(value),8)]
                  for identity,value in zip(model.hulls,model.parameters[p:p+h])],
        'pairs': [[int(code//2**32),int(code%2**32),round(float(value),8)]
                  for code,value in zip(model.pairs,model.parameters[p+h:])],
        'combat_items': catalog['combat_items'],
        'excluded_hulls': excluded,
        'evidence': {'filtered_outcomes': len(rows), 'source_records': sum(r[1] for r in coverage),
                     'source_days': len(coverage), 'coverage_sha256': digest, 'fit': model.result},
    }
    raw = json.dumps(document,separators=(',',':'),allow_nan=False).encode()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('wb') as out:
        with gzip.GzipFile(fileobj=out, mode='wb', compresslevel=9, mtime=0) as zipped:zipped.write(raw)
    report = {'version':document['version'],'period_start':start.isoformat(),'period_end':end.isoformat(),
              'players':p,'rated_players':len(eligible),'hulls':h,'pairs':len(model.pairs),
              'outcomes':len(rows),'uncompressed_bytes':len(raw),'compressed_bytes':args.output.stat().st_size,
              'sha256':hashlib.sha256(args.output.read_bytes()).hexdigest(),'fit':model.result}
    args.output.with_suffix('.report.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report),flush=True)


if __name__ == '__main__':
    main()
