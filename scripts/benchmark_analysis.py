#!/usr/bin/env python3
"""Measure the overlay's local analysis using an executable and isolated profiles."""
import argparse
import json
import pathlib
import shutil
import statistics
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--executable', type=pathlib.Path, required=True)
    parser.add_argument('--seed', type=pathlib.Path, required=True)
    parser.add_argument('--fit', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--runs', type=int, default=3)
    parser.add_argument(
        '--ships', nargs='+',
        default=['Rifter', 'Gnosis', 'Praxis', 'Megathron', 'Nighthawk', 'Vedmak', 'Marshal', 'Loki'],
    )
    args = parser.parse_args()
    if not 1 <= args.runs <= 20:
        parser.error('--runs must be between 1 and 20')
    executable, seed, fit = (p.resolve(strict=True) for p in (args.executable, args.seed, args.fit))
    rows = []
    with tempfile.TemporaryDirectory(prefix='eve-analysis-benchmark-') as temporary:
        for ship in args.ships:
            for run in range(args.runs):
                profile = pathlib.Path(temporary) / str(len(rows))
                command = [
                    str(executable), '--fit-diagnostic', '--ship', ship,
                    '--fit', str(fit), '--data-dir', str(profile), '--seed-database', str(seed),
                ]
                memory = pathlib.Path(temporary) / 'rss.txt'
                if pathlib.Path('/usr/bin/time').is_file():
                    command = ['/usr/bin/time', '-f', '%M', '-o', str(memory), *command]
                started = time.perf_counter()
                subprocess.run(command, check=True, capture_output=True, text=True)
                result = json.loads((profile / 'fit-diagnostic.json').read_text(encoding='utf-8'))
                result.update(run=run + 1, wall_ms=round((time.perf_counter() - started) * 1000, 2))
                result['peak_rss_kib'] = int(memory.read_text().strip()) if memory.exists() else None
                rows.append(result)
                # Release each profile before the next case; the seed is attached read-only.
                shutil.rmtree(profile)
    summaries = []
    for ship in args.ships:
        cases = [row for row in rows if row['ship'] == ship]
        summary = {
            'ship': ship,
            'median_full_ms': statistics.median(row['elapsed_ms'] for row in cases),
            'median_first_ms': statistics.median(
                row.get('first_result_ms', row['elapsed_ms']) for row in cases
            ),
        }
        summaries.append(summary)
        print(json.dumps(summary, ensure_ascii=False), flush=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({'summary': summaries, 'runs': rows}, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')


if __name__ == '__main__':
    main()
