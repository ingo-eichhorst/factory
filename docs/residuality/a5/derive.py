#!/usr/bin/env python3
"""Trace proposed structures AFTER the independent toy-dynamics stage.

The regime/candidate interpretation is human-authored, not discovered by this
join. It does not feed back into the dynamics or set their number of end sets.
"""
import argparse
import csv
from pathlib import Path
import dynamics

ROOT = Path(__file__).resolve().parent
OWNER = 'Generated exclusively by docs/residuality/a5/derive.py.\n'


def table(name):
    with (ROOT / name).open(encoding='utf-8', newline='') as f:
        rows = list(csv.DictReader(f, delimiter=';'))
    if not rows or any(None in r or any(not v or not v.strip() for v in r.values()) for r in rows):
        raise ValueError('Malformed or empty table: ' + name)
    if len({r['id'] for r in rows}) != len(rows):
        raise ValueError('Duplicate IDs: ' + name)
    return {r['id']: r for r in rows}


def model():
    cases = {c['id']: c for c in dynamics.cases()}
    regimes, candidates = table('regimes.csv'), table('candidates.csv')
    kinds = {g['kind'] for c in cases.values() for g in dynamics.analyze(c)}
    for r in regimes.values():
        if not set(r['cases'].split()) <= cases.keys() or r['kind'] not in kinds:
            raise ValueError('Unknown case or kind: ' + r['id'])
        for key in r['cases'].split():
            if r['kind'] not in {g['kind'] for g in dynamics.analyze(cases[key])}:
                raise ValueError('Regime classification not present in linked case: ' + r['id'])
    for c in candidates.values():
        if not set(c['regimes'].split()) <= regimes.keys():
            raise ValueError('Unknown regime: ' + c['id'])
        if c['status'] not in {'model-motivated-proposal', 'unproven-prerequisite'}:
            raise ValueError('No observed-residue claim is established: ' + c['id'])
    return cases, regimes, candidates


def render():
    cases, regimes, candidates = model()
    rows = []
    for candidate in candidates.values():
        for rid in candidate['regimes'].split():
            regime = regimes[rid]
            for cid in regime['cases'].split():
                for sid in cases[cid]['stressors']:
                    rows.append([sid, cid, rid, regime['kind'], candidate['id'], candidate['status'],
                                 'conditional-mechanism-link-not-full-scenario-simulation'])
    report = ['<!-- ' + OWNER.strip() + ' -->', '# Provisional structures derived after state analysis', '',
              '**These are design candidates, not empirically established Factory residues.**',
              'A linked case isolates a mechanism and can enter several regimes from different initial states.',
              'The interpretation below is authored; the dynamics and basin search do not read these labels.', '',
              '| Candidate | Regimes | Structure / retained function | Condition | Architecture consequence |',
              '|---|---|---|---|---|']
    for c in candidates.values():
        report.append('| ' + ' | '.join([c['id'], c['regimes'], c['candidate_structure'] + ': ' +
                      c['retained_function'], c['required_conditions'], c['architectural_consequence']]) + ' |')
    report += ['', f'{len(regimes)} interpreted regimes; {len(candidates)} provisional structures for the investigated mechanisms.',
               'This is NOT a new global answer replacing 17. The unmodelled scenarios can split, merge or invalidate this set.',
               'An irreversible damage fact is not a surviving safety capability. A perfect oracle is an unproven prerequisite.', '']
    return {'OWNERSHIP.txt': OWNER,
            'traceability.csv': dynamics.csv_text(['stressor', 'case', 'regime', 'regime_kind', 'candidate',
                                                  'candidate_status', 'evidence_limit'], rows),
            'candidates.md': '\n'.join(report)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    out, files = ROOT / 'derived', render()
    if args.check:
        stale = [n for n, t in files.items() if not (out / n).exists() or (out / n).read_text(encoding='utf-8') != t]
        if stale:
            raise SystemExit('Missing/stale: ' + ', '.join(stale))
    else:
        if out.exists() and (not (out / 'OWNERSHIP.txt').exists() or
                             (out / 'OWNERSHIP.txt').read_text(encoding='utf-8') != OWNER):
            raise SystemExit('No matching ownership marker')
        out.mkdir(exist_ok=True)
        for name, text in files.items():
            (out / name).write_text(text, encoding='utf-8')
    print(f'{len(files)} downstream candidate files ' + ('checked' if args.check else 'written'))


if __name__ == '__main__':
    main()
