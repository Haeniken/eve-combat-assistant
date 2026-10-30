#!/usr/bin/env python3
"""Evaluate the locked current-year validation winner on untouched month blocks."""
import argparse
import datetime as dt
import json
from pathlib import Path

import numpy as np

from evaluate_pilot_ratings import established_players, score
from pilot_rating_models import (AggregateModel, EloModel, PairwiseModel, SmoothedWinRateModel,
                                 SPECIFICATIONS, load_window, repeat_weights)


def construct(name):
    if name == 'old_strength_proxy': return AggregateModel()
    if name == 'smoothed_win_loss': return SmoothedWinRateModel()
    if name.startswith('elo_k'): return EloModel(float(name[5:]))
    return PairwiseModel(next(s for s in SPECIFICATIONS if s.name == name))


def day_bootstrap(rows, selected, baseline):
    # Paired UTC-day blocks retain dependence between kills from the same day.
    weight = repeat_weights(rows)
    difference = np.log(np.clip(selected,1e-9,1.))-np.log(np.clip(baseline,1e-9,1.))
    days, index = np.unique(rows['day'],return_inverse=True)
    total = np.bincount(index,weight*difference,minlength=len(days))
    denominator = np.bincount(index,weight,minlength=len(days))
    rng=np.random.default_rng(20261030)
    samples=rng.integers(0,len(days),size=(3000,len(days)))
    averages=total[samples].sum(axis=1)/denominator[samples].sum(axis=1)
    return {'baseline_minus_selected_log_loss':float(total.sum()/denominator.sum()),
            'ci95':[float(v) for v in np.quantile(averages,[.025,.975])],
            'days':len(days),'resamples':3000}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory',required=True,type=Path)
    args=parser.parse_args()
    root=args.directory
    latest=json.loads((root/'validation/2026/report.json').read_text())
    selected=latest['best_validation']
    names=list(dict.fromkeys([selected,'elo_k48','smoothed_win_loss','old_strength_proxy',
                              'hulls_only_pairs16','players_ridge1']))
    locked={'selected':selected,'selection_subset':latest['selection_subset'],
            'selection_period':'2026 validation only; no held-out labels used','years':{}}
    (root/'locked-model-choice.json').write_text(json.dumps(locked,indent=2)+'\n')
    for year in range(2026,2021,-1):
        end=dt.date(year,10,29);start=end-dt.timedelta(days=364)
        rows=load_window(root/'history.sqlite3',str(start),str(end))
        valid_begin=int((start+dt.timedelta(days=304)).strftime('%Y%m%d'))
        test_begin=int((start+dt.timedelta(days=334)).strftime('%Y%m%d'))
        training=rows[rows['day']<valid_begin];test=rows[rows['day']>=test_begin]
        established=established_players(training)
        mask=np.isin(test['winner'],established)&np.isin(test['loser'],established)
        predictions={};results={}
        for name in names:
            model=construct(name).fit(training)
            if hasattr(model,'result') and not model.result['success']:
                raise ValueError(f'Unconverged {name} in {year}: {model.result}')
            predictions[name]=model.predict(test)
            results[name]=score(model,test,training)
        comparisons={name:day_bootstrap(test[mask],predictions[selected][mask],predictions[name][mask])
                     for name in names if name!=selected}
        locked['years'][str(year)]={'outcomes':len(test),'established_outcomes':int(mask.sum()),
                                   'metrics':results,'day_block_comparisons':comparisons}
        (root/'holdout-report.json').write_text(json.dumps(locked,indent=2)+'\n')
        print(json.dumps({'event':'held_out_year','year':year,'selected':selected,
                          'established_outcomes':int(mask.sum()),'comparisons':comparisons}),flush=True)
    current=locked['years']['2026']
    good=all(current['day_block_comparisons'].get(n,{'ci95':[0,0]})['ci95'][0]>0 for n in ['elo_k48','hulls_only_pairs16'])
    wins=sum(v['day_block_comparisons'].get('elo_k48',{'baseline_minus_selected_log_loss':0})['baseline_minus_selected_log_loss']>0 for v in locked['years'].values())
    good=good and wins>=3
    locked['quality_gate']={'passed':good,'current_year_improves_elo_and_hull_only_at_95_percent_day_block_ci':
                            all(current['day_block_comparisons'].get(n,{'ci95':[0,0]})['ci95'][0]>0 for n in ['elo_k48','hulls_only_pairs16']),
                            'eras_improved_over_elo':wins,'eras':5}
    (root/'holdout-report.json').write_text(json.dumps(locked,indent=2)+'\n')
    print(json.dumps({'event':'holdout_complete','quality_gate':locked['quality_gate']}),flush=True)


if __name__=='__main__':main()
