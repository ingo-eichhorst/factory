"""Mechanical artifact checks only; no source execution, live faults or dynamics model."""
import csv
import hashlib
import json
import re
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent
HEADERS = {
    'scenarios.csv': 'stressor_id;scenario',
    'states.csv': 'state_id;name;kind;conditions;feedback;entry;exit;survives;lost;falsifier;evidence;source_refs',
    'trajectories.csv': 'trajectory_id;stressors;initial_state;sequence;destination_states;conditions;evidence;falsifier',
    'coverage.csv': 'stressor_id;state_ids;analysis_status;reason',
}


def load(name):
    with (ROOT / name).open(encoding='utf-8', newline='') as src:
        reader = csv.DictReader(src, delimiter=';')
        assert reader.fieldnames == HEADERS[name].split(';'), (name, reader.fieldnames)
        rows = list(reader)
    assert all(None not in row and all(value and value.strip() for value in row.values()) for row in rows), name
    return rows


scenarios = load('scenarios.csv')
states = load('states.csv')
trajectories = load('trajectories.csv')
coverage = load('coverage.csv')
expected = {f'S{i}' for i in range(276, 351)}
assert {r['stressor_id'] for r in scenarios} == expected and len(scenarios) == 75
assert {r['stressor_id'] for r in coverage} == expected and len(coverage) == 75
state_ids = {r['state_id'] for r in states}
assert len(state_ids) == len(states)
assert all(re.fullmatch(r'BD\d{2,}', state_id) for state_id in state_ids)
trajectory_ids = {r['trajectory_id'] for r in trajectories}
assert len(trajectory_ids) == len(trajectories)
assert all(re.fullmatch(r'BDT\d{3,}', tid) for tid in trajectory_ids)
assert trajectory_ids == {f'BDT{i:03}' for i in range(1, 76)}
by_stressor = {}
for row in trajectories:
    stressors = row['stressors'].split()
    dest = set(row['destination_states'].split())
    assert dest == {'none'} or dest <= state_ids, row
    for sid in stressors:
        assert sid in expected
        by_stressor.setdefault(sid, set()).update(dest)
assert set(by_stressor) == expected
used_states = set()
for row in coverage:
    sid = row['stressor_id']
    status = row['analysis_status']
    assert status in {'conditional', 'source-supported', 'toy-supported', 'unresolved'}
    dest = set(row['state_ids'].split())
    assert dest == by_stressor[sid], sid
    if status == 'unresolved':
        assert dest == {'none'}, sid
    else:
        assert dest and dest <= state_ids, sid
        used_states.update(dest)
assert used_states == state_ids, state_ids - used_states
assert (ROOT / 'report.md').is_file()
report = (ROOT / 'report.md').read_text(encoding='utf-8')
assert 'No actual Factory attractor was empirically established.' in report
assert 'No residues are proposed.' in report
for name in ['states.csv', 'trajectories.csv', 'coverage.csv', 'report.md']:
    inline_refs = set(re.findall(r'\bBD\d+\b', (ROOT / name).read_text(encoding='utf-8')))
    assert inline_refs <= state_ids, (name, inline_refs - state_ids)
unresolved = sorted(r['stressor_id'] for r in coverage if r['analysis_status'] == 'unresolved')
assert unresolved == ['S319', 'S323', 'S338', 'S341', 'S344', 'S347']
files = ['scenarios.csv', 'states.csv', 'trajectories.csv', 'coverage.csv', 'report.md', 'build_review.py', 'validate_review.py']
result = {
    'result': 'PASS',
    'scope': 'Mechanical CSV headers, nonempty cells, exact scenario coverage, unique IDs, destination and inline state references, coverage/trajectory agreement, no unused states, report existence and evidence disclaimer.',
    'not_validated': 'Semantic truth, completeness of every causal branch, empirical attraction, implementation safety, legal authority, or real-world recoverability.',
    'counts': {'scenarios': len(scenarios), 'states': len(states), 'trajectories': len(trajectories), 'coverage': len(coverage)},
    'coverage_statuses': dict(Counter(r['analysis_status'] for r in coverage)),
    'state_kinds': dict(Counter(r['kind'] for r in states)),
    'unresolved_stressors': unresolved,
    'sha256': {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in files},
}
(ROOT / 'validation.json').write_text(json.dumps(result, indent=2, ensure_ascii=False) + '\n', encoding='utf-8')
print(json.dumps({key: result[key] for key in ['result', 'counts', 'coverage_statuses', 'unresolved_stressors']}, indent=2))
