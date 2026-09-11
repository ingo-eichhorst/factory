"""Tests of the documentation model, NOT tests of Factory resilience."""
import csv
import io
import unittest
from collections import Counter
from copy import deepcopy

import build_matrices as model


class MatrixTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.stressors = model.load("stressors.csv")
        cls.controls = {r["id"]: r for r in model.load("controls.csv")}
        cls.residues = {r["id"]: r for r in model.load("residues.csv")}
        cls.perspectives = {r["id"]: r for r in model.load("perspectives.csv")}
        cls.files = model.render(cls.stressors, cls.controls, cls.residues, cls.perspectives, "A3")

    def test_corpus_ids_counts_membership_and_nonempty_fields(self):
        model.validate(self.stressors, self.controls, self.residues, self.perspectives, 200)
        self.assertEqual(Counter(s["perspective"] for s in self.stressors),
                         {f"P{i:02}": 10 for i in range(1, 21)})

    def test_unknown_control_is_rejected(self):
        bad = deepcopy(self.stressors)
        bad[0]["required_controls"] += " C99"
        with self.assertRaises(AssertionError):
            model.validate(bad, self.controls, self.residues, self.perspectives, 200)

    def test_duplicate_stressor_is_rejected(self):
        bad = deepcopy(self.stressors)
        bad[1]["stressor"] = bad[0]["stressor"]
        with self.assertRaises(AssertionError):
            model.validate(bad, self.controls, self.residues, self.perspectives, 200)

    def test_incidence_is_binary_and_matches_explicit_mapping(self):
        rows = list(csv.DictReader(io.StringIO(self.files["incidence-matrix.csv"])))
        self.assertEqual(len(rows), 200)
        self.assertEqual(sum(int(row[r]) for row in rows for r in self.residues), 443)
        for source, derived in zip(self.stressors, rows):
            expected = {self.controls[c]["residue"] for c in source["required_controls"].split()}
            self.assertEqual(derived["stressor"], source["id"])
            self.assertEqual({r for r in self.residues if derived[r] == "1"}, expected)
            self.assertTrue(all(derived[r] in {"0", "1"} for r in self.residues))

    def test_overlap_is_symmetric_and_diagonal_equals_incidence(self):
        rows = {r["residue"]: r for r in csv.DictReader(io.StringIO(self.files["residue-overlap.csv"]))}
        incidence = list(csv.DictReader(io.StringIO(self.files["incidence-matrix.csv"])))
        for a in self.residues:
            self.assertEqual(int(rows[a][a]), sum(int(row[a]) for row in incidence))
            for b in self.residues:
                self.assertEqual(rows[a][b], rows[b][a])

    def test_revision_gate_and_open_obligation_do_not_become_success(self):
        self.assertEqual(model.assess(["C01"], self.controls, "A0"), ("fehlend", [], ["C01"]))
        self.assertEqual(model.assess(["C01"], self.controls, "A1"), ("benannt", ["C01"], []))
        self.assertEqual(model.assess(["C01", "C33"], self.controls, "A3"),
                         ("teilweise", ["C01"], ["C33"]))

    def test_removing_answer_exposes_missing_obligation(self):
        changed = deepcopy(self.controls)
        changed["C01"]["introduced"] = "offen"
        self.assertEqual(model.assess(["C01"], changed, "A3")[0], "fehlend")

    def test_loss_is_not_silently_relabelled_as_recovery(self):
        rows = list(csv.DictReader(io.StringIO(self.files["architecture-assessments.csv"])))
        row = next(r for r in rows if r["stressor"] == "S190" and r["revision"] == "A3")
        self.assertEqual(row["obligations"], "benannt")
        self.assertEqual(row["response_mode"], "verlust")
        self.assertIn("informationstheoretische", row["remaining_limit"])
        self.assertEqual(len(rows), 800)

    def test_recorded_iteration_counts_are_reproducible(self):
        for size, rev, expected in [
                (160, "A0", (9, 70, 81)), (160, "A1", (41, 94, 25)),
                (180, "A1", (41, 111, 28)), (180, "A2", (117, 60, 3)),
                (200, "A0", (9, 89, 102)), (200, "A1", (41, 123, 36)),
                (200, "A2", (120, 76, 4)), (200, "A3", (178, 22, 0))]:
            with self.subTest(size=size, revision=rev):
                counts = Counter(model.assess(s["required_controls"].split(), self.controls, rev)[0]
                                 for s in self.stressors[:size])
                self.assertEqual(tuple(counts[k] for k in ("benannt", "teilweise", "fehlend")), expected)

    def test_references_in_incidents_experiments_and_dependencies(self):
        tests = {r["id"] for r in model.load("experiments.csv")}
        self.assertEqual(tests, {f"T{i:02}" for i in range(1, 18)})
        self.assertTrue(all(c["test"] in tests for c in self.controls.values()))
        stressors = {s["id"] for s in self.stressors}
        incidents = model.load("incidents.csv")
        self.assertEqual(len(incidents), 12)
        for incident in incidents:
            self.assertTrue(set(incident["ordered_stressors"].split()) <= stressors)
            self.assertTrue(set(incident["tests"].split()) <= tests)
        with (model.ROOT / "residue-dependencies.csv").open(encoding="utf-8", newline="") as handle:
            dependencies = list(csv.DictReader(handle, delimiter=";"))
        self.assertEqual(len(dependencies), 26)
        for row in dependencies:
            self.assertIn(row["residue"], self.residues)
            self.assertIn(row["requires"], self.residues)
            self.assertNotEqual(row["residue"], row["requires"])
            self.assertTrue(all(row.values()))

    def test_deterministic_rendering(self):
        self.assertEqual(self.files, model.render(self.stressors, self.controls, self.residues,
                                                 self.perspectives, "A3"))


if __name__ == "__main__":
    unittest.main()
