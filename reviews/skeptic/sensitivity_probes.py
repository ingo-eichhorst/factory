#!/usr/bin/env python3
"""Read-only A5 audit; all alternative rules are local analytical countermodels.

Run: PYTHONDONTWRITEBYTECODE=1 python3 sensitivity_probes.py > probe-results.json
No Factory process, plugin, service, or persistent task store is exercised.
"""
import copy
import csv
import hashlib
import json
import sys
from pathlib import Path
from unittest.mock import patch

sys.dont_write_bytecode = True
A5 = Path(__file__).resolve().parents[2] / 'docs/residuality/a5'
sys.path.insert(0, str(A5))
import dynamics as d
import derive

CASES = {c['id']: c for c in d.cases()}

def variant(cid, **params):
    c = copy.deepcopy(CASES[cid])
    c['params'].update(params)
    return c

def path(step, seed, ticks):
    states = [seed]
    for _ in range(ticks):
        states.append(step(states[-1]))
    return states

def summary(c):
    return [{'cycle': g['cycle'], 'kind': g['kind'], 'seed_count': len(g['runs']),
             'returned': g['returned'], 'tested': g['perturbations']}
            for g in d.analyze(c)]

results = {'boundary': 'Toy probes, not Factory observations', 'source_sha256': {
    n: hashlib.sha256((A5 / n).read_bytes()).hexdigest()
    for n in ['dynamics.py', 'cases.json', 'derive.py', 'regimes.csv', 'candidates.csv']}}

# P01: exact separators differ from a neutral family of fixed queues.
results['P01_queue'] = {
    'D01_seed_endpoints': {str(q): d.follow((q,), CASES['D01'])['cycle'] for q in range(3, 7)},
    'D04_seed_endpoints': {str(q): d.follow((q,), CASES['D04'])['cycle'] for q in range(5, 10)},
    'D04_q8_service_sweep': {str(s): d.follow((8,), variant('D04', service_congested=s))['cycle']
                            for s in [0, 1, 2]}}
assert d.follow((4,), CASES['D01'])['cycle'] == ((4,),)
assert d.follow((5,), CASES['D01'])['cycle'] == ((5,),)

# P02: an unchanged feedback mechanism need not produce a capped attractor.
def uncapped_restart(state):
    heat, wait, attempts, phase = state
    if phase == 'run':
        return (max(1, heat - 1), 0, attempts, phase)
    heat += 2  # same D06 feedback, remove only min(6, ...)
    return (heat, 0, 0, 'boot' if heat > 4 else 'run')
results['P02_restart'] = {
    'D06_original_hot': path(lambda s: d.step(s, CASES['D06']), (4, 0, 0, 'boot'), 8),
    'D06_uncapped_hot': path(uncapped_restart, (4, 0, 0, 'boot'), 8),
    'D08_cooldown_sweep': {str(t): summary(variant('D08', cooldown=t)) for t in range(5)}}
assert results['P02_restart']['D06_uncapped_hot'][-1][0] == 20

# P03: original alarm rules delete unresolved backlog, not merely new alerts.
def preserving_alarm(state):
    pending, mute = state
    if mute > 0:
        return (pending, mute - 1)  # mute does not acknowledge existing incidents
    pending = max(0, pending + 4 - 1)
    return (pending, 3 if pending > 6 else 0)
results['P03_alarm_loss'] = {
    'D11_original': path(lambda s: d.step(s, CASES['D11']), (0, 0), 24),
    'D11_preserve_unserved_during_mute': path(preserving_alarm, (0, 0), 24),
    'D13_group_six_unserved_plus_four_new': d.step((6, 0), CASES['D13']),
    'D10_mute_deletes_pending': d.step((5, -1), CASES['D10'])}
assert results['P03_alarm_loss']['D11_preserve_unserved_during_mute'][-1][0] > 6
assert results['P03_alarm_loss']['D13_group_six_unserved_plus_four_new'] == (1, 0)

# P04: do not confuse conditional constant-input cycles with periodic forcing.
results['P04_alarm_input_withdrawal'] = {
    'D11_no_more_arrivals': path(lambda s: d.step(s, variant('D11', arrival=0)), (6, 0), 8),
    'D10_no_more_arrivals_while_muted': path(lambda s: d.step(s, variant('D10', arrival=0)), (0, -1), 4),
    'D13_no_more_arrivals_existing_incident': path(lambda s: d.step(s, variant('D13', arrival=0)), (1, 0), 4)}
assert results['P04_alarm_input_withdrawal']['D11_no_more_arrivals'][-1] == (0, 0)
assert results['P04_alarm_input_withdrawal']['D13_no_more_arrivals_existing_incident'][-1] == (1, 0)

# P05: acceptance can be ordinary completion, with no repeated post-accept review.
def stop_on_accept(state, case):
    return state if state[2] == 'accepted' else d.step(state, case)
results['P05_verification_projection'] = {
    'D14_truth_flip': d.follow((4, False, 'accepted'), CASES['D14'])['cycle'],
    'D14_original_confidence_perturbation': d.follow((3, True, 'accepted'), CASES['D14'])['cycle'],
    'D14_stop_on_accept_confidence_perturbation': path(lambda s: stop_on_accept(s, CASES['D14']), (3, True, 'accepted'), 3),
    'D16_stop_on_accept': path(lambda s: stop_on_accept(s, CASES['D16']), (0, True, 'review'), 8)}
assert results['P05_verification_projection']['D14_truth_flip'] == ((4, False, 'accepted'),)

# P06: exact tested-axis evidence; seed dependence and large excursions are not
# bugs relative to A5's documented heuristic, but limit the mathematical claim.
original = summary(CASES['D14'])
with patch.object(d, 'seeds', return_value=[(4, True, 'accepted')]):
    endpoint_seeds = summary(CASES['D14'])
excursion_case = variant('D04')
def excursion_step(state, case):
    return {0: (0,), 1: (100,), 100: (0,), 2: (0,)}[state[0]]
with patch.object(d, 'step', side_effect=excursion_step), patch.object(d, 'seeds', return_value=[(2,)]):
    excursion_summary = summary(excursion_case)
    excursion_path = d.follow((1,), excursion_case)['path']
results['P06_basin_test_limits'] = {
    'D14_original_seeds': original, 'D14_endpoint_only_seeds': endpoint_seeds,
    'local_return_with_far_excursion': excursion_summary, 'excursion_path': excursion_path,
    'cycle_neighbors_already_in_cycle': {cid: sum(n in g['cycle'] for n in {
        n for s in g['cycle'] for n in d.neighbors(s, CASES[cid])})
        for cid in ['D08', 'D11'] for g in d.analyze(CASES[cid])}}
assert original[0]['kind'] == 'model-attractor'
assert endpoint_seeds[0]['kind'] == 'model-fixed-point'
assert excursion_summary[0]['kind'] == 'model-attractor'

# P07: regime join silently omits computed sets; no count target is asserted.
cases, regimes, candidates = derive.model()
missing = []
for cid, c in cases.items():
    for i, g in enumerate(d.analyze(c), 1):
        matching = [r['id'] for r in regimes.values() if cid in r['cases'].split() and r['kind'] == g['kind']]
        if not matching:
            missing.append({'set_id': f'{cid}.{i}', 'cycle': g['cycle'], 'kind': g['kind']})
results['P07_uninterpreted_sets'] = missing
assert {r['set_id'] for r in missing} == {'D01.3', 'D01.4'}

# P08: counts alone cannot prove any particular accepted job finishes. A FIFO
# and a starvation scheduler can realize precisely the same D05 scalar trace.
fifo = ['old-A', 'old-B']
starving = list(fifo)
service_log = []
for tick in range(4):
    incoming = [f'new-{tick}-{i}' for i in range(3)]  # q=2 gate admits 3
    fifo += incoming
    starving += incoming
    served_fifo, fifo = fifo[:3], fifo[3:]
    served_starving = starving[-3:]
    starving = starving[:-3]
    service_log.append({'q_next_both': len(fifo), 'FIFO_served': served_fifo,
                        'LIFO_served': served_starving, 'LIFO_remaining': list(starving)})
    assert len(fifo) == len(starving) == d.step((2,), CASES['D05'])[0]
results['P08_queue_identity'] = service_log
assert starving == ['old-A', 'old-B']

# P09: permanently silent accepted and unaccepted worlds are observationally
# equivalent to the sender, although their ground-truth effect counts differ.
accepted = path(lambda s: d.step(s, CASES['D19']), ('intent', 0, 0), 8)
def never_accepted(state):
    phase, effects, age = state
    if phase == 'intent':
        return ('unknown', 0, 0)
    return ('unknown', 0, min(3, age + 1))
unaccepted = path(never_accepted, ('intent', 0, 0), 8)
observe = lambda states: [(phase, age) for phase, _, age in states]
assert observe(accepted) == observe(unaccepted)
results['P09_information'] = {
    'accepted_ground_truth': accepted, 'unaccepted_ground_truth': unaccepted,
    'identical_sender_observations': observe(accepted),
    'D19_and_D22_same_endpoint': d.follow(d.seeds(CASES['D19'])[0], CASES['D19'])['cycle'] ==
                                d.follow(d.seeds(CASES['D22'])[0], CASES['D22'])['cycle'],
    'dedup_retention_counterfactuals': {cid: d.follow(d.seeds(CASES[cid])[0], CASES[cid])['cycle']
                                       for cid in ['D22', 'D23']}}

# P10: structural validation accepts a contradictory authored interpretation.
# Only an in-memory copy is changed, not any input file.
original_table = derive.table
def substituted_table(name):
    data = original_table(name)
    if name == 'regimes.csv':
        data['G01']['regime'] = 'Every accepted task has completed with independent correctness proof'
        data['G01']['sustaining_condition'] = 'An absent oracle guarantees useful completion'
    return data
with patch.object(derive, 'table', side_effect=substituted_table):
    _, changed_regimes, _ = derive.model()
results['P10_join_not_semantic_validation'] = {'accepted_counterfeit_G01': changed_regimes['G01']['regime']}
print(json.dumps(results, indent=2, ensure_ascii=False))
