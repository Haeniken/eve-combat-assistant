#!/usr/bin/env python3
"""Build the research hull/weapon catalog from the app's pinned SDE catalog."""
import argparse
import gzip
import json
from pathlib import Path


def offensive(row):
    attrs = row.get('attributes', {})
    damage = sum(attrs.get(k, 0) for k in ['emDamage', 'thermalDamage', 'kineticDamage', 'explosiveDamage'])
    if row['category'] == 18:
        return damage > 0
    if row['category'] != 7:
        return False
    name = row['name'].lower()
    return (row['group'] in [53, 55, 74, 72, 1986]
            or 'disintegrator' in name or 'vorton' in name
            or damage > 0
            or ('launcher' in name and any(k in name for k in ['missile', 'rocket', 'torpedo', 'heavy assault'])
                and 'defender' not in name))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    rows = json.loads(gzip.decompress((root / 'resources/catalog.json.gz').read_bytes()))['types']
    catalog = {'ships': {r['id']: {k: r[k] for k in ['id', 'name', 'group']}
                         for r in rows if r['category'] == 6},
               'combat_items': sorted(r['id'] for r in rows if offensive(r))}
    args.output.write_text(json.dumps(catalog, separators=(',', ':')) + '\n')


if __name__ == '__main__':
    main()
