#!/usr/bin/env python3
"""Prepare neutral review inputs before reviewer sessions start. No runtime calls."""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PROJECT = ROOT.parents[2]
RANGES = {'operations': (1, 100), 'governance': (101, 200),
          'observability': (201, 275), 'boundaries': (276, 350)}
OWNER = 'Only docs/residuality/a6/prepare_inputs.py owns scenarios.csv in this directory.\n'


def prepared():
    scenarios = {}
    for path in (ROOT.parent / 'stressors.csv', ROOT.parent / 'a4/stressors.csv'):
        with path.open(encoding='utf-8', newline='') as f:
            for row in csv.DictReader(f, delimiter=';'):
                if row['id'] in scenarios:
                    raise ValueError('Duplicate source ID')
                scenarios[row['id']] = row['stressor']
    if set(scenarios) != {f'S{i:03}' for i in range(1, 351)}:
        raise ValueError('Expected original S001-S350 text corpus')
    outputs, manifest = {}, {}
    for role, (start, end) in RANGES.items():
        stream = io.StringIO(newline='')
        writer = csv.writer(stream, delimiter=';', lineterminator='\n')
        writer.writerow(['stressor_id', 'scenario'])
        writer.writerows((f'S{i:03}', scenarios[f'S{i:03}']) for i in range(start, end + 1))
        text = stream.getvalue()
        outputs[role] = text
        manifest[role] = {'count': end - start + 1, 'range': f'S{start:03}-S{end:03}',
                          'sha256': hashlib.sha256(text.encode()).hexdigest()}
    return outputs, json.dumps(manifest, indent=2, sort_keys=True) + '\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    outputs, manifest = prepared()
    for role, text in outputs.items():
        target = PROJECT / 'reviews' / role
        marker = target / 'SCENARIOS-OWNERSHIP.txt'
        path = target / 'scenarios.csv'
        if args.check:
            if path.read_text(encoding='utf-8') != text or marker.read_text(encoding='utf-8') != OWNER:
                raise SystemExit('Changed neutral input: ' + role)
        else:
            if path.exists() and (not marker.exists() or marker.read_text(encoding='utf-8') != OWNER):
                raise SystemExit('Refusing to overwrite unowned scenario input')
            target.mkdir(parents=True, exist_ok=True)
            marker.write_text(OWNER, encoding='utf-8')
            path.write_text(text, encoding='utf-8')
    path = ROOT / 'input-manifest.json'
    if args.check:
        if path.read_text(encoding='utf-8') != manifest:
            raise SystemExit('Changed input manifest')
    else:
        if path.exists() and path.read_text(encoding='utf-8') != manifest:
            raise SystemExit('Input snapshot already exists with different content; start a new review round')
        path.write_text(manifest, encoding='utf-8')
    print('Four disjoint neutral inputs covering all 350 scenarios ' + ('checked' if args.check else 'written'))


if __name__ == '__main__':
    main()
