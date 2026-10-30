"""Seed attachment must preserve the observation source and existing fit data."""
import datetime as dt
import gzip
import hashlib
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import unittest


class InstallTests(unittest.TestCase):
    def test_attachment_is_read_only_and_does_not_drop_same_named_source_tables(self):
        self.check_attachment(365)

    def test_five_year_attachment_uses_the_full_period(self):
        self.check_attachment(1826)

    def check_attachment(self, days):
        with tempfile.TemporaryDirectory() as folder:
            root=Path(folder);seed=root/'seed.sqlite3';observations=root/'observations.sqlite3';model=root/'model.json.gz'
            db=sqlite3.connect(seed);db.executescript('CREATE TABLE killmails(id INTEGER PRIMARY KEY);INSERT INTO killmails VALUES(1);CREATE TABLE meta(key TEXT PRIMARY KEY,value TEXT)');db.close()
            (root/'seed-manifest.json').write_text(json.dumps({'records':1}))
            db=sqlite3.connect(observations)
            db.executescript('CREATE TABLE annual_rating_duels(id INTEGER PRIMARY KEY,day TEXT,at INTEGER,winner INTEGER,loser INTEGER,winner_hull INTEGER,loser_hull INTEGER);CREATE TABLE annual_rating_days(day TEXT PRIMARY KEY,source_count INTEGER,policy TEXT)')
            begin=dt.date(2026,10,29)-dt.timedelta(days=days-1)
            db.executemany('INSERT INTO annual_rating_days VALUES(?,1,?)',[((begin+dt.timedelta(days=i)).strftime('%Y%m%d'),'pilot-evidence-v3') for i in range(days)])
            db.execute("INSERT INTO annual_rating_duels VALUES(1,'20261029',1793275200,100,200,587,593)");db.commit();db.close()
            content={'schema':1,'policy':'pilot-evidence-v3','version':'fixture','period_start':str(begin),'period_end':'2026-10-29',
                     'players':[[]],'calibration_players':1,'evidence':{'filtered_outcomes':1,'source_records':days,'fit':{'success':True}}}
            if days!=365:content['window_days']=days
            model.write_bytes(gzip.compress(json.dumps(content).encode()))
            before=hashlib.sha256(observations.read_bytes()).hexdigest()
            process=subprocess.run([sys.executable,str(Path(__file__).with_name('install_pilot_seed.py')),
                                    '--seed-directory',str(root),'--observations',str(observations),'--model',str(model)],capture_output=True,text=True)
            self.assertEqual(process.returncode,0,process.stderr)
            self.assertEqual(hashlib.sha256(observations.read_bytes()).hexdigest(),before)
            db=sqlite3.connect(seed)
            self.assertEqual(db.execute('SELECT count(*) FROM killmails').fetchone()[0],1)
            self.assertEqual(db.execute('SELECT count(*) FROM annual_rating_duels').fetchone()[0],1)
            self.assertEqual(db.execute('SELECT count(*) FROM annual_rating_days').fetchone()[0],days)
            db.close()
            manifest=json.loads((root/'seed-manifest.json').read_text())
            self.assertEqual(manifest['sha256'],hashlib.sha256(seed.read_bytes()).hexdigest())


if __name__=='__main__':unittest.main()
