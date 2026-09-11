#!/usr/bin/env python3
"""Structural and traceability tests only; no empirical resilience validation."""
import copy
import csv
import hashlib
import io
import json
from pathlib import Path
import unittest
import tempfile
import shutil
from unittest.mock import patch
import build
import bind_modules


class A7Integrity(unittest.TestCase):
    def test_every_scenario_has_explicit_branches(self):
        cases, _, _, _ = build.load_analysis()
        self.assertEqual({c['stressor_id'] for c in cases}, {f'S{n:03}' for n in range(1,351)})
        self.assertEqual(len(cases), 350)
        self.assertTrue(all(c['branches'] for c in cases))

    def test_residues_are_referenced_and_not_fixed_in_number(self):
        cases, residues, _, _ = build.load_analysis()
        used = {rid for c in cases for b in c['branches'] for rid in b['residue_ids']}
        self.assertEqual(used, set(residues))

    def test_every_residue_has_one_primary_owner(self):
        _, residues, _, _ = build.load_analysis()
        modules, bindings = build.load_architecture(residues)
        self.assertEqual(set(bindings), set(residues))
        self.assertTrue(all(b['primary_module'] in {m['id'] for m in modules} for b in bindings.values()))

    def test_no_residue_is_an_explicit_disposition(self):
        cases, _, _, _ = build.load_analysis()
        for c in cases:
            for b in c['branches']:
                if not b['residue_ids']:
                    self.assertIn(b['residue_status'], {'keines','ungeklaert'})
                    self.assertTrue(b['reason'] and b['validation'])

    def change_first_case(self, change):
        original = json.loads
        def changed(text, *args, **kwargs):
            value = original(text, *args, **kwargs)
            if isinstance(value,list) and value and isinstance(value[0],dict) and value[0].get('stressor_id')=='S001':
                change(value)
            return value
        return patch.object(build.json, 'loads', side_effect=changed)

    def test_missing_scenario_fails(self):
        with self.change_first_case(lambda rows: rows.pop()):
            with self.assertRaisesRegex(ValueError, 'Missing/duplicate/extra scenarios'):
                build.load_analysis()

    def test_duplicate_scenario_fails(self):
        with self.change_first_case(lambda rows: rows.__setitem__(-1,copy.deepcopy(rows[0]))):
            with self.assertRaisesRegex(ValueError, 'Missing/duplicate/extra scenarios'):
                build.load_analysis()

    def test_invented_state_kind_fails(self):
        with self.change_first_case(lambda rows: rows[0]['branches'][0].update(kind='proven-attractor')):
            with self.assertRaisesRegex(ValueError, 'Unknown state kind'):
                build.load_analysis()

    def test_unknown_historical_state_fails(self):
        with self.change_first_case(lambda rows: rows[0].update(source_states=['INVENTED'])):
            with self.assertRaisesRegex(ValueError, 'Unknown source state'):
                build.load_analysis()

    def test_unknown_residue_fails(self):
        with self.change_first_case(lambda rows: rows[0]['branches'][0].update(residue_ids=['INVENTED'])):
            with self.assertRaisesRegex(ValueError, 'Unknown residue'):
                build.load_analysis()

    def test_unexplained_empty_residue_fails(self):
        with self.change_first_case(lambda rows: rows[0]['branches'][0].update(residue_ids=[],residue_status='bedingt')):
            with self.assertRaisesRegex(ValueError, 'Unexplained missing residue'):
                build.load_analysis()

    def test_no_branch_fails(self):
        with self.change_first_case(lambda rows: rows[0].update(branches=[])):
            with self.assertRaisesRegex(ValueError, 'No branches'):
                build.load_analysis()

    def test_missing_module_binding_fails(self):
        original = build.read_csv
        def changed(path, *args, **kwargs):
            rows = original(path, *args, **kwargs)
            return rows[:-1] if path.name=='module-bindings.csv' else rows
        with patch.object(build,'read_csv',side_effect=changed):
            _, residues, _, _ = build.load_analysis()
            with self.assertRaisesRegex(ValueError, 'Missing/duplicate residue module binding'):
                build.load_architecture(residues)

    def test_branch_trace_does_not_silently_omit_negative_cases(self):
        files = build.render()
        branches = list(csv.DictReader(io.StringIO(files['branches.csv'])))
        links = list(csv.DictReader(io.StringIO(files['traceability.csv'])))
        self.assertEqual({b['branch_id'] for b in branches}, {r['branch_id'] for r in links})
        for b in branches:
            actual = {r['residue_id'] for r in links if r['branch_id']==b['branch_id']}
            self.assertEqual(actual,set(b['residue_ids'].split()))

    def test_readable_catalogue_has_all_cases_and_candidates(self):
        cases,residues,_,_ = build.load_analysis()
        files = build.render()
        for c in cases:
            self.assertIn(f'<a id="{c["stressor_id"].lower()}"></a>', files['stressoren.md'])
        for rid in residues:
            self.assertIn(f'<a id="{rid.lower()}"></a>', files['residues.md'])
        self.assertIn('keine beobachteten Attraktoren', files['stressoren.md'])

    def test_a6_originals_still_match_freeze(self):
        hashes = json.loads((build.ROOT.parent/'a6/generated/source-hashes.json').read_text())
        for path,digest in hashes.items():
            self.assertEqual(hashlib.sha256((build.PROJECT/path).read_bytes()).hexdigest(),digest,path)

    def test_a7_independent_outputs_remain_frozen(self):
        frozen = json.loads((build.ROOT/'discovery-freeze.json').read_text())['sha256']
        self.assertEqual(len(frozen), 12)
        for path,digest in frozen.items():
            self.assertEqual(hashlib.sha256((build.PROJECT/path).read_bytes()).hexdigest(),digest,path)

    def test_all_historical_origins_match_the_original_disposition(self):
        cases,_,_,_ = build.load_analysis()
        original = {r['stressor_id']:set(r['state_ids'].split())-{'none'}
                    for role in build.ROLES for r in build.read_csv(build.PROJECT/'reviews'/role/'coverage.csv')}
        for c in cases:
            self.assertEqual(set(c['source_states']),original[c['stressor_id']],c['stressor_id'])

    def test_module_bindings_reproduce_explicit_assignments(self):
        self.assertEqual((build.ROOT/'module-bindings.csv').read_text(),bind_modules.render())

    def test_every_branch_has_its_own_readable_anchor(self):
        files=build.render()
        branches=list(csv.DictReader(io.StringIO(files['branches.csv'])))
        self.assertEqual(len(branches),len({b['branch_id'] for b in branches}))
        for b in branches:
            self.assertIn(f'<a id="{b["branch_id"].lower()}"></a>',files['stressoren.md'])

    def test_generated_files_reproduce(self):
        files = build.render()
        self.assertEqual(files,build.render())
        for name,text in files.items():
            self.assertEqual((build.ROOT/'generated'/name).read_text(),text,name)

    def test_rejected_package_does_not_invent_correct_package(self):
        cases,_,_,_ = build.load_analysis()
        c=next(c for c in cases if c['stressor_id']=='S156')
        self.assertEqual(c['branches'][0]['residue_ids'],['GVR005'])
        self.assertIn('noch nicht erzeugt',c['branches'][0]['conditions'])
        self.assertEqual(c['branches'][2]['residue_ids'],['GVR036'])

    def test_unspent_authorization_is_not_past_attempt(self):
        cases,_,_,_ = build.load_analysis()
        c=next(c for c in cases if c['stressor_id']=='S074')
        self.assertNotIn('OPR002',c['branches'][1]['residue_ids'])
        self.assertIn('KOR001',c['branches'][1]['residue_ids'])
        self.assertEqual(set(c['branches'][2]['residue_ids']),{'OPR002','KOR001'})

    def test_subscriber_cursor_is_not_runtime_observation(self):
        cases,_,_,_ = build.load_analysis()
        c=next(c for c in cases if c['stressor_id']=='S054')
        for b in c['branches'][:2]:
            self.assertIn('OBR029',b['residue_ids'])
            self.assertNotIn('OPR014',b['residue_ids'])

    def test_growth_and_conflict_keep_different_classifications(self):
        cases,_,_,_ = build.load_analysis()
        by_id={c['stressor_id']:c for c in cases}
        for sid in ['S130','S196']:
            self.assertEqual(by_id[sid]['branches'][1]['kind'],'eskalation')
        for sid in ['S235','S245']:
            self.assertEqual(by_id[sid]['branches'][1]['kind'],'konflikt')

    def test_compiler_rejects_a_bootstrap_cycle(self):
        original=build.read_csv
        def cyclic(path,*args,**kwargs):
            rows=original(path,*args,**kwargs)
            if path.name=='module-dependencies.csv':
                rows.append(dict(source='M07',target='M01',phase='bootstrap',contract='synthetic',failure_response='reject'))
            return rows
        with patch.object(build,'read_csv',side_effect=cyclic):
            with self.assertRaisesRegex(ValueError,'Bootstrap dependency cycle'):
                build.render()

    def test_reordering_review_target_in_memory_fails(self):
        original=json.loads
        def changed(text,*args,**kwargs):
            value=original(text,*args,**kwargs)
            if isinstance(value,list) and value and isinstance(value[0],dict) and value[0].get('stressor_id')=='S101':
                next(c for c in value if c['stressor_id']=='S156')['branches'].reverse()
            return value
        with patch.object(build.json,'loads',side_effect=changed):
            with self.assertRaisesRegex(ValueError,'Correction original mismatch'):
                build.render()

    def test_actual_changed_frozen_file_fails_before_parsing(self):
        frozen=json.loads((build.ROOT/'discovery-freeze.json').read_text())['sha256']
        with tempfile.TemporaryDirectory() as directory:
            project=Path(directory)
            for source in frozen:
                destination=project/source
                destination.parent.mkdir(parents=True,exist_ok=True)
                shutil.copyfile(build.PROJECT/source,destination)
            p=project/'reviews/operations/a7/cases.json'
            p.write_text(p.read_text()+'\n')
            with patch.object(build,'PROJECT',project):
                with self.assertRaisesRegex(ValueError,'A7 frozen source changed'):
                    build.render()

    def test_independent_modes_do_not_require_running_factory(self):
        modes={r['id']:r for r in build.read_csv(build.ROOT/'operation-boundaries.csv')}
        for mode in ['O02','O04','O05','O06','O07','O08']:
            self.assertEqual(modes[mode]['required_factory_runtime'],'none')
        _,residues,_,_=build.load_analysis()
        _,bindings=build.load_architecture(residues)
        for rid in ['GVR035','BDR026','BDR027']:
            self.assertEqual(bindings[rid]['support_modules'],'none')
        self.assertEqual(bindings['BDR037']['primary_module'],'M13')

    def test_boot_dependencies_have_no_cycle(self):
        rows = build.read_csv(build.ROOT/'module-dependencies.csv')
        modules = {m['id'] for m in build.read_csv(build.ROOT/'modules.csv')}
        graph = {m:set() for m in modules}
        for r in rows:
            self.assertIn(r['source'],modules)
            self.assertIn(r['target'],modules)
            self.assertIn(r['phase'],{'bootstrap','command','optional'})
            self.assertTrue(r['contract'] and r['failure_response'])
            if r['phase']=='bootstrap':
                graph[r['source']].add(r['target'])
        pending = dict(graph)
        while pending:
            ready = {m for m,deps in pending.items() if not deps & pending.keys()}
            self.assertTrue(ready,'Bootstrap dependency cycle')
            pending = {m:d for m,d in pending.items() if m not in ready}


if __name__ == '__main__':
    unittest.main()
