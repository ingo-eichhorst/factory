#!/usr/bin/env python3
"""Attach explicit interpretation limits to raw reviewer labels; never merge them."""
import argparse
import csv
import io
from pathlib import Path
import aggregate

ROOT = Path(__file__).resolve().parent
OWNER = 'Generated exclusively by docs/residuality/a6/reviewed_index.py.\n'


def load(path):
    with path.open(encoding='utf-8', newline='') as f:
        rows = list(csv.DictReader(f, delimiter=';'))
    if any(None in r or any(not v for v in r.values()) for r in rows):
        raise ValueError('Malformed interpretation table: ' + str(path))
    return rows


def render():
    tables, _, _ = aggregate.model()
    states = {s['state_id']: s for s in tables['states.csv']}
    trajectories = {t['trajectory_id']: t for t in tables['trajectories.csv']}
    findings = load(aggregate.PROJECT / 'reviews/skeptic/cross-findings.csv')
    alignments = load(aggregate.PROJECT / 'reviews/skeptic/alignment.csv')
    qualifications = load(ROOT / 'state-qualifications.csv')
    branches = load(ROOT / 'supplemental-branches.csv')
    for rows in (findings, alignments, qualifications, branches):
        for row in rows:
            if not set(row['source_states'].split()) <= states.keys():
                raise ValueError('Unknown reviewed state: ' + str(row))
    for b in branches:
        if b['stressor'] not in trajectories[b['source_trajectory']]['stressors'].split():
            raise ValueError('Supplemental branch has wrong scenario/trajectory')
    rows = []
    for sid, state in states.items():
        ids = lambda data, field: ' '.join(r[field] for r in data if sid in r['source_states'].split()) or 'none'
        rows.append({'reviewer': state['reviewer'], 'state_id': sid, 'author_name': state['name'],
                     'author_kind': state['kind'], 'findings': ids(findings, 'id'),
                     'qualifications': ids(qualifications, 'id'), 'supplemental_branches': ids(branches, 'branch_id'),
                     'alignment_proposals': ids(alignments, 'alignment_id'),
                     'status': 'conditional-author-description-with-review-limits'})
    out = io.StringIO(newline='')
    writer = csv.DictWriter(out, list(rows[0]), lineterminator='\n')
    writer.writeheader()
    writer.writerows(rows)
    return {'OWNERSHIP.txt': OWNER, 'reviewed-state-index.csv': out.getvalue()}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--check', action='store_true')
    args = p.parse_args()
    files, out = render(), ROOT / 'reviewed'
    if args.check:
        stale = [n for n,t in files.items() if not (out/n).exists() or (out/n).read_text()!=t]
        if stale:
            raise SystemExit('Missing/stale interpretation index: ' + ', '.join(stale))
    else:
        if out.exists() and (not (out/'OWNERSHIP.txt').exists() or (out/'OWNERSHIP.txt').read_text()!=OWNER):
            raise SystemExit('Refusing to overwrite unowned interpretation output')
        out.mkdir(exist_ok=True)
        for n,t in files.items():
            (out/n).write_text(t, encoding='utf-8')
    print('Reviewer state IDs linked to findings, qualifications and branches without merging')


if __name__ == '__main__':
    main()
