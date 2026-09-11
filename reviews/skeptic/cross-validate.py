#!/usr/bin/env python3
"""Read-only peer audit and cross-artifact checks; writes only beside this file.

Checks are structural, not evidence of dynamics or semantic completeness.
"""
import csv
import hashlib
import json
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SCHEMAS = {
    "scenarios": "stressor_id;scenario",
    "states": "state_id;name;kind;conditions;feedback;entry;exit;survives;lost;falsifier;evidence;source_refs",
    "trajectories": "trajectory_id;stressors;initial_state;sequence;destination_states;conditions;evidence;falsifier",
    "coverage": "stressor_id;state_ids;analysis_status;reason",
}
STATUS = {"conditional", "source-supported", "toy-supported", "unresolved"}
inputs = {}


def fingerprint(path):
    inputs[str(path.relative_to(ROOT.parent))] = hashlib.sha256(path.read_bytes()).hexdigest()


def table(path, schema):
    with path.open(encoding="utf-8", newline="") as stream:
        reader = csv.DictReader(stream, delimiter=";")
        fields = schema.split(";")
        assert reader.fieldnames == fields, (path, reader.fieldnames)
        rows = list(reader)
    assert rows and all(set(row) == set(fields) and all(row.values()) for row in rows), path
    assert len({row[fields[0]] for row in rows}) == len(rows), path
    return rows


all_states = set()
all_scenarios = []
summary = {}
totals = Counter()
for scope, lo, hi in [
    ("operations", 1, 100), ("governance", 101, 200),
    ("observability", 201, 275), ("boundaries", 276, 350),
]:
    directory = ROOT.parent / scope
    data = {}
    for name, schema in SCHEMAS.items():
        path = directory / f"{name}.csv"
        fingerprint(path)
        data[name] = table(path, schema)
    fingerprint(directory / "report.md")
    expected = [f"S{i:03d}" for i in range(lo, hi + 1)]
    assert [r["stressor_id"] for r in data["scenarios"]] == expected
    assert [r["stressor_id"] for r in data["coverage"]] == expected
    assert [r["stressors"] for r in data["trajectories"]] == expected
    states = {r["state_id"] for r in data["states"]}
    assert not states & all_states
    all_states |= states
    all_scenarios.extend(expected)
    used = set()
    for coverage, trajectory in zip(data["coverage"], data["trajectories"]):
        assert coverage["state_ids"] == trajectory["destination_states"]
        refs = set(coverage["state_ids"].split())
        assert refs == {"none"} or refs <= states
        assert coverage["analysis_status"] in STATUS
        assert (coverage["analysis_status"] == "unresolved") == (refs == {"none"})
        used |= refs - {"none"}
    assert used == states
    counts = Counter(r["analysis_status"] for r in data["coverage"])
    totals.update(counts)
    summary[scope] = {
        "dispositions": dict(counts),
        "unresolved": [r["stressor_id"] for r in data["coverage"] if r["analysis_status"] == "unresolved"],
    }
assert all_scenarios == [f"S{i:03d}" for i in range(1, 351)]

for path in [ROOT / "AGENTS.md", ROOT.parent / "REVIEW-PROTOCOL.md", ROOT / "method-audit.md", ROOT / "findings.csv"]:
    fingerprint(path)

findings = table(ROOT / "cross-findings.csv", "id;severity;source_states;issue;recommended_disposition;evidence")
alignment = table(ROOT / "alignment.csv", "alignment_id;source_states;relationship;reason;required_distinction;confidence")
referenced = set()
for row in findings + alignment:
    words = row["source_states"].split(" ")
    assert all(words) and len(words) == len(set(words))
    assert set(words) <= all_states, row
    referenced.update(words)
assert all(row["severity"] in {"low", "medium", "high"} for row in findings)
assert all(row["relationship"] in {"merge-candidate", "keep-separate", "disagreement", "not-comparable"} for row in alignment)
assert all(row["confidence"] in {"low", "medium", "high"} for row in alignment)
artifacts = {
    name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
    for name in ["cross-review.md", "cross-findings.csv", "alignment.csv", "cross-validate.py"]
}
result = {
    "result": "PASS structural checks only, not semantic completeness or dynamics validation",
    "scenario_dispositions": 350,
    "totals": dict(totals),
    "reviewers": summary,
    "cross_artifacts": {"findings": len(findings), "alignments": len(alignment)},
    "states_not_referenced_by_cross_artifacts": sorted(all_states - referenced),
    "input_sha256": inputs,
    "artifact_sha256": artifacts,
}
(ROOT / "cross-validation.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
print(result["result"])
print(json.dumps({"totals": result["totals"], "cross_artifacts": result["cross_artifacts"], "unmatched": result["states_not_referenced_by_cross_artifacts"]}, indent=2))
