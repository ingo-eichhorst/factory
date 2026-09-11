#!/usr/bin/env python3
"""Integrity checks, not validation of the reviewers' causal claims."""
import copy
import csv
import hashlib
import io
import json
import unittest
from unittest.mock import patch
import aggregate
import prepare_inputs
import build_a5_corrections
import reviewed_index
import re


class IndependentReviewIntegrity(unittest.TestCase):
    def test_neutral_inputs_and_ranges(self):
        outputs, manifest = prepare_inputs.prepared()
        seen = set()
        for role, text in outputs.items():
            rows = list(csv.DictReader(io.StringIO(text), delimiter=';'))
            self.assertEqual(set(rows[0]), {'stressor_id', 'scenario'})
            ids = {r['stressor_id'] for r in rows}
            self.assertFalse(seen & ids)
            seen |= ids
            self.assertEqual((aggregate.PROJECT / 'reviews' / role / 'scenarios.csv').read_text(), text)
        self.assertEqual(seen, {f'S{i:03}' for i in range(1, 351)})
        self.assertEqual((aggregate.ROOT / 'input-manifest.json').read_text(), manifest)

    def test_full_disposition_not_fixed_state_count(self):
        tables, counts, snapshot = aggregate.model()
        self.assertEqual(len(tables['coverage.csv']), 350)
        self.assertEqual(sum(r['scenarios'] for r in counts.values()), 350)
        self.assertEqual(sum(r['states'] for r in counts.values()), len(tables['states.csv']))
        self.assertEqual(len(snapshot), 4 * len(counts))

    def test_global_identifiers_are_unambiguous(self):
        tables, _, _ = aggregate.model()
        for table, key in [('states.csv', 'state_id'), ('trajectories.csv', 'trajectory_id')]:
            self.assertEqual(len({r[key] for r in tables[table]}), len(tables[table]))

    def changed_coverage(self, mutate):
        original = aggregate.load
        def changed(path, headers):
            rows = copy.deepcopy(original(path, headers))
            if path.name == 'coverage.csv':
                mutate(rows[0])
            return rows
        return patch.object(aggregate, 'load', side_effect=changed)

    def test_unknown_state_fails(self):
        with self.changed_coverage(lambda row: row.update(state_ids='MISSING-STATE')):
            with self.assertRaisesRegex(ValueError, 'Unknown coverage state'):
                aggregate.model()

    def test_no_state_requires_honest_unresolved_label(self):
        with self.changed_coverage(lambda row: row.update(state_ids='none', analysis_status='conditional')):
            with self.assertRaisesRegex(ValueError, 'explicitly unresolved'):
                aggregate.model()

    def test_unknown_status_fails(self):
        with self.changed_coverage(lambda row: row.update(analysis_status='empirically-proven')):
            with self.assertRaisesRegex(ValueError, 'Unknown coverage status'):
                aggregate.model()

    def test_missing_coverage_fails(self):
        original = aggregate.load
        def changed(path, headers):
            rows = original(path, headers)
            return rows[1:] if path.name == 'coverage.csv' else rows
        with patch.object(aggregate, 'load', side_effect=changed):
            with self.assertRaisesRegex(ValueError, 'Missing or extra coverage'):
                aggregate.model()

    def test_generated_inventory_keeps_provenance(self):
        outputs = aggregate.render()
        rows = list(csv.DictReader(io.StringIO(outputs['states.csv'])))
        self.assertEqual({r['reviewer'] for r in rows}, set(prepare_inputs.RANGES))
        self.assertIn('Counts do not mean distinct real attractors', outputs['summary.md'])
        self.assertIn('not proof that a scenario is modelled', outputs['summary.md'])

    def test_artifact_hashes_match(self):
        _, _, hashes = aggregate.model()
        for path, digest in hashes.items():
            self.assertEqual(hashlib.sha256((aggregate.PROJECT / path).read_bytes()).hexdigest(), digest)

    def test_discovery_snapshot_still_matches_pre_cross_review(self):
        freeze = json.loads((aggregate.ROOT / 'discovery-freeze.json').read_text())
        path = aggregate.ROOT / freeze['artifact_hash_manifest']
        self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), freeze['sha256'])

    def test_every_a5_computed_set_has_a_disposition(self):
        files = build_a5_corrections.render()
        rows = list(csv.DictReader(io.StringIO(files['a5-set-dispositions.csv'])))
        original = build_a5_corrections.read_csv(build_a5_corrections.A5 / 'generated/basins.csv', ',')
        self.assertEqual({r['set_id'] for r in rows}, {r['set_id'] for r in original})
        self.assertEqual(len(rows), len(original))
        excluded = {r['set_id'] for r in rows if r['review_disposition']=='explicitly-excluded-from-residue-evidence'}
        self.assertEqual(excluded, {'D01.3', 'D01.4'})
        mute = next(r for r in rows if r['case']=='D10')
        self.assertIn('policy-maintained', mute['retained_claim'])

    def test_candidate_links_have_distinct_evidence_types(self):
        files = build_a5_corrections.render()
        rows = list(csv.DictReader(io.StringIO(files['a5-typed-candidate-edges.csv'])))
        source = build_a5_corrections.read_csv(build_a5_corrections.A5 / 'candidates.csv', ';')
        expected = {(c['id'], g) for c in source for g in c['regimes'].split()}
        self.assertEqual({(r['candidate'], r['regime']) for r in rows}, expected)
        self.assertTrue(all(r['preservation_status']=='not-observed-preservation' for r in rows))
        self.assertIn('hazard-motivation', {r['relation'] for r in rows})
        self.assertIn('unmodelled-prerequisite', {r['relation'] for r in rows})

    def test_audit_dispositions_reproduce(self):
        for name, text in build_a5_corrections.render().items():
            self.assertEqual((aggregate.ROOT / 'audit-derived' / name).read_text(), text)

    def test_reviewed_state_index_preserves_all_authors(self):
        tables, _, _ = aggregate.model()
        files = reviewed_index.render()
        rows = list(csv.DictReader(io.StringIO(files['reviewed-state-index.csv'])))
        self.assertEqual({r['state_id'] for r in rows}, {r['state_id'] for r in tables['states.csv']})
        self.assertEqual(len(rows), len(tables['states.csv']))
        self.assertTrue(all(r['status']=='conditional-author-description-with-review-limits' for r in rows))
        for name, text in files.items():
            self.assertEqual((aggregate.ROOT / 'reviewed' / name).read_text(), text)

    def test_all_cross_findings_have_explicit_synthesis_dispositions(self):
        findings = reviewed_index.load(aggregate.PROJECT / 'reviews/skeptic/cross-findings.csv')
        overlays = reviewed_index.load(aggregate.ROOT / 'state-qualifications.csv')
        branches = reviewed_index.load(aggregate.ROOT / 'supplemental-branches.csv')
        addressed = {f for r in overlays+branches for f in re.findall(r'CF\d+',r['review_basis'])}
        self.assertEqual(addressed, {r['id'] for r in findings})
        self.assertEqual(len({r['branch_id'] for r in branches}), len(branches))

    def test_alignment_relations_are_not_automatic_merges(self):
        rows = reviewed_index.load(aggregate.PROJECT / 'reviews/skeptic/alignment.csv')
        allowed = {'merge-candidate','keep-separate','disagreement','not-comparable'}
        self.assertTrue(all(r['relationship'] in allowed for r in rows))
        self.assertIn('disagreement', {r['relationship'] for r in rows})
        self.assertTrue(all(r['required_distinction'] for r in rows))

    def german_attractor_sections(self):
        text = (aggregate.ROOT / 'attraktoren-residues-architektur-einfach.md').read_text()
        headings = list(re.finditer(r'^## \d+\. ([A-Z]+\d+) — .+$', text, re.MULTILINE))
        sections = {}
        for i, heading in enumerate(headings):
            end = headings[i+1].start() if i+1 < len(headings) else text.index('\n## Was diese Herleitungen gemeinsam bedeuten')
            self.assertNotIn(heading[1], sections)
            sections[heading[1]] = text[heading.end():end]
        return text, sections

    def test_german_document_covers_exactly_the_author_attractor_hypotheses(self):
        tables, _, _ = aggregate.model()
        text, sections = self.german_attractor_sections()
        expected = {s['state_id'] for s in tables['states.csv'] if s['kind']=='attractor hypothesis'}
        self.assertEqual(set(sections), expected)
        for sid, section in sections.items():
            self.assertIn(f'<a id="{sid.lower()}"></a>', text)
            self.assertIn('### Wie entsteht der Zustand?', section)
            self.assertIn('### Was bleibt, und was ist der Residue-Vorschlag?', section)
            self.assertIn('### Welche Architekturänderungen folgen daraus?', section)
            self.assertIn('**Grenze und Prüffrage:**', section)
        self.assertIn('keine nachgewiesenen Factory-Attraktoren', text)

    def test_german_document_stressors_match_trajectory_destinations(self):
        tables, _, _ = aggregate.model()
        _, sections = self.german_attractor_sections()
        for sid, section in sections.items():
            expected = {stressor for t in tables['trajectories.csv']
                        if sid in t['destination_states'].split() for stressor in t['stressors'].split()}
            listed = re.findall(r'^- \*\*(S\d+):\*\*', section, re.MULTILINE)
            self.assertEqual(len(listed), len(set(listed)), sid)
            self.assertEqual(set(listed), expected, sid)

    def test_deterministic_join(self):
        self.assertEqual(aggregate.render(), aggregate.render())
        for name, text in aggregate.render().items():
            self.assertEqual((aggregate.ROOT / 'generated' / name).read_text(), text)


if __name__ == '__main__':
    unittest.main()
