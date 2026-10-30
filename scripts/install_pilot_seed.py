#!/usr/bin/env python3
"""Attach verified compact annual player evidence to a prepared release seed."""
import argparse
import datetime as dt
import gzip
import hashlib
import json
from pathlib import Path
import sqlite3


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seed-directory',required=True,type=Path)
    parser.add_argument('--observations',required=True,type=Path)
    parser.add_argument('--model',required=True,type=Path)
    args=parser.parse_args()
    model=json.loads(gzip.decompress(args.model.read_bytes()))
    if model['schema']!=1 or model['policy']!='pilot-evidence-v3' or not model['evidence']['fit']['success']:
        raise ValueError('Invalid or unconverged annual rating model')
    seed=args.seed_directory/'seed.sqlite3'
    manifest_path=args.seed_directory/'seed-manifest.json'
    manifest=json.loads(manifest_path.read_text())
    db=sqlite3.connect(seed,uri=True)
    db.execute('PRAGMA cache_size=-16384')
    db.execute('ATTACH DATABASE ? AS annual_source',(args.observations.resolve().as_uri()+'?mode=ro',))
    days=db.execute('SELECT count(*),min(day),max(day) FROM annual_source.annual_rating_days WHERE policy=?',(model['policy'],)).fetchone()
    if days!=(365,model['period_start'].replace('-',''),model['period_end'].replace('-','')):
        raise ValueError('Incomplete/mismatched annual source dates')
    count=db.execute('SELECT count(*) FROM annual_source.annual_rating_duels').fetchone()[0]
    if count!=model['evidence']['filtered_outcomes']:
        raise ValueError('Observation count differs from model fitting data')
    info={'model':model['version'],'model_sha256':hashlib.sha256(args.model.read_bytes()).hexdigest(),
          'days':365,'period_start':model['period_start'],'period_end':model['period_end'],
          'policy':model['policy'],'outcomes':count,'participants':len(model['players']),
          'rated_players':model['calibration_players'],'source_records':model['evidence']['source_records']}
    built=dt.datetime.now(dt.timezone.utc).isoformat()
    with db:
        db.execute('DROP TABLE IF EXISTS main.annual_rating_duels')
        db.execute('DROP TABLE IF EXISTS main.annual_rating_days')
        db.execute('CREATE TABLE annual_rating_duels(id INTEGER PRIMARY KEY,day TEXT NOT NULL,at INTEGER NOT NULL,winner INTEGER NOT NULL,loser INTEGER NOT NULL,winner_hull INTEGER NOT NULL,loser_hull INTEGER NOT NULL)')
        db.execute('CREATE TABLE annual_rating_days(day TEXT PRIMARY KEY,source_count INTEGER NOT NULL,policy TEXT NOT NULL)')
        db.execute('INSERT INTO annual_rating_duels SELECT * FROM annual_source.annual_rating_duels')
        db.execute('INSERT INTO annual_rating_days SELECT * FROM annual_source.annual_rating_days')
        db.execute('CREATE INDEX annual_rating_winner ON annual_rating_duels(winner,at)')
        db.execute('CREATE INDEX annual_rating_loser ON annual_rating_duels(loser,at)')
        db.execute('CREATE INDEX annual_rating_day ON annual_rating_duels(day)')
        db.execute('INSERT OR REPLACE INTO meta VALUES(?,?)',('pilot_rating',json.dumps(info)))
        db.execute('INSERT OR REPLACE INTO meta VALUES(?,?)',('built_at',json.dumps(built)))
    assert db.execute('SELECT count(*) FROM killmails').fetchone()[0]==manifest['records']
    assert db.execute('PRAGMA integrity_check').fetchone()[0]=='ok'
    db.close()
    with seed.open('rb') as stream:sha=hashlib.file_digest(stream,'sha256').hexdigest()
    manifest.update(pilot_rating=info,built_at=built,sha256=sha)
    manifest_path.write_text(json.dumps(manifest,indent=2)+'\n')
    print(json.dumps({'seed_bytes':seed.stat().st_size,'seed_sha256':sha,**info}),flush=True)


if __name__=='__main__':main()
