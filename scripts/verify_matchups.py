#!/usr/bin/env python3
"""Verify daily reference coverage and incorporate changed archive days."""
import argparse
from collections import Counter
import json
from pathlib import Path
import sqlite3
import time
from build_bootstrap import download, BASE, reconcile_archive
from build_matchups import POLICY, MIN_BATTLES, duel, records, quantile
import gzip


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory',type=Path)
    parser.add_argument('--raw-directories',nargs='+',type=Path,required=True)
    parser.add_argument('--totals',type=Path,required=True)
    args=parser.parse_args()
    root=Path(__file__).resolve().parents[1]
    ships={r['id']:r for r in json.loads(gzip.decompress((root/'resources/catalog.json.gz').read_bytes()))['types'] if r['category']==6}
    totals=json.loads(args.totals.read_text())
    db=sqlite3.connect(args.directory/'matchups.sqlite3')
    db.execute('CREATE TABLE IF NOT EXISTS source_verification(day TEXT PRIMARY KEY,published INTEGER NOT NULL,actual INTEGER NOT NULL,index_verified INTEGER NOT NULL)')
    report=json.loads((args.directory/'report.json').read_text())
    coverage=list(db.execute('SELECT day,source_count FROM hull_matchup_days ORDER BY day'))
    if len(coverage)!=365:
        raise ValueError('Expected all 365 complete days')
    corrected=[]
    discrepancies=[]
    for day,previous in coverage:
        expected=int(totals.get(day,0))
        if not expected:
            raise ValueError('Missing daily source total: '+day)
        if expected==previous:
            with db: db.execute('INSERT OR IGNORE INTO source_verification VALUES(?,?,?,0)',(day,expected,previous))
            continue
        paths=[d/(day+'.json') for d in args.raw_directories]
        path=next((p for p in paths if p.exists()),args.directory/'raw'/(day+'.json'))
        # First recheck the current daily ID index if available locally.
        index_path=next((d.parent/('index-'+day+'.json') for d in args.raw_directories if (d.parent/('index-'+day+'.json')).exists()),args.directory/('index-'+day+'.json'))
        if not index_path.exists():
            download(f'{BASE}/{day}.json',index_path)
            time.sleep(1.2)
        index=set(json.loads(index_path.read_text()))
        ids={str(row['killmail_id']) for row in records(path)}
        if not index or not index.issubset(ids):
            path=args.directory/'raw'/(day+'.json')
            download(f'{BASE}/raw/{day}.json',path)
            time.sleep(1.2)
            ids={str(row['killmail_id']) for row in records(path)}
            if not index or not index.issubset(ids):
                rows,extra=reconcile_archive(json.loads(path.read_bytes()),json.loads(index_path.read_text()),day,args.directory)
                temporary=path.with_suffix('.reconciled.tmp')
                temporary.write_text(json.dumps(rows,separators=(',',':')))
                temporary.replace(path)
                ids=set(rows)
                del rows
        with db:
            db.execute('DELETE FROM hull_duels WHERE day=?',(day,))
            db.execute('DELETE FROM hull_matchups_daily WHERE day=?',(day,))
            diagnostics=Counter()
            for mail in records(path):
                result,reason=duel(mail,ships)
                diagnostics[reason]+=1
                if result:
                    if mail.get('killmail_time','')[:10].replace('-','')!=day:
                        raise ValueError('Archive date mismatch: '+day)
                    db.execute('INSERT OR IGNORE INTO hull_duels VALUES(?,?,?,?)',(mail['killmail_id'],day,*result))
            db.execute('''INSERT INTO hull_matchups_daily SELECT day,min(winner,loser),max(winner,loser),sum(winner<=loser),sum(winner>loser) FROM hull_duels WHERE day=? GROUP BY day,min(winner,loser),max(winner,loser)''',(day,))
            n=db.execute('SELECT count(*) FROM hull_duels WHERE day=?',(day,)).fetchone()[0]
            db.execute('INSERT OR REPLACE INTO hull_matchup_days VALUES(?,?,?,?)',(day,expected,n,POLICY))
            db.execute('INSERT OR REPLACE INTO source_verification VALUES(?,?,?,1)',(day,expected,len(ids)))
        change={'day':day,'previous':previous,'published':expected,'available':len(ids),'duels':n,'daily_id_index_verified':True,'extra_raw_records':len(ids-index)}
        corrected.append(change)
        if len(ids)!=expected: discrepancies.append(change)
        print(json.dumps({'event':'verified_changed_day',**change}),flush=True)
    pairs=list(db.execute('SELECT low,high,sum(low_wins),sum(high_wins) FROM hull_matchups_daily GROUP BY low,high'))
    directed=sorted(p for a,b,w,l in pairs if a!=b and w+l>=MIN_BATTLES for p in [100*w/(w+l),100*l/(w+l)])
    deviations=sorted(abs(p-50) for p in directed)
    report.update({
        'source_records':db.execute('SELECT sum(actual) FROM source_verification').fetchone()[0],
        'eligible_killmails':db.execute('SELECT count(*) FROM hull_duels').fetchone()[0],
        'unique_hull_pairs':len(pairs),'verified_days':365,
        'calibration_directed_pairs':len(directed),
        'winrate_quantiles_percent':{p:quantile(directed,float(p)/100) for p in report['winrate_quantiles_percent']},
        'absolute_deviation_quantiles_pp':{p:quantile(deviations,float(p)/100) for p in report['absolute_deviation_quantiles_pp']},
        'changed_days_verified':corrected,'index_discrepancies':discrepancies,
        'sample_sizes':{str(n):sum(a!=b and w+l>=n for a,b,w,l in pairs) for n in [1,20,30,50,100,200]},
        'examples':[{ 'own':ships[a]['name'],'enemy':ships[b]['name'],'wins':w,'losses':l,'winrate':100*w/(w+l)} for a,b,w,l in pairs if {ships[a]['name'],ships[b]['name']}=={'Harpy','Stabber'}],
    })
    (args.directory/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    with db: db.execute('INSERT OR REPLACE INTO meta VALUES(?,?)',('report',json.dumps(report)))
    db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    print(json.dumps({'event':'year_verified','days':365,'source_records':report['source_records'],'eligible_killmails':report['eligible_killmails'],'changed_days':len(corrected)}),flush=True)


if __name__=='__main__':
    main()
