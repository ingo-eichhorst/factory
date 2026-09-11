#!/usr/bin/env python3
"""Tests of the stipulated toy rules and documentation, not Factory behavior."""
import copy
import csv
import io
import unittest
from unittest.mock import patch
import dynamics as d
import derive


class StateFirstAnalysis(unittest.TestCase):
    def setUp(self):
        self.cases = {c['id']: c for c in d.cases()}

    def end(self, case, seed=None):
        case = self.cases[case]
        return d.follow(d.seeds(case)[0] if seed is None else seed, case)['cycle']

    def test_every_selected_seed_terminates_or_repeats_within_search(self):
        for case in self.cases.values():
            for seed in d.seeds(case):
                self.assertTrue(d.follow(seed, case)['cycle'])

    def test_queue_has_different_basins(self):
        self.assertEqual(self.end('D01', (3,)), ((0,),))
        self.assertEqual(self.end('D01', (8,)), (d.ESCAPE,))

    def test_fixed_separator_is_not_called_an_attractor(self):
        separator = next(g for g in d.analyze(self.cases['D01']) if g['cycle'] == ((4,),))
        self.assertFalse(separator['local_return'])
        self.assertEqual(separator['kind'], 'fixed-without-local-attraction')

    def test_removing_only_fresh_demand_is_insufficient(self):
        self.assertEqual(self.end('D03', (8,)), (d.ESCAPE,))
        self.assertEqual(self.end('D26', (8,)), ((0,),))

    def test_stopping_retry_growth_is_not_necessarily_drain(self):
        self.assertEqual(self.end('D04', (8,)), ((8,),))
        self.assertEqual(self.end('D02', (8,)), ((0,),))

    def test_parameters_can_change_the_plateau(self):
        case = copy.deepcopy(self.cases['D04'])
        case['params']['service_congested'] = 2
        self.assertEqual(d.follow((8,), case)['cycle'], ((0,),))

    def test_bounded_queue_can_remain_productively_nonempty(self):
        for seed in d.seeds(self.cases['D05']):
            self.assertEqual(self.end('D05', seed), ((2,),))

    def test_restart_has_hot_and_running_basins(self):
        self.assertEqual(self.end('D06', (0, 0, 0, 'boot'))[0][3], 'run')
        self.assertEqual(self.end('D06', (4, 0, 0, 'boot'))[0][3], 'boot')
        self.assertEqual(self.end('D07', (4, 0, 0, 'boot'))[0][3], 'run')

    def test_backoff_does_not_repair_bad_binary(self):
        cycle = self.end('D08')
        self.assertGreater(len(cycle), 1)
        self.assertTrue(all(s[3] == 'boot' for s in cycle))
        self.assertEqual(self.end('D09')[0][3], 'quarantine')

    def test_expiring_mute_can_form_a_cycle(self):
        self.assertEqual(self.end('D10'), ((0, -1),))
        self.assertGreater(len(self.end('D11')), 1)
        self.assertEqual(self.end('D12'), ((0, 0),))

    def test_grouping_does_not_create_a_human(self):
        self.assertEqual(self.end('D13'), ((1, 0),))
        self.assertEqual(d.analyze(self.cases['D13'])[0]['kind'], 'externally-maintained-hold')

    def test_echo_can_stabilize_wrong_acceptance(self):
        self.assertEqual(self.end('D14'), ((4, True, 'accepted'),))
        self.assertEqual(self.end('D15')[0][2], 'held')
        self.assertEqual(self.end('D16'), ((4, False, 'accepted'),))

    def test_scheduling_cycle_is_not_business_cycle(self):
        self.assertEqual({s[0] for s in self.end('D17')}, {0, 1})
        self.assertEqual({s[0] for s in self.end('D18')}, {0})
        self.assertEqual(d.analyze(self.cases['D18'])[0]['kind'], 'clock-cycle-only')

    def test_uncertainty_completion_and_damage_are_different(self):
        self.assertEqual(self.end('D19')[0][0], 'unknown')
        self.assertEqual(self.end('D20')[0][0], 'damage')
        self.assertEqual(self.end('D21')[0][0], 'confirmed')
        self.assertEqual(d.analyze(self.cases['D21'])[0]['kind'], 'ordinary-completion')

    def test_same_unknown_state_does_not_mean_same_retry_policy(self):
        self.assertEqual(self.end('D19'), self.end('D22'))
        self.assertFalse(self.cases['D19']['params']['retry'])
        self.assertTrue(self.cases['D22']['params']['retry'])
        self.assertEqual(self.end('D23')[0][0], 'damage')

    def test_restore_gate_differs_from_safe_unknown_retry_policy(self):
        self.assertFalse(self.cases['D24']['params']['retry'])
        self.assertEqual(self.end('D24')[0][0], 'damage')
        self.assertEqual(self.end('D25')[0][0], 'quarantine')

    def test_cycle_phase_is_canonicalized(self):
        self.assertEqual(d.canonical_cycle([(0,), (1,)]), d.canonical_cycle([(1,), (0,)]))

    def test_screening_does_not_claim_every_scenario_was_modelled(self):
        rows = list(csv.DictReader(io.StringIO(d.render()['screening.csv'])))
        self.assertEqual({r['stressor'] for r in rows}, d.scenarios().keys())
        linked = {s for case in self.cases.values() for s in case['stressors']}
        for row in rows:
            self.assertEqual(row['evidence'] == 'toy-mechanism-linked', row['stressor'] in linked)
        self.assertTrue(any(r['evidence'] == 'text-screened-only' for r in rows))

    def test_model_is_independent_of_downstream_candidates(self):
        before = d.render()
        with patch.object(derive, 'table', side_effect=AssertionError('No residue labels in dynamics')):
            self.assertEqual(d.render(), before)
        inputs = list(csv.DictReader(io.StringIO(before['scenario-text.csv'])))
        self.assertEqual(set(inputs[0]), {'stressor', 'scenario'})

    def test_derived_references_and_hypothesis_status(self):
        cases, regimes, candidates = derive.model()
        self.assertTrue(regimes and candidates)
        self.assertEqual(candidates['N10']['status'], 'unproven-prerequisite')
        for row in csv.DictReader(io.StringIO(derive.render()['traceability.csv'])):
            self.assertIn(row['stressor'], cases[row['case']]['stressors'])
            self.assertIn(row['case'], regimes[row['regime']]['cases'].split())
            self.assertIn(row['regime'], candidates[row['candidate']]['regimes'].split())

    def test_unknown_regime_fails_without_enforcing_a_candidate_count(self):
        original = derive.table
        def changed(name):
            data = original(name)
            if name == 'candidates.csv':
                data[next(iter(data))]['regimes'] = 'does-not-exist'
            return data
        with patch.object(derive, 'table', side_effect=changed):
            with self.assertRaisesRegex(ValueError, 'Unknown regime'):
                derive.model()

    def test_both_derivations_are_reproducible(self):
        for directory, outputs in [('generated', d.render()), ('derived', derive.render())]:
            for name, text in outputs.items():
                self.assertEqual((d.ROOT / directory / name).read_text(encoding='utf-8'), text)


if __name__ == '__main__':
    unittest.main()
