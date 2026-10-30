#!/usr/bin/env python3
"""Export compact annual observations for instant, local per-pilot recalculation."""
import argparse
import datetime as dt
import gzip
import json
from pathlib import Path
import shutil
import sqlite3


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory',required=True,type=Path)
    parser.add_argument('--end',required=True)
    parser.add_argument('--output',required=True,type=Path)
    args=parser.parse_args();end=dt.date.fromisoformat(args.end);start=end-dt.timedelta(days=364)
    catalog=json.loads((args.directory/'annual-rating-catalog.json').read_text())
    excluded=','.join(str(int(k)) for k,v in catalog['ships'].items() if v['group'] in [547,659])
    if args.output.exists():raise ValueError('Output already exists; choose a new export path')
    db=sqlite3.connect(args.output)
    db.execute('ATTACH DATABASE ? AS research',(str(args.directory/'history.sqlite3'),))
    bounds=(start.strftime('%Y%m%d'),end.strftime('%Y%m%d'))
    if db.execute("SELECT count(*) FROM research.days WHERE day BETWEEN ? AND ? AND policy='pilot-evidence-v3'",bounds).fetchone()[0]!=365:
        raise ValueError('Incomplete annual source coverage')
    db.executescript('''CREATE TABLE annual_rating_duels(id INTEGER PRIMARY KEY,day TEXT NOT NULL,at INTEGER NOT NULL,
        winner INTEGER NOT NULL,loser INTEGER NOT NULL,winner_hull INTEGER NOT NULL,loser_hull INTEGER NOT NULL);
        CREATE TABLE annual_rating_days(day TEXT PRIMARY KEY,source_count INTEGER NOT NULL,policy TEXT NOT NULL);''')
    with db:
        db.execute(f'''INSERT INTO annual_rating_duels SELECT id,day,at,winner,loser,winner_hull,loser_hull
            FROM research.duels WHERE day BETWEEN ? AND ? AND victim_armed=1 AND player_damage>0
            AND npc_damage*9<=player_damage AND (winner_corp=0 OR loser_corp=0 OR winner_corp!=loser_corp)
            AND winner_hull NOT IN ({excluded}) AND loser_hull NOT IN ({excluded})''',bounds)
        db.execute("INSERT INTO annual_rating_days SELECT day,published,policy FROM research.days WHERE day BETWEEN ? AND ?",bounds)
        db.execute('CREATE INDEX annual_rating_winner ON annual_rating_duels(winner,at)')
        db.execute('CREATE INDEX annual_rating_loser ON annual_rating_duels(loser,at)')
        db.execute('CREATE INDEX annual_rating_day ON annual_rating_duels(day)')
    count=db.execute('SELECT count(*) FROM annual_rating_duels').fetchone()[0]
    assert db.execute('PRAGMA integrity_check').fetchone()[0]=='ok'
    db.close()
    with args.output.open('rb') as source,gzip.open(str(args.output)+'.gz','wb',compresslevel=6) as target:
        shutil.copyfileobj(source,target,1024*1024)
    print(json.dumps({'period_start':str(start),'period_end':str(end),'days':365,'outcomes':count,
                      'database_bytes':args.output.stat().st_size,'compressed_bytes':Path(str(args.output)+'.gz').stat().st_size}),flush=True)


if __name__=='__main__':main()
