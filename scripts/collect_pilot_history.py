#!/usr/bin/env python3
"""Collect public daily killmails into a resumable pilot-vs-pilot research dataset.

Raw archives stay compressed on the research host. Nothing here runs in the
Windows application. Two workers share a single request rate limiter.
"""
import argparse
from collections import Counter
import concurrent.futures
import datetime as dt
import gzip
import hashlib
import json
from pathlib import Path
import shutil
import sqlite3
import threading
import time
import urllib.request

from build_matchups import duel, records

BASE = 'https://r2z2.zkillboard.com/history'
AGENT = 'EVECombatAssistant rating research (https://github.com/Haeniken/eve-combat-assistant)'
POLICY = 'pilot-evidence-v3'


class Collector:
    def __init__(self, directory, catalog, totals):
        self.directory = directory
        self.ships = {int(k): v for k, v in catalog['ships'].items()}
        self.weapons = set(catalog['combat_items'])
        self.totals = totals
        self.lock = threading.Lock()
        self.next_request = 0.

    def connection(self):
        db = sqlite3.connect(self.directory / 'history.sqlite3', timeout=120)
        db.execute('PRAGMA cache_size=-16384')
        return db

    def download(self, url, path, compressed=False):
        if path.is_file():
            return
        if shutil.disk_usage(self.directory).free < 25 * 2**30:
            raise RuntimeError('Less than 25 GiB free: collection stopped before filling disk')
        path.parent.mkdir(parents=True, exist_ok=True)
        temporary = path.with_suffix(path.suffix + '.part')
        for attempt in range(4):
            try:
                with self.lock:
                    time.sleep(max(0., self.next_request - time.monotonic()))
                    self.next_request = time.monotonic() + 1.2
                request = urllib.request.Request(url, headers={
                    'User-Agent': AGENT, 'Accept-Encoding': 'gzip',
                })
                with urllib.request.urlopen(request, timeout=120) as response:
                    prefix = response.read(2)
                    incoming_gzip = prefix == b'\x1f\x8b'
                    if compressed:
                        with temporary.open('wb') as output:
                            if incoming_gzip:
                                output.write(prefix)
                                shutil.copyfileobj(response, output, 256 * 1024)
                            else:
                                with gzip.GzipFile(fileobj=output, mode='wb', compresslevel=3) as zipped:
                                    zipped.write(prefix)
                                    shutil.copyfileobj(response, zipped, 256 * 1024)
                    else:
                        # Small indexes/totals only. Raw archives use streaming above.
                        body = prefix + response.read(8 * 1024 * 1024 + 1)
                        if len(body) > 8 * 1024 * 1024:
                            raise ValueError('Daily index exceeds 8 MiB')
                        temporary.write_bytes(gzip.decompress(body) if incoming_gzip else body)
                temporary.replace(path)
                return
            except (OSError, ValueError) as error:
                temporary.unlink(missing_ok=True)
                if attempt == 3:
                    raise
                print(json.dumps({'event': 'retry', 'file': path.name, 'attempt': attempt + 1,
                                  'error': str(error)[:180]}), flush=True)
                time.sleep(2 ** (attempt + 1))

    def evidence(self, mail, day):
        hulls, reason = duel(mail, self.ships)
        if hulls is None:
            return None, reason
        stamp = mail['killmail_time']
        if stamp[:10].replace('-', '') != day:
            raise ValueError('Archive date mismatch')
        victim = mail['victim']
        players = [a for a in mail.get('attackers', []) if a.get('character_id')]
        winner = players[0]['character_id']
        player_damage = sum(max(0, a.get('damage_done', 0)) for a in players)
        npc_damage = sum(max(0, a.get('damage_done', 0)) for a in mail.get('attackers', [])
                         if not a.get('character_id'))
        items = victim.get('items')
        armed = -1 if items is None else int(any(
            item.get('item_type_id') in self.weapons
            and (27 <= item.get('flag', 0) <= 34 or item.get('flag') == 87)
            for item in items
        ))
        at = int(dt.datetime.fromisoformat(stamp.replace('Z', '+00:00')).timestamp())
        return (mail['killmail_id'], day, at, winner, victim['character_id'], *hulls,
                mail.get('solar_system_id', 0), players[0].get('corporation_id', 0),
                victim.get('corporation_id', 0), armed, player_damage, npc_damage,
                int(bool(mail.get('zkb')))), 'eligible'

    def day(self, day):
        db = self.connection()
        try:
            if db.execute('SELECT 1 FROM days WHERE day=? AND policy=?', (day, POLICY)).fetchone():
                return {'event': 'day_cached', 'day': day}
            archive = self.directory / 'raw' / (day + '.json.gz')
            self.download(f'{BASE}/raw/{day}.json', archive, compressed=True)
            ids = set()
            diagnostics = Counter()
            expected = int(self.totals[day])
            # Retain compact outcomes for one day; network I/O never holds a DB write lock.
            outcomes = []
            for mail in records(archive):
                identity = str(mail['killmail_id'])
                if identity in ids:
                    diagnostics['duplicate_id'] += 1
                    continue
                ids.add(identity)
                row, reason = self.evidence(mail, day)
                diagnostics[reason] += 1
                if row:
                    outcomes.append(row)
            index_verified = False
            if len(ids) != expected:
                index_path = self.directory / 'indexes' / (day + '.json')
                self.download(f'{BASE}/{day}.json', index_path)
                index = json.loads(index_path.read_bytes())
                if not index:
                    raise ValueError('Empty daily ID index')
                missing = sorted(set(index) - ids)
                if len(missing) > 100:
                    raise ValueError(f'{len(missing)} indexed records missing: requires archive review')
                for identity in missing:
                    hashed = index[identity]
                    if not (identity.isdecimal() and isinstance(hashed, str) and len(hashed) == 40
                            and all(c in '0123456789abcdefABCDEF' for c in hashed)):
                        raise ValueError('Invalid public ESI killmail identity/hash')
                    path = self.directory / 'individual' / (identity + '.json')
                    self.download(f'https://esi.evetech.net/killmails/{identity}/{hashed}/', path)
                    mail = json.loads(path.read_bytes())
                    if str(mail.get('killmail_id')) != identity:
                        raise ValueError('Individual killmail identity mismatch')
                    row, reason = self.evidence(mail, day)
                    diagnostics[reason] += 1
                    if row:
                        outcomes.append(row)
                    ids.add(identity)
                index_verified = set(index).issubset(ids)
                if not index_verified:
                    raise ValueError('Daily ID index not covered')
            with archive.open('rb') as stream:
                sha = hashlib.file_digest(stream, 'sha256').hexdigest()
            with db:
                db.execute('DELETE FROM duels WHERE day=?', (day,))
                db.executemany('INSERT OR IGNORE INTO duels VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)', outcomes)
                eligible = db.execute('SELECT count(*) FROM duels WHERE day=?', (day,)).fetchone()[0]
                db.execute('INSERT OR REPLACE INTO days VALUES(?,?,?,?,?,?,?,?)',
                           (day, len(ids), expected, eligible, int(index_verified), sha,
                            POLICY, json.dumps(dict(diagnostics), sort_keys=True)))
            return {'event': 'day_ready', 'day': day, 'source_records': len(ids),
                    'eligible': eligible, 'index_verified': index_verified,
                    'compressed_bytes': archive.stat().st_size}
        finally:
            db.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', required=True, type=Path)
    parser.add_argument('--catalog', required=True, type=Path)
    parser.add_argument('--start', required=True)
    parser.add_argument('--end', required=True)
    parser.add_argument('--workers', default=2, type=int, choices=(1, 2))
    args = parser.parse_args()
    args.directory.mkdir(parents=True, exist_ok=True)
    start, end = dt.date.fromisoformat(args.start), dt.date.fromisoformat(args.end)
    if start > end:
        parser.error('start is after end')
    totals = json.loads((args.directory / 'totals.json').read_bytes())
    days = [(end - dt.timedelta(days=i)).strftime('%Y%m%d') for i in range((end-start).days+1)]
    if any(day not in totals for day in days):
        raise ValueError('Daily source totals do not cover requested period')
    collector = Collector(args.directory, json.loads(args.catalog.read_bytes()), totals)
    db = collector.connection()
    db.executescript('''PRAGMA journal_mode=WAL;
      CREATE TABLE IF NOT EXISTS duels(
        id INTEGER PRIMARY KEY,day TEXT NOT NULL,at INTEGER NOT NULL,
        winner INTEGER NOT NULL,loser INTEGER NOT NULL,
        winner_hull INTEGER NOT NULL,loser_hull INTEGER NOT NULL,system INTEGER NOT NULL,
        winner_corp INTEGER NOT NULL,loser_corp INTEGER NOT NULL,
        victim_armed INTEGER NOT NULL,player_damage INTEGER NOT NULL,npc_damage INTEGER NOT NULL,
        labels_present INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS duel_day ON duels(day);
      CREATE TABLE IF NOT EXISTS days(day TEXT PRIMARY KEY,actual INTEGER NOT NULL,
        published INTEGER NOT NULL,duels INTEGER NOT NULL,index_verified INTEGER NOT NULL,
        archive_sha256 TEXT NOT NULL,policy TEXT NOT NULL,diagnostics TEXT NOT NULL);
    ''')
    ready = {r[0] for r in db.execute('SELECT day FROM days WHERE policy=?', (POLICY,))}
    db.close()
    pending = [day for day in days if day not in ready]
    progress = {'status': 'collecting', 'start': args.start, 'end': args.end,
                'requested_days': len(days), 'completed_days': len(set(days) & ready), 'failures': []}
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.workers) as pool:
        futures = {pool.submit(collector.day, day): day for day in pending}
        for future in concurrent.futures.as_completed(futures):
            try:
                event = future.result()
                progress['completed_days'] += 1
                progress['last_day'] = event
            except Exception as error:
                event = {'event': 'day_failed', 'day': futures[future], 'error': str(error)[:300]}
                progress['failures'].append(event)
            (args.directory / 'progress.json').write_text(json.dumps(progress, indent=2) + '\n')
            print(json.dumps(event), flush=True)
    progress['status'] = 'complete' if progress['completed_days'] == len(days) else 'incomplete'
    (args.directory / 'progress.json').write_text(json.dumps(progress, indent=2) + '\n')
    print(json.dumps(progress), flush=True)
    if progress['status'] != 'complete':
        raise SystemExit(1)


if __name__ == '__main__':
    main()
