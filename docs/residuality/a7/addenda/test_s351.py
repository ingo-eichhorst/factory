#!/usr/bin/env python3
"""Document consistency only; no OS compatibility test."""
import csv
from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT.parent))
import build


class S351Tests(unittest.TestCase):
    def test_seven_explicit_branches_reuse_existing_candidates(self):
        _, candidates, _, _ = build.load_analysis()
        known = set(candidates)
        with (ROOT / 'S351-branches.csv').open(encoding='utf-8', newline='') as source:
            rows = list(csv.DictReader(source, delimiter=';'))
        self.assertEqual([r['branch_id'] for r in rows], [f'S351.B{i:02}' for i in range(1, 8)])
        for r in rows:
            self.assertEqual(r['stressor_id'], 'S351')
            self.assertIn(r['kind'], build.KINDS)
            file, anchor = r['document_anchor'].split('#')
            text = (ROOT / file).read_text()
            section = text.split(f'<a id="{anchor}"></a>', 1)[1].split('<a id=', 1)[0]
            self.assertIn('**Residue: ' + r['residue_ids'], section)
            self.assertIn('**Gegenprobe:**', section)
            for residue in r['residue_ids'].split():
                self.assertTrue(residue == 'keines' or residue in known)
            self.assertEqual(r['review_status'], 'coordinator-proposal-not-independently-reviewed')

    def test_original_review_corpus_is_not_silently_expanded(self):
        cases, _, _, _ = build.load_analysis()
        self.assertEqual({c['stressor_id'] for c in cases}, {f'S{i:03}' for i in range(1, 351)})


if __name__ == '__main__':
    unittest.main()
