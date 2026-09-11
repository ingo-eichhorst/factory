#!/usr/bin/env python3
"""Small explicit toy dynamics, NOT a Factory simulator or production test.

No old control/residue labels are read. End sets are found by iterating the
transition rules and detecting repetition, not by selecting a residue count.
"""
import argparse
import csv
import io
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent
OWNER = 'Generated exclusively by docs/residuality/a5/dynamics.py.\n'
ESCAPE = ('outside-observation-window',)


def scenarios():
    result = {}
    for path in (ROOT.parent / 'stressors.csv', ROOT.parent / 'a4/stressors.csv'):
        with path.open(encoding='utf-8', newline='') as f:
            for row in csv.DictReader(f, delimiter=';'):
                # Deliberately discard the old categories, assumptions, controls,
                # proposed survivals and outcomes. The author is not blinded.
                if row['id'] in result:
                    raise ValueError('Duplicate scenario ID')
                result[row['id']] = row['stressor']
    return result


def cases():
    rows = json.loads((ROOT / 'cases.json').read_text(encoding='utf-8'))
    if len({r['id'] for r in rows}) != len(rows):
        raise ValueError('Duplicate case ID')
    known = scenarios()
    for r in rows:
        if not r['stressors'] or not set(r['stressors']) <= known.keys():
            raise ValueError('Unknown or missing stressor: ' + r['id'])
        if r['model'] not in {'queue', 'restart', 'alarm', 'verification', 'writer', 'effect'}:
            raise ValueError('Unknown model')
    return rows


def seeds(case):
    model, p = case['model'], case['params']
    if model == 'queue':
        return [(q,) for q in range(13)]
    if model == 'restart':
        return [(h, 0, 0, 'boot') for h in range(7)]
    if model == 'alarm':
        return [(q, 0) for q in range(7)]
    if model == 'verification':
        return [(confidence, True, 'review') for confidence in range(5)]
    if model == 'writer':
        return [(value, turn) for value in range(2) for turn in range(2)]
    return [('intent', int(p['restore']), 0)]


def step(state, case):
    model, p = case['model'], case['params']
    if state == ESCAPE:
        return state
    if model == 'queue':
        q, = state
        retry = q // 2 if p['retry'] and q >= 4 else 0
        incoming = p['arrival'] + retry
        if p['gate']:
            incoming = min(incoming, max(0, 5 - q))
        service = p.get('service_low', 3) if q < 6 else p.get('service_congested', 1)
        new_q = max(0, q + incoming - service)
        return ESCAPE if new_q > 100 else (new_q,)
    if model == 'restart':
        heat, wait, attempts, phase = state
        if phase == 'quarantine':
            return (max(0, heat - 1), 0, attempts, phase)
        if phase == 'run':
            return (max(1, heat - 1), 0, attempts, phase)
        if wait:
            return (max(0, heat - 1), wait - 1, attempts, phase)
        heat = min(6, heat + 2)
        attempts = min(3, attempts + 1) if p['quarantine'] else 0
        if p['bad_binary'] or heat > 4:
            if p['quarantine'] and attempts == 3:
                return (heat, 0, attempts, 'quarantine')
            return (heat, p['cooldown'], attempts, 'boot')
        return (heat, 0, attempts, 'run')
    if model == 'alarm':
        pending, mute = state
        if mute == -1:
            return (0, -1)
        if mute > 0:
            return (0, mute - 1)
        offered = pending + p['arrival']
        if p['group']:
            offered = min(1, offered)
        pending = max(0, offered - p['human_capacity'])
        if pending > 6:
            return (0, p['mute_ticks'])
        return (pending, 0)
    if model == 'verification':
        confidence, wrong, phase = state
        if phase == 'held':
            return state
        if wrong and p['verifier'] == 'block':
            return (0, True, 'held')
        if wrong and p['verifier'] == 'repair':
            return (0, False, 'review')
        confidence = min(4, confidence + 1)
        return (confidence, wrong, 'accepted' if confidence >= 3 else 'review')
    if model == 'writer':
        value, turn = state
        if not p['fence'] or turn == 0:
            value = turn
        return (value, 1 - turn)
    local, effects, age = state
    if local in {'confirmed', 'damage', 'quarantine'}:
        return state
    if local == 'intent':
        if p['restore'] and p['restore_gate']:
            return ('quarantine', effects, age)
        duplicate_protected = p['dedup'] and p['remember'] and effects > 0
        effects += int(not duplicate_protected)
        return ('damage' if effects >= 2 else 'unknown', effects, 0)
    if p['response'] == 'late' and age >= 3:
        return ('confirmed', effects, age)
    if p['retry']:
        effects += int(not (p['dedup'] and p['remember']))
        if effects >= 2:
            return ('damage', effects, age)
    return ('unknown', effects, min(3, age + 1))


def canonical_cycle(cycle):
    rotations = [tuple(cycle[i:] + cycle[:i]) for i in range(len(cycle))]
    return min(rotations, key=lambda x: json.dumps(x, sort_keys=True))


def follow(seed, case, horizon=2000):
    state, seen, path = seed, {}, []
    for _ in range(horizon):
        if state == ESCAPE:
            return {'path': path + [state], 'cycle': (ESCAPE,), 'transient': len(path), 'escape': True}
        if state in seen:
            start = seen[state]
            return {'path': path, 'cycle': canonical_cycle(path[start:]),
                    'transient': start, 'escape': False}
        seen[state] = len(path)
        path.append(state)
        state = step(state, case)
    raise ValueError('Unclassified transient exceeds search horizon: ' + case['id'])


def neighbors(state, case):
    if state == ESCAPE:
        return []
    model = case['model']
    if model in {'queue', 'alarm', 'restart', 'verification'}:
        top = {'queue': 100, 'alarm': 6, 'restart': 6, 'verification': 4}[model]
        return [(state[0] + delta, *state[1:]) for delta in (-1, 1) if 0 <= state[0] + delta <= top]
    if model == 'writer':
        return [(1 - state[0], state[1])]
    return [(state[0], state[1], max(0, state[2] - 1))] if state[2] else []


def interpretation(case, cycle, attracting):
    model, p = case['model'], case['params']
    state = cycle[0]
    if state == ESCAPE:
        return 'escape-not-attractor', 'Queue exceeds observation window; do not turn divergence into a capped fixed point'
    if model == 'queue':
        return ('model-attractor' if attracting else 'fixed-without-local-attraction'), f'Pending queue {state[0]}'
    if model == 'restart':
        if state[3] == 'quarantine':
            return 'policy-hold', 'No automatic restart; host resources can cool'
        if state[3] == 'run':
            return 'model-attractor' if attracting else 'model-fixed-point', 'Process running after bootstrap'
        return ('model-attractor' if attracting else 'model-recurrent-set'), 'Repeated failed starts; backoff may only change the period'
    if model == 'alarm':
        if state[1] == -1:
            return 'model-attractor' if attracting else 'model-fixed-point', 'Permanent suppression induced by the assumed overload response'
        if len(cycle) > 1:
            return 'model-attractor' if attracting else 'model-recurrent-set', 'Alarm flood and mute alternate'
        if state[0] > 0:
            return 'externally-maintained-hold', 'Bounded pending incident but no human service'
        return 'model-attractor' if attracting else 'model-fixed-point', 'Alert queue serviceable under the assumed arrival grouping'
    if model == 'verification':
        if state[2] == 'held':
            return 'policy-hold', 'Wrong artifact retained but not accepted'
        return ('model-attractor' if attracting else 'model-fixed-point'), ('Wrong artifact confidently accepted' if state[1] else 'Corrected artifact accepted under a stipulated perfect repair oracle')
    if model == 'writer':
        values = {s[0] for s in cycle}
        if len(values) > 1:
            return 'externally-driven-cycle', 'Alternating writers overwrite one another'
        return 'clock-cycle-only', 'Writer turn still alternates but business value is stable under enforced fencing'
    if state[0] == 'damage':
        return 'absorbing-damage-fact', 'At least two effects happened; future service state is not modelled'
    if state[0] == 'confirmed':
        return 'ordinary-completion', 'One effect confirmed after delay; not evidence of autonomous attraction'
    if state[0] == 'quarantine':
        return 'policy-hold', 'Restored intent cannot dispatch automatically'
    return 'information-starved-hold', ('Outcome unknown; repeated calls are deduplicated but still consume resources' if p['retry'] else 'Outcome unknown; no new call is made')


def analyze(case):
    groups = {}
    for seed in seeds(case):
        run = follow(seed, case)
        groups.setdefault(run['cycle'], []).append((seed, run))
    result = []
    for cycle, runs in sorted(groups.items(), key=lambda item: json.dumps(item[0])):
        perturbations = sorted({n for state in cycle for n in neighbors(state, case)}, key=str)
        returned = sum(follow(n, case)['cycle'] == cycle for n in perturbations)
        nontrivial_basin = any(run['transient'] > 0 for _, run in runs)
        attracting = nontrivial_basin and bool(perturbations) and returned == len(perturbations)
        kind, meaning = interpretation(case, cycle, attracting)
        result.append({'cycle': cycle, 'runs': runs, 'perturbations': len(perturbations),
                       'returned': returned, 'local_return': attracting, 'kind': kind, 'meaning': meaning})
    return result


def csv_text(headers, rows):
    out = io.StringIO(newline='')
    writer = csv.writer(out, lineterminator='\n')
    writer.writerow(headers)
    writer.writerows(rows)
    return out.getvalue()


def render():
    source, experiments = scenarios(), cases()
    outputs = {'OWNERSHIP.txt': OWNER}
    outputs['scenario-text.csv'] = csv_text(['stressor', 'scenario'], source.items())
    basin_rows, trace_rows, coverage = [], [], {s: [] for s in source}
    report = ['<!-- ' + OWNER.strip() + ' -->', '# Model-derived terminal and recurrent sets', '',
              '**No empirically observed Factory attractor is established here.**',
              'These are exact executions of deliberately small, stipulated toy transition rules.',
              'A locally returning set in this grid is not a proof of robustness to different equations, scales or inputs.', '',
              '| Case | Recurrent set / boundary | Kind | Seed basin | Local perturbations returning | Meaning |',
              '|---|---|---|---:|---:|---|']
    types = Counter()
    for case in experiments:
        for s in case['stressors']:
            coverage[s].append(case['id'])
        for index, group in enumerate(analyze(case), 1):
            set_id = f'{case["id"]}.{index}'
            types[group['kind']] += 1
            state_repr = json.dumps(group['cycle'], ensure_ascii=False)
            basin_rows.append([set_id, case['id'], case['model'], group['kind'], state_repr,
                               len(group['runs']), group['perturbations'], group['returned'], group['meaning']])
            report.append('| ' + ' | '.join([case['id'], '`' + state_repr + '`', group['kind'],
                str(len(group['runs'])), f'{group["returned"]}/{group["perturbations"]}', group['meaning']]) + ' |')
            for seed, run in group['runs']:
                trace_rows.append([case['id'], json.dumps(seed), set_id, run['transient'],
                                   json.dumps(run['path'], ensure_ascii=False)])
    outputs['basins.csv'] = csv_text(['set_id', 'case', 'model', 'kind', 'recurrent_states', 'seed_count',
                                     'perturbation_count', 'returned_count', 'meaning'], basin_rows)
    outputs['trajectories.csv'] = csv_text(['case', 'seed', 'end_set', 'transient_steps', 'path'], trace_rows)
    outputs['screening.csv'] = csv_text(['stressor', 'scenario', 'toy_cases', 'evidence'],
        [[s, text, ' '.join(coverage[s]), 'toy-mechanism-linked' if coverage[s] else 'text-screened-only']
         for s, text in source.items()])
    n_linked = sum(bool(c) for c in coverage.values())
    report += ['', '## Scope and exclusions', '',
        f'- {len(source)} scenario texts screened; {n_linked} linked to a toy mechanism; '
        f'{len(source) - n_linked} have no toy trajectory in this pass.',
        f'- {len(experiments)} parameter cases across {len({c["model"] for c in experiments})} toy mechanisms.',
        f'- {len(trace_rows)} initial-state runs. Their seed grid is chosen, not a probability distribution.',
        '- Basin counts are grid cardinalities, not likelihoods of incidents or deployment outcomes.',
        '- A source scenario link means the case isolates one mechanism, not that every detail of that scenario was simulated.',
        '- Queue escape is not counted as an attractor. Writer scheduling is explicitly external forcing.',
        '- Policy holds, information starvation, ordinary completion and irreversible damage are separated from model attractors.',
        '', '## Recurrent-set classifications (case-specific, not unique business states)', '']
    report += [f'- {kind}: {count}' for kind, count in sorted(types.items())]
    outputs['model-results.md'] = '\n'.join(report) + '\n'
    return outputs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    outputs = render()
    out = ROOT / 'generated'
    if args.check:
        stale = [name for name, text in outputs.items() if not (out / name).exists() or
                 (out / name).read_text(encoding='utf-8') != text]
        if stale:
            raise SystemExit('Missing/stale: ' + ', '.join(stale))
    else:
        if out.exists() and (not (out / 'OWNERSHIP.txt').exists() or
                             (out / 'OWNERSHIP.txt').read_text(encoding='utf-8') != OWNER):
            raise SystemExit('No matching ownership marker; refusing overwrite')
        out.mkdir(exist_ok=True)
        for name, text in outputs.items():
            (out / name).write_text(text, encoding='utf-8')
    print(f'{len(outputs)} A5 model files ' + ('checked' if args.check else 'written'))


if __name__ == '__main__':
    main()
