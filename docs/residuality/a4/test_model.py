#!/usr/bin/env python3
"""Checks the documentation model, NOT Factory or a live monitor."""
import copy
import csv
import io
import re
import unittest
from collections import Counter
import build


class DocumentationModel(unittest.TestCase):
    def setUp(self):
        self.m = build.model()

    def test_shape_and_strategy_diversity(self):
        build.validate(self.m)
        self.assertEqual(Counter(s['lens'] for s in self.m['stressors'][200:]),
                         Counter({f'Q{i:02}': 3 for i in range(1, 51)}))
        self.assertEqual(len({q['strategy'] for q in self.m['strategies'].values()}), 50)

    def test_all_baseline_incidences_preserved(self):
        combined = list(csv.reader(io.StringIO(build.render(self.m)['incidence-matrix.csv'])))
        with (build.BASE / 'generated/incidence-matrix.csv').open() as f:
            baseline = list(csv.reader(f))
        self.assertEqual([row[:-1] for row in combined[:201]], baseline)
        self.assertTrue(all(row[-1] == '0' for row in combined[1:201]))

    def test_deterministic_derivation(self):
        self.assertEqual(build.render(self.m), build.render(copy.deepcopy(self.m)))

    def test_all_trace_edges_are_real(self):
        data = list(csv.DictReader(io.StringIO(build.render(self.m)['traceability.csv'])))
        expected = {(s['id'], c) for s in self.m['stressors'] for c in s['required_controls'].split()}
        self.assertEqual({(r['stressor'], r['control']) for r in data}, expected)
        self.assertEqual(len(data), len(expected))
        for row in data:
            control = self.m['controls'][row['control']]
            self.assertEqual((row['residue'], row['experiment']), (control['residue'], control['test']))

    def test_external_monitor_is_new_not_retroactive(self):
        controls = self.m['controls']
        self.assertEqual(build.assess(['C37', 'C38'], controls, 'A3')[0], 'fehlend')
        self.assertEqual(build.assess(['C37', 'C38'], controls, 'A4')[0], 'benannt')
        self.assertEqual(controls['C37']['residue'], 'R17')
        self.assertEqual(controls['C38']['residue'], 'R17')

    def test_monitor_and_tail_counterexamples_keep_obligations(self):
        cards = {s['id']: set(s['required_controls'].split()) for s in self.m['stressors']}
        for sid, required in {
            'S201': {'C37', 'C38'}, 'S213': {'C37'}, 'S215': {'C37', 'C06'},
            'S226': {'C41', 'C03'}, 'S231': {'C40', 'C34'},
            'S326': {'C37', 'C18'}, 'S347': {'C40', 'C24', 'C34'},
            'S350': {'C33', 'C34', 'C35', 'C38'},
        }.items():
            self.assertTrue(required <= cards[sid], sid)

    def test_loss_can_be_fully_named(self):
        card = next(s for s in self.m['stressors'] if s['id'] == 'S345')
        self.assertEqual(card['response_mode'], 'verlust')
        self.assertEqual(build.assess(card['required_controls'].split(), self.m['controls'], 'A4')[0],
                         'benannt')

    def test_open_controls_stay_missing_in_a4(self):
        self.assertEqual(build.assess(['C33', 'C34', 'C35', 'C36'], self.m['controls'], 'A4')[0],
                         'fehlend')

    def test_unknown_control_rejected(self):
        self.m['stressors'][-1]['required_controls'] += ' C99'
        with self.assertRaisesRegex(ValueError, 'Unknown control'):
            build.validate(self.m)

    def test_unknown_experiment_rejected(self):
        self.m['controls']['C37']['test'] = 'T99'
        with self.assertRaisesRegex(ValueError, 'Unknown residue/experiment'):
            build.validate(self.m)

    def test_duplicate_stressor_rejected(self):
        self.m['stressors'][-1]['stressor'] = self.m['stressors'][-2]['stressor']
        with self.assertRaisesRegex(ValueError, 'Duplicate stressor'):
            build.validate(self.m)

    def test_wrong_strategy_rejected(self):
        self.m['stressors'][-1]['lens'] = 'Q01'
        with self.assertRaisesRegex(ValueError, 'Wrong strategy'):
            build.validate(self.m)

    def test_silent_baseline_rewrite_rejected(self):
        self.m['controls']['C37']['introduced'] = 'A3'
        with self.assertRaisesRegex(ValueError, 'rewritten baseline'):
            build.validate(self.m)

    def test_silent_resolution_rejected(self):
        self.m['controls']['C34']['introduced'] = 'A4'
        with self.assertRaisesRegex(ValueError, 'silently resolve'):
            build.validate(self.m)

    def test_no_product_experiment_claimed_executed(self):
        self.m['experiments']['T18']['execution_status'] = 'bestanden'
        with self.assertRaisesRegex(ValueError, 'not executed'):
            build.validate(self.m)

    def test_architecture_explanation_matches_canonical_edges(self):
        cards = {s['id']: set(s['required_controls'].split()) for s in self.m['stressors']}
        rows = 0
        for line in (build.ROOT / 'architecture-A4.md').read_text(encoding='utf-8').splitlines():
            if not line.startswith('| S'):
                continue
            columns = line.split('|')
            ids = re.findall(r'S\d{3}', columns[1])
            residues = set(re.findall(r'R\d{2}', columns[2]))
            rules = set(re.findall(r'C\d{2}', columns[3]))
            tests = set(re.findall(r'T\d{2}', columns[4]))
            self.assertTrue(ids and residues and rules and tests)
            for sid in ids:
                self.assertTrue(cards[sid] & rules, (sid, rules))
            for key in rules:
                self.assertIn(self.m['controls'][key]['residue'], residues)
                self.assertIn(self.m['controls'][key]['test'], tests)
            rows += 1
        self.assertEqual(rows, 12)

    def test_revision_rows_and_generated_files(self):
        files = build.render(self.m)
        rows = list(csv.DictReader(io.StringIO(files['architecture-assessments.csv'])))
        self.assertEqual(len(rows), 1750)
        self.assertEqual(Counter(r['revision'] for r in rows), Counter({r: 350 for r in build.REVISIONS}))
        for name, content in files.items():
            self.assertEqual((build.ROOT / 'generated' / name).read_text(encoding='utf-8'), content, name)


if __name__ == '__main__':
    unittest.main()
