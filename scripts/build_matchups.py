#!/usr/bin/env python3
"""Build resumable hull-vs-hull solo statistics from public daily archives.

One JSON record is decoded at a time. SQLite retains killmail IDs for deduplication;
the shipped daily aggregates need neither pilot names nor fitting data.
"""
import argparse
from collections import Counter
import datetime as dt
import gzip
import json
from pathlib import Path
import sqlite3
import time
from build_bootstrap import download, BASE

POLICY = 'hull-solo-v1'
MIN_BATTLES = 50
EXCLUDED_GROUPS = {29, 31, 237}  # capsules, shuttles, corvettes, on both sides


def records(path):
    """Iterate either {id: killmail} or [killmail], with bounded decoding memory."""
    decoder = json.JSONDecoder()
    with path.open(encoding='utf-8') as source:
        buffer, pos = '', 0

        def more():
            nonlocal buffer, pos
            tail = source.read(256 * 1024)
            buffer, pos = buffer[pos:] + tail, 0
            if len(buffer) > 16 * 1024 * 1024:
                raise ValueError('Individual archive record exceeds 16 MiB')
            return bool(tail)

        def peek():
            nonlocal pos
            while True:
                while pos < len(buffer) and buffer[pos].isspace():
                    pos += 1
                if pos < len(buffer):
                    return buffer[pos]
                if not more():
                    raise ValueError('Truncated archive')

        def value():
            nonlocal pos
            peek()
            while True:
                try:
                    result, end = decoder.raw_decode(buffer, pos)
                    pos = end
                    return result
                except json.JSONDecodeError:
                    if not more():
                        raise

        opening = peek()
        if opening not in '{[':
            raise ValueError('Expected archive object or array')
        pos += 1
        closing = '}' if opening == '{' else ']'
        while peek() != closing:
            if opening == '{':
                if not isinstance(value(), str) or peek() != ':':
                    raise ValueError('Invalid archive key')
                pos += 1
            row = value()
            if not isinstance(row, dict):
                raise ValueError('Expected a killmail object')
            yield row
            next_char = peek()
            if next_char == ',':
                pos += 1
            elif next_char != closing:
                raise ValueError('Expected comma or end of archive')
        pos += 1
        if buffer[pos:].strip() or source.read().strip():
            raise ValueError('Data after archive end')


def duel(mail, ships):
    victim = mail.get('victim') or {}
    loser = victim.get('ship_type_id', 0)
    if loser not in ships or ships[loser]['group'] in EXCLUDED_GROUPS:
        return None, 'excluded_victim_hull'
    victim_id = victim.get('character_id', 0)
    if not victim_id:
        return None, 'npc_victim'
    zkb = mail.get('zkb') or {}
    labels = zkb.get('labels') or []
    if zkb.get('npc') or zkb.get('awox') or any(x in labels for x in ['npc', 'awox', 'padding', 'ganked']):
        return None, 'excluded_labels'
    players = {}
    for attacker in mail.get('attackers', []):
        character = attacker.get('character_id', 0)
        if character:
            if character in players and players[character].get('ship_type_id') != attacker.get('ship_type_id'):
                return None, 'ambiguous_attacker_hull'
            players[character] = attacker
    if len(players) != 1:
        return None, 'not_one_player_attacker'
    attacker_id, attacker = next(iter(players.items()))
    winner = attacker.get('ship_type_id', 0)
    if attacker_id == victim_id:
        return None, 'self_kill'
    if winner not in ships or ships[winner]['group'] in EXCLUDED_GROUPS:
        return None, 'unknown_or_excluded_winner_hull'
    # Avoid recorded suicide ganks when raw ESI archives lack zKill labels.
    # A police final blow is observable; invisible support cannot be recovered.
    if any(a.get('final_blow') and not a.get('character_id') and a.get('corporation_id') == 1000125 for a in mail.get('attackers', [])):
        return None, 'concord_final_blow'
    return (winner, loser), 'eligible'


def quantile(values, p):
    position = (len(values) - 1) * p
    lo = int(position)
    hi = min(lo + 1, len(values) - 1)
    return values[lo] + (values[hi] - values[lo]) * (position - lo)


def install_seed(seed_path, directory):
    """Embed compact daily counts, never raw year archives or pilot IDs."""
    directory=Path(directory)
    report=json.loads((directory/'report.json').read_text())
    if report['days']!=365 or report.get('verified_days')!=365 or report['policy']!=POLICY:
        raise ValueError('Prepare the complete 365-day matchup reference first')
    root=Path(__file__).resolve().parents[1]
    calibration=json.loads((root/'resources/rating-tiers.json').read_text())['winrate']
    if (calibration['period_start'],calibration['period_end'])!=(report['period_start'],report['period_end']):
        raise ValueError('Recalibrate colors for the prepared year before packaging')
    db=sqlite3.connect(seed_path)
    db.execute('ATTACH DATABASE ? AS matchup_source',(str((directory/'matchups.sqlite3').resolve()),))
    with db:
        db.execute('CREATE TABLE IF NOT EXISTS hull_matchup_days(day TEXT PRIMARY KEY,source_count INTEGER NOT NULL,duels INTEGER NOT NULL,policy TEXT NOT NULL)')
        db.execute('CREATE TABLE IF NOT EXISTS hull_matchups_daily(day TEXT NOT NULL,low INTEGER NOT NULL,high INTEGER NOT NULL,low_wins INTEGER NOT NULL,high_wins INTEGER NOT NULL,PRIMARY KEY(day,low,high))')
        db.execute('DELETE FROM hull_matchup_days')
        db.execute('DELETE FROM hull_matchups_daily')
        db.execute('INSERT INTO hull_matchup_days SELECT * FROM matchup_source.hull_matchup_days WHERE day>=? AND day<=?',(report['period_start'].replace('-',''),report['period_end'].replace('-','')))
        db.execute('INSERT INTO hull_matchups_daily SELECT * FROM matchup_source.hull_matchups_daily WHERE day>=? AND day<=?',(report['period_start'].replace('-',''),report['period_end'].replace('-','')))
        db.execute('CREATE INDEX IF NOT EXISTS hull_matchup_pair ON hull_matchups_daily(low,high,day)')
        db.execute('INSERT OR REPLACE INTO meta VALUES(?,?)',('hull_matchup_calibration',json.dumps(calibration)))
    info={
        'period_start':report['period_start'],'period_end':report['period_end'],
        'days':report['days'],'source_records':report['source_records'],
        'eligible_killmails':report['eligible_killmails'],'unique_hull_pairs':report['unique_hull_pairs'],
        'minimum_battles':MIN_BATTLES,'policy':POLICY,
        'daily_rows':db.execute('SELECT count(*) FROM hull_matchups_daily').fetchone()[0],
    }
    db.close()
    return info


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--raw-directory', type=Path, required=True)
    parser.add_argument('--directory', type=Path, required=True)
    parser.add_argument('--end', required=True)
    parser.add_argument('--days', type=int, default=90)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    rows = json.loads(gzip.decompress((root / 'resources/catalog.json.gz').read_bytes()))['types']
    ships = {r['id']: r for r in rows if r['category'] == 6}
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    end = dt.date.fromisoformat(args.end)
    start = end - dt.timedelta(days=args.days-1)
    db = sqlite3.connect(directory / 'matchups.sqlite3')
    db.executescript('''PRAGMA journal_mode=WAL; PRAGMA cache_size=-8192;
      CREATE TABLE IF NOT EXISTS hull_duels(id INTEGER PRIMARY KEY,day TEXT NOT NULL,winner INTEGER NOT NULL,loser INTEGER NOT NULL);
      CREATE INDEX IF NOT EXISTS hull_duel_day ON hull_duels(day);
      CREATE TABLE IF NOT EXISTS hull_matchup_days(day TEXT PRIMARY KEY,source_count INTEGER NOT NULL,duels INTEGER NOT NULL,policy TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS hull_matchups_daily(day TEXT NOT NULL,low INTEGER NOT NULL,high INTEGER NOT NULL,low_wins INTEGER NOT NULL,high_wins INTEGER NOT NULL,PRIMARY KEY(day,low,high));
      CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
    ''')
    diagnostics = Counter()
    for offset in range(args.days):
        day = start + dt.timedelta(days=offset)
        stamp = day.strftime('%Y%m%d')
        previous = db.execute('SELECT policy FROM hull_matchup_days WHERE day=?',(stamp,)).fetchone()
        if previous and previous[0] == POLICY:
            continue
        path = args.raw_directory / (stamp + '.json')
        if not path.exists():
            path = directory / 'raw' / (stamp + '.json')
            if not path.exists():
                download(f'{BASE}/raw/{stamp}.json',path)
                time.sleep(1.2)
        count = 0
        day_duels = 0
        with db:
            db.execute('DELETE FROM hull_duels WHERE day=?',(stamp,))
            db.execute('DELETE FROM hull_matchups_daily WHERE day=?',(stamp,))
            for row in records(path):
                count += 1
                result, reason = duel(row, ships)
                diagnostics[reason] += 1
                if result is None:
                    continue
                if row.get('killmail_time','')[:10] != day.isoformat():
                    raise ValueError(f'Archive date mismatch: {stamp}')
                winner, loser = result
                inserted = db.execute('INSERT OR IGNORE INTO hull_duels VALUES(?,?,?,?)',(row['killmail_id'],stamp,winner,loser)).rowcount
                day_duels += inserted
                diagnostics['duplicate_id'] += 1 - inserted
            db.execute('''INSERT INTO hull_matchups_daily
              SELECT day,min(winner,loser),max(winner,loser),sum(winner<=loser),sum(winner>loser)
              FROM hull_duels WHERE day=? GROUP BY day,min(winner,loser),max(winner,loser)''',(stamp,))
            db.execute('INSERT OR REPLACE INTO hull_matchup_days VALUES(?,?,?,?)',(stamp,count,day_duels,POLICY))
        progress={'event':'matchup_day','day':stamp,'position':offset+1,'total_days':args.days,'source_records':count,'eligible':day_duels}
        (directory/'progress.json').write_text(json.dumps(progress)+'\n')
        print(json.dumps(progress),flush=True)
    pairs = db.execute('''SELECT low,high,sum(low_wins),sum(high_wins) FROM hull_matchups_daily
      WHERE day>=? AND day<=? GROUP BY low,high ORDER BY low,high''',(start.strftime('%Y%m%d'),end.strftime('%Y%m%d'))).fetchall()
    directed=[]
    for a,b,w,l in pairs:
        if a != b and w+l >= MIN_BATTLES:
            directed.extend([100*w/(w+l),100*l/(w+l)])
    directed.sort()
    deviations=sorted(abs(p-50) for p in directed)
    q={str(p):quantile(directed,p/100) for p in [5,10,12.5,16.6667,20,25,30,33.3333,40,50,60,66.6667,70,75,80,83.3333,87.5,90,95]}
    report={
      'policy':POLICY,'period_start':start.isoformat(),'period_end':end.isoformat(),
      'source':'https://r2z2.zkillboard.com/history/raw/{YYYYMMDD}.json',
      'days':db.execute('SELECT count(*) FROM hull_matchup_days WHERE day>=? AND day<=?',(start.strftime('%Y%m%d'),end.strftime('%Y%m%d'))).fetchone()[0],
      'source_records':db.execute('SELECT sum(source_count) FROM hull_matchup_days WHERE day>=? AND day<=?',(start.strftime('%Y%m%d'),end.strftime('%Y%m%d'))).fetchone()[0],
      'eligible_killmails':sum(w+l for a,b,w,l in pairs),'unique_hull_pairs':len(pairs),
      'minimum_battles':MIN_BATTLES,'calibration_directed_pairs':len(directed),
      'calibration_weight':'each eligible unordered hull pair contributes both directions equally; mirror pairs excluded',
      'winrate_quantiles_percent':q,
      'absolute_deviation_quantiles_pp':{str(p):quantile(deviations,p/100) for p in [25,50,75]},
      'sample_sizes':{str(n):sum(a!=b and w+l>=n for a,b,w,l in pairs) for n in [1,20,30,50,100,200]},
      'diagnostics_this_run':dict(diagnostics),
      'examples':[{ 'own':ships[a]['name'],'enemy':ships[b]['name'],'wins':w,'losses':l,'winrate':100*w/(w+l)} for a,b,w,l in pairs if {ships[a]['name'],ships[b]['name']}=={'Harpy','Stabber'}],
    }
    (directory/'report.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
    with db:
        db.execute('INSERT OR REPLACE INTO meta VALUES(?,?)',('report',json.dumps(report)))
    db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
    db.close()
    print(json.dumps({'event':'matchup_complete',**report}),flush=True)


if __name__ == '__main__':
    main()
