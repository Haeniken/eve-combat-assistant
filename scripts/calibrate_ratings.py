#!/usr/bin/env python3
"""Derive the shared seven-color scale from recorded hull and lifetime pilot data."""
import argparse
import bisect
import json
import math
from pathlib import Path
import sqlite3
from build_matchups import MIN_BATTLES, quantile

COLORS = [
    ('red', 'Красный', [231, 91, 91]),
    ('orange', 'Оранжевый', [240, 148, 69]),
    ('yellow', 'Жёлтый', [231, 210, 96]),
    ('green', 'Зелёный', [116, 205, 118]),
    ('turquoise', 'Бирюзовый', [73, 213, 201]),
    ('blue', 'Синий', [95, 153, 241]),
    ('purple', 'Фиолетовый', [189, 131, 239]),
]


def pilot_score(wins, losses, knots):
    strength=(wins+5)/(wins+losses+150)
    index=bisect.bisect_left([k[0] for k in knots],strength)
    if index==0:
        percentile=0
    elif index==len(knots):
        percentile=1
    else:
        left,lo=knots[index-1]
        right,hi=knots[index]
        percentile=lo+(hi-lo)*(strength-left)/(right-left)
    return max(1000,min(8999,math.floor(1000+7999*percentile)))


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory',type=Path)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[1]
    report=json.loads((args.directory/'report.json').read_text())
    db=sqlite3.connect(args.directory/'matchups.sqlite3')
    pairs=list(db.execute('SELECT low,high,sum(low_wins),sum(high_wins) FROM hull_matchups_daily WHERE day>=? AND day<=? GROUP BY low,high', (report['period_start'].replace('-',''),report['period_end'].replace('-',''))))
    # The five-win requirement applies to the color reference only. One-sided
    # observed outcomes still display 0/100 when they meet MIN_BATTLES.
    calibration=[(a,b,w,l) for a,b,w,l in pairs if a!=b and w+l>=MIN_BATTLES and min(w,l)>=5]
    deviations=sorted(abs(100*w/(w+l)-50) for a,b,w,l in calibration)
    radii=[quantile(deviations,p) for p in [.25,.5,.75]]
    raw=[50-r for r in reversed(radii)]+[50+r for r in radii]
    boundaries=[math.floor(v+.5) for v in raw]
    assert len(set(boundaries))==6 and boundaries==sorted(boundaries)
    assert bisect.bisect_right(boundaries,50)==3
    reference=json.loads((root/'resources/pilot-rating-reference.json').read_text())
    scores=sorted(pilot_score(r['solo_kills'],r['solo_losses'],reference['knots']) for r in reference['samples'])
    percentiles=[.125,.25,.375,.625,.75,.875]
    elo_quantiles=[quantile(scores,p) for p in percentiles]
    elo_boundaries=[math.ceil(value/100)*100 for value in elo_quantiles]
    assert len(set(elo_boundaries))==6
    count=lambda vals,cuts:[sum(bisect.bisect_right(cuts,v)==tier for v in vals) for tier in range(7)]
    data={
        'schema_version':1,'colors':[{'id':k,'name':n,'rgb':rgb} for k,n,rgb in COLORS],
        'winrate':{
            'boundaries':boundaries,'minimum_battles':MIN_BATTLES,
            'classification':'rounded_display_percentage; 50 percent is green',
            'period_start':report['period_start'],'period_end':report['period_end'],
            'source_records':report['source_records'],'eligible_killmails':report['eligible_killmails'],
            'reference_pairs':len(calibration),'reference_duels':sum(w+l for a,b,w,l in calibration),
            'selection':'At least 50 solo outcomes and at least 5 wins in each direction. Same-hull pairs excluded from color calibration only. Each pair contributes equally.',
            'method':'Quartiles of absolute deviation from 50%; symmetric unrounded cutoffs, rounded to nearest whole percent for display classification.',
            'unrounded_boundaries':raw,
            'tier_reference_counts':count([int(100*w/(w+l)+.5) for a,b,w,l in calibration]+[int(100*l/(w+l)+.5) for a,b,w,l in calibration],boundaries),
        },
        'solo':{
            'boundaries':elo_boundaries,'reference_profiles':len(scores),
            'reference_model':reference['model_id'],'reference_percentiles':percentiles,'unrounded_boundaries':elo_quantiles,
            'tier_reference_counts':count(scores,elo_boundaries),
            'scope':'lifetime','score_min':1000,'score_max':8999,'reserved':[9000,9998],
            'method':'Empirical score quantiles at 12.5,25,37.5,62.5,75,87.5 percent, rounded upwards to a hundred; the central quarter is green. Score model and lifetime counters are unchanged.',
        },
    }
    (root/'resources/rating-tiers.json').write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
    (args.directory/'calibration.json').write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
    print(json.dumps(data,ensure_ascii=False,indent=2))


if __name__=='__main__':
    main()
