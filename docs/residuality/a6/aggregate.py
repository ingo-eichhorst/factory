#!/usr/bin/env python3
"""Validate and join independent reviewer artifacts without semantic merging.

This validates reference integrity, not the truth of any attractor claim.
"""
import argparse
import csv
import hashlib
import io
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent
PROJECT = ROOT.parents[2]
OWNER = 'Generated exclusively by docs/residuality/a6/aggregate.py.\n'
SCHEMAS = {
    'states.csv': 'state_id name kind conditions feedback entry exit survives lost falsifier evidence source_refs'.split(),
    'trajectories.csv': 'trajectory_id stressors initial_state sequence destination_states conditions evidence falsifier'.split(),
    'coverage.csv': 'stressor_id state_ids analysis_status reason'.split(),
}


def load(path, headers):
    with path.open(encoding='utf-8', newline='') as f:
        reader = csv.DictReader(f, delimiter=';')
        if reader.fieldnames != headers:
            raise ValueError(f'Wrong headers in {path}: {reader.fieldnames}')
        rows = list(reader)
    for row in rows:
        if None in row or any(v is None or not v.strip() for v in row.values()):
            raise ValueError('Malformed/empty field in ' + str(path))
    key = headers[0]
    if len({r[key] for r in rows}) != len(rows):
        raise ValueError('Duplicate primary IDs in ' + str(path))
    return rows


def model():
    manifest = json.loads((ROOT / 'review-manifest.json').read_text())
    input_hashes = json.loads((ROOT / 'input-manifest.json').read_text())
    roles = [r for r in manifest['reviewers'] if r['role'] != 'skeptic']
    combined = {name: [] for name in SCHEMAS}
    snapshot, counts, seen = {}, {}, set()
    for role in roles:
        folder = PROJECT / 'reviews' / role['role']
        neutral = folder / 'scenarios.csv'
        if hashlib.sha256(neutral.read_bytes()).hexdigest() != input_hashes[role['role']]['sha256']:
            raise ValueError('Neutral input changed: ' + role['role'])
        inputs = load(neutral, ['stressor_id', 'scenario'])
        expected = {r['stressor_id'] for r in inputs}
        if seen & expected:
            raise ValueError('Overlapping reviewer input ranges')
        seen |= expected
        tables = {name: load(folder / name, headers) for name, headers in SCHEMAS.items()}
        states = {r['state_id']: r for r in tables['states.csv']}
        if any(not key.startswith(role['prefix']) for key in states):
            raise ValueError('State ID lacks reviewer prefix')
        if {r['stressor_id'] for r in tables['coverage.csv']} != expected:
            raise ValueError('Missing or extra coverage rows: ' + role['role'])
        for row in tables['coverage.csv']:
            references = set(row['state_ids'].split()) - {'none'}
            if not references <= states.keys():
                raise ValueError('Unknown coverage state: ' + row['stressor_id'])
            if row['analysis_status'] not in {'conditional', 'source-supported', 'toy-supported', 'unresolved'}:
                raise ValueError('Unknown coverage status: ' + row['stressor_id'])
            if not references and row['analysis_status'] != 'unresolved':
                raise ValueError('An unassigned scenario must be explicitly unresolved')
        for row in tables['trajectories.csv']:
            if not set(row['stressors'].split()) <= expected:
                raise ValueError('Trajectory references outside assigned range')
            if not (set(row['destination_states'].split()) - {'none'}) <= states.keys():
                raise ValueError('Unknown trajectory destination')
        for name, rows in tables.items():
            combined[name] += [dict(reviewer=role['role'], **row) for row in rows]
        counts[role['role']] = {'scenarios': len(inputs), 'states': len(states),
            'trajectories': len(tables['trajectories.csv']),
            'status': dict(Counter(r['analysis_status'] for r in tables['coverage.csv']))}
        for name in [*SCHEMAS, 'report.md']:
            path = folder / name
            snapshot[str(path.relative_to(PROJECT))] = hashlib.sha256(path.read_bytes()).hexdigest()
    if seen != {f'S{i:03}' for i in range(1, 351)}:
        raise ValueError('The input review partition does not cover S001-S350')
    ids = [r['state_id'] for r in combined['states.csv']]
    if len(set(ids)) != len(ids):
        raise ValueError('Reviewer state namespaces overlap')
    return combined, counts, snapshot


def csv_text(headers, rows):
    out = io.StringIO(newline='')
    writer = csv.DictWriter(out, headers, lineterminator='\n')
    writer.writeheader()
    writer.writerows(rows)
    return out.getvalue()


def render():
    tables, counts, snapshot = model()
    files = {name: csv_text(['reviewer', *SCHEMAS[name]], rows) for name, rows in tables.items()}
    files['OWNERSHIP.txt'] = OWNER
    files['source-hashes.json'] = json.dumps(snapshot, sort_keys=True, indent=2) + '\n'
    lines = ['<!-- ' + OWNER.strip() + ' -->', '# Independent review inventory', '',
             '**Author-proposed state labels are retained without silently merging them.**',
             'Counts do not mean distinct real attractors, independent mechanisms or proven residues.', '',
             '| Reviewer | Scenario dispositions | State labels | Trajectories | Coverage labels |',
             '|---|---:|---:|---:|---|']
    for role, count in counts.items():
        lines.append(f'| {role} | {count["scenarios"]} | {count["states"]} | {count["trajectories"]} | '
                     + ', '.join(f'{k}: {v}' for k, v in sorted(count['status'].items())) + ' |')
    lines += ['', '## Author-assigned state classifications', '']
    for kind, count in sorted(Counter(r['kind'] for r in tables['states.csv']).items()):
        lines.append(f'- {kind}: {count}')
    lines += ['', 'Each row preserves its author, conditions, evidence, exits and falsifier.',
              'Coverage means an explicit disposition. It is not proof that a scenario is modelled, solved or an attractor.',
              'See the separate skeptical audit and synthesis for disputed interpretations.', '']
    files['summary.md'] = '\n'.join(lines)
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    files = render()
    out = ROOT / 'generated'
    if args.check:
        stale = [name for name, text in files.items() if not (out / name).exists() or
                 (out / name).read_text(encoding='utf-8') != text]
        if stale:
            raise SystemExit('Missing/stale review joins: ' + ', '.join(stale))
    else:
        if out.exists() and (not (out / 'OWNERSHIP.txt').exists() or
                             (out / 'OWNERSHIP.txt').read_text(encoding='utf-8') != OWNER):
            raise SystemExit('Refusing to overwrite unowned review output')
        out.mkdir(exist_ok=True)
        for name, text in files.items():
            (out / name).write_text(text, encoding='utf-8')
    print(f'{len(files)} independent review files ' + ('checked' if args.check else 'written'))


if __name__ == '__main__':
    main()
