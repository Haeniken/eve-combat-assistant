#!/usr/bin/env python3
"""Prepare the installer's public, compact 90-day killmail snapshot; resumable.

Only public fitting/selection fields are kept. Downloads and temporary data stay
under build/. The shipped app does not require Python.
"""
import argparse
import datetime as dt
import gc
import gzip
import hashlib
import json
import shutil
import sqlite3
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

AGENT = 'EVECombatAssistant/0.3.4 (https://github.com/Haeniken/eve-combat-assistant)'
BASE = 'https://r2z2.zkillboard.com/history'


def download(url, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    temporary = destination.with_suffix(destination.suffix + '.tmp')
    for attempt in range(3):
        try:
            request = urllib.request.Request(url, headers={'User-Agent': AGENT, 'Accept-Encoding': 'gzip'})
            with urllib.request.urlopen(request, timeout=120) as response:
                source = gzip.GzipFile(fileobj=response) if response.headers.get('Content-Encoding') == 'gzip' else response
                with temporary.open('wb') as output:
                    shutil.copyfileobj(source, output, 1024 * 256)
            temporary.replace(destination)
            return
        except (OSError, urllib.error.URLError) as error:
            temporary.unlink(missing_ok=True)
            if attempt == 2:
                raise
            print(json.dumps({'event': 'download_retry', 'url': url, 'attempt': attempt + 1, 'error': str(error)[:200]}), flush=True)
            time.sleep(2 ** (attempt + 1))


def compact(mail, allowed, categories):
    victim = mail.get('victim', {})
    ship = victim.get('ship_type_id', 0)
    if ship not in allowed:
        return None
    items = []
    for item in victim.get('items', []):
        flag = item.get('flag', 0)
        category = categories.get(item.get('item_type_id'))
        if (category in (7, 8, 32, 66) and (11 <= flag <= 34 or 92 <= flag <= 99 or 125 <= flag <= 132)) or (category == 18 and flag == 87):
            items.append({key: item[key] for key in ('item_type_id', 'flag', 'quantity_destroyed', 'quantity_dropped') if key in item})
    # The product's attacker slider is 1..5. Six distinct players are sufficient
    # to reject larger fleets without shipping names, positions, or attack damage.
    attackers = []
    ids = set()
    for attacker in mail.get('attackers', []):
        character = attacker.get('character_id', 0)
        if character and character not in ids:
            ids.add(character)
            if len(attackers) < 6:
                attackers.append({'character_id': character})
    record = {
        'killmail_id': mail['killmail_id'], 'killmail_time': mail['killmail_time'],
        'solar_system_id': mail.get('solar_system_id', 0),
        'victim': {key: victim.get(key, 0) for key in ('ship_type_id', 'character_id', 'corporation_id')},
        'attackers': attackers,
    }
    record['victim']['items'] = items
    if 'zkb' in mail:
        record['zkb'] = {key: mail['zkb'][key] for key in ('npc', 'awox') if key in mail['zkb']}
    return record


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--days', type=int, default=90)
    parser.add_argument('--end', default=(dt.datetime.now(dt.timezone.utc).date() - dt.timedelta(days=1)).isoformat())
    parser.add_argument('--prepare-executable', type=Path, help='Built native app: precompute default populations before publishing the manifest')
    parser.add_argument('--directory', type=Path, default=Path('build/bootstrap-v0.3.4'))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    catalog = json.loads(gzip.decompress((root / 'resources/catalog.json.gz').read_bytes()))['types']
    ships = {row['id']: row['name'] for row in catalog if row['category'] == 6}
    categories = {row['id']: row['category'] for row in catalog}
    upstream = json.loads((root / 'native/upstream.json').read_text())
    totals_path = directory / 'totals.json'
    if not totals_path.exists():
        download(BASE + '/totals.json', totals_path)
    totals = json.loads(totals_path.read_text())
    end = dt.date.fromisoformat(args.end)
    start = end - dt.timedelta(days=args.days - 1)
    db = sqlite3.connect(directory / 'seed.sqlite3')
    db.executescript('''PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-32768;
      CREATE TABLE IF NOT EXISTS killmails(id INTEGER PRIMARY KEY,ship INTEGER NOT NULL,pilot INTEGER,happened TEXT NOT NULL,system INTEGER,corp INTEGER,raw TEXT NOT NULL);
      CREATE INDEX IF NOT EXISTS km_ship_time ON killmails(ship,happened);
      CREATE INDEX IF NOT EXISTS km_pilot_ship ON killmails(pilot,ship,happened);
      CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT);
      CREATE TABLE IF NOT EXISTS days(day TEXT PRIMARY KEY,source_count INTEGER NOT NULL,ship_count INTEGER NOT NULL);
      CREATE TABLE IF NOT EXISTS hulls(id INTEGER PRIMARY KEY,name TEXT NOT NULL);
    ''')
    if 'index_total' not in {row[1] for row in db.execute('PRAGMA table_info(days)')}:
        db.execute('ALTER TABLE days ADD COLUMN index_total INTEGER')
    discrepancies = []
    populations_invalidated = False
    previous_period = dict(db.execute("SELECT key,value FROM meta WHERE key IN ('period_start','period_end')"))
    if previous_period and (json.loads(previous_period.get('period_start', 'null')) != start.isoformat() or json.loads(previous_period.get('period_end', 'null')) != end.isoformat()):
        raise RuntimeError('Use a new directory for another snapshot period')
    db.executemany('INSERT OR REPLACE INTO hulls VALUES(?,?)', ships.items())
    meta = {'schema_version': 1, 'sde_build': upstream['sde_build'], 'period_start': start.isoformat(), 'period_end': end.isoformat(), 'source': BASE + '/raw/{YYYYMMDD}.json', 'labels': 'Public ESI raw archive: zkb labels may be unavailable; local fleet/attacker filters apply.'}
    db.executemany('INSERT OR REPLACE INTO meta VALUES(?,?)', ((key, json.dumps(value)) for key, value in meta.items()))
    db.commit()
    for offset in range(args.days):
        day = start + dt.timedelta(days=offset)
        stamp = day.strftime('%Y%m%d')
        expected = int(totals.get(stamp, 0))
        previous = db.execute('SELECT coalesce(index_total,source_count) FROM days WHERE day=?', (stamp,)).fetchone()
        if previous and previous[0] == expected:
            continue
        if not populations_invalidated:
            db.execute('DROP TABLE IF EXISTS populations')
            db.commit()
            populations_invalidated = True
        path = directory / 'raw' / (stamp + '.json')
        if previous and previous[0] != expected:
            path.unlink(missing_ok=True)
            (directory / ('index-' + stamp + '.json')).unlink(missing_ok=True)
        if not expected:
            raise RuntimeError('Source has no complete daily archive for ' + stamp)
        if expected and not path.exists():
            download(BASE + '/raw/' + stamp + '.json', path)
            time.sleep(1.2)
        rows = json.loads(path.read_bytes()) if expected else []
        if len(rows) != expected:
            index_path = directory / ('index-' + stamp + '.json')
            if not index_path.exists():
                download(BASE + '/' + stamp + '.json', index_path)
                time.sleep(1.2)
            index = json.loads(index_path.read_bytes())
            raw_ids = set(rows) if isinstance(rows, dict) else {str(row['killmail_id']) for row in rows}
            if set(index) != raw_ids:
                path.unlink(missing_ok=True)
                index_path.unlink(missing_ok=True)
                raise RuntimeError('Raw archive does not match daily ID index for ' + stamp + '; rerun to retry')
            discrepancies.append({'day': stamp, 'published_total': expected, 'available_records': len(rows), 'daily_id_index_verified': True})
        values = rows.values() if isinstance(rows, dict) else rows
        batch = []
        for mail in values:
            record = compact(mail, ships, categories)
            if record is None:
                continue
            v = record['victim']
            batch.append((record['killmail_id'], v['ship_type_id'], v.get('character_id'), record['killmail_time'], record['solar_system_id'], v.get('corporation_id'), json.dumps(record, separators=(',', ':'))))
        with db:
            db.executemany('INSERT OR REPLACE INTO killmails VALUES(?,?,?,?,?,?,?)', batch)
            db.execute('INSERT OR REPLACE INTO days(day,source_count,ship_count,index_total) VALUES(?,?,?,?)', (stamp, len(rows), len(batch), expected))
        status = {'event': 'day_complete', 'day': stamp, 'position': offset + 1, 'days': args.days, 'source_records': len(rows), 'ship_records': len(batch)}
        (directory / 'progress.json').write_text(json.dumps(status, indent=2) + '\n')
        print(json.dumps(status), flush=True)
        del rows, values, batch
        gc.collect()
    actual_days = db.execute('SELECT count(*) FROM days WHERE day>=? AND day<=?', (start.strftime('%Y%m%d'), end.strftime('%Y%m%d'))).fetchone()[0]
    if actual_days != args.days:
        raise RuntimeError('Incomplete seed date coverage')
    for stamp, available, published in db.execute('SELECT day,source_count,index_total FROM days WHERE index_total != source_count'):
        if not any(row['day'] == stamp for row in discrepancies):
            discrepancies.append({'day': stamp, 'published_total': published, 'available_records': available, 'daily_id_index_verified': True})
    manifest = dict(meta, index_discrepancies=discrepancies, known_hulls=len(ships), hulls_with_records=db.execute('SELECT count(DISTINCT ship) FROM killmails').fetchone()[0], records=db.execute('SELECT count(*) FROM killmails').fetchone()[0], complete_days=actual_days, built_at=dt.datetime.now(dt.timezone.utc).isoformat())
    with db:
        db.executemany('INSERT OR REPLACE INTO meta VALUES(?,?)', ((key, json.dumps(value)) for key, value in manifest.items()))
    db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    db.execute('PRAGMA journal_mode=DELETE')
    db.close()
    if args.prepare_executable:
        prepared_db = sqlite3.connect(directory / 'seed.sqlite3')
        prepared_db.execute('DROP TABLE IF EXISTS populations')
        prepared_db.commit()
        prepared_db.close()
        subprocess.run([str(args.prepare_executable.resolve()), '--data-dir', str(directory / 'prepare-user'), '--prepare-seed', str(directory / 'seed.sqlite3')], check=True)
    prepared_db = sqlite3.connect(directory / 'seed.sqlite3')
    has_prepared = prepared_db.execute("SELECT count(*) FROM sqlite_schema WHERE name='populations'").fetchone()[0]
    manifest['prepared_populations'] = prepared_db.execute('SELECT count(*) FROM populations').fetchone()[0] if has_prepared else 0
    prepared_db.close()
    with (directory / 'seed.sqlite3').open('rb') as source:
        manifest['sha256'] = hashlib.file_digest(source, 'sha256').hexdigest()
    (directory / 'seed-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(json.dumps({'event': 'seed_complete', **manifest}), flush=True)


if __name__ == '__main__':
    main()
