#!/usr/bin/env python3
"""Derive documentation tables only; never invoke Factory or change runtime state.

Input CSVs use semicolons, UTF-8, and space-separated control IDs.
Outputs use ordinary comma-delimited CSV. See methodology.md for semantics.
"""
import argparse
import csv
import io
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent
REVISIONS = ("A0", "A1", "A2", "A3")
MODES = {"halten", "degradiert", "grenze", "verlust", "offen"}
OWNER = "Generated exclusively by docs/residuality/build_matrices.py.\n"


def load(name):
    with (ROOT / name).open(encoding="utf-8", newline="") as handle:
        rows = list(csv.DictReader(handle, delimiter=";"))
    if any(None in row or any(value is None or not value.strip() for value in row.values()) for row in rows):
        raise ValueError(f"Malformed or empty field in {name}")
    ids = [row["id"] for row in rows]
    if len(ids) != len(set(ids)):
        raise ValueError(f"Duplicate ID in {name}")
    return rows


def assess(required, controls, revision):
    available = {key for key, row in controls.items()
                 if row["introduced"] in REVISIONS
                 and REVISIONS.index(row["introduced"]) <= REVISIONS.index(revision)}
    named, missing = sorted(set(required) & available), sorted(set(required) - available)
    status = "benannt" if not missing else ("teilweise" if named else "fehlend")
    return status, named, missing


def as_csv(headers, rows):
    output = io.StringIO(newline="")
    writer = csv.writer(output, lineterminator="\n")
    writer.writerow(headers)
    writer.writerows(rows)
    return output.getvalue()


def validate(stressors, controls, residues, perspectives, expected):
    assert len(stressors) == expected, (len(stressors), expected)
    assert [row["id"] for row in stressors] == [f"S{i:03}" for i in range(1, expected + 1)]
    assert len({row["stressor"].casefold() for row in stressors}) == expected
    assert set(controls) == {f"C{i:02}" for i in range(1, 37)}
    assert set(residues) == {f"R{i:02}" for i in range(1, 17)}
    assert set(perspectives) == {f"P{i:02}" for i in range(1, 21)}
    for number, row in enumerate(stressors, 1):
        assert row["perspective"] == f"P{(number - 1) // 10 + 1:02}"
        required = row["required_controls"].split()
        assert required and len(required) == len(set(required))
        assert set(required) <= controls.keys(), row["id"]
        assert row["response_mode"] in MODES
    for row in controls.values():
        assert row["introduced"] in (*REVISIONS, "offen")
        assert row["residue"] in residues
        assert row["test"] in {f"T{i:02}" for i in range(1, 18)}
    for key, row in residues.items():
        assert set(row["strengthening"].split()) == {
            c for c, control in controls.items() if control["residue"] == key}
    # Rarity is not proof of dispensability, but an unused residue is a modelling error.
    used = {controls[c]["residue"] for s in stressors for c in s["required_controls"].split()}
    assert used == residues.keys()


def render(stressors, controls, residues, perspectives, revision):
    rids = sorted(residues)
    memberships = {s["id"]: {controls[c]["residue"] for c in s["required_controls"].split()}
                   for s in stressors}
    files = {}
    files["incidence-matrix.csv"] = as_csv(
        ["stressor", *rids], [[s["id"], *[int(r in memberships[s["id"]]) for r in rids]] for s in stressors])
    files["residue-overlap.csv"] = as_csv(
        ["residue", *rids], [[a, *[sum(a in m and b in m for m in memberships.values())
                                    for b in rids]] for a in rids])
    assessments, counts = [], {}
    for rev in REVISIONS[:REVISIONS.index(revision) + 1]:
        counts[rev] = Counter()
        for s in stressors:
            status, named, missing = assess(s["required_controls"].split(), controls, rev)
            counts[rev][status] += 1
            assessments.append([s["id"], rev, status, " ".join(named), " ".join(missing),
                                s["response_mode"], s["remaining_limit"]])
    files["architecture-assessments.csv"] = as_csv(
        ["stressor", "revision", "obligations", "named_controls", "missing_controls",
         "response_mode", "remaining_limit"], assessments)
    trace_rows = []
    for s in stressors:
        for c in s["required_controls"].split():
            control = controls[c]
            trace_rows.append([s["id"], control["residue"], c, control["introduced"], control["test"]])
    files["traceability.csv"] = as_csv(
        ["stressor", "residue", "control", "introduced", "test"], trace_rows)
    lines = ["<!-- " + OWNER.strip() + " -->", "# Matrixauswertung", "",
             "**Nur syntaktische Entwurfsobligationen; keine bestandenen Resilienztests.**", "",
             f"Korpus: {len(stressors)} Stressoren; {len(residues)} Residues; "
             f"{sum(map(len, memberships.values()))} direkte Inzidenzen.", "",
             "| Revision | Alle zugeordneten Obligationen benannt | Teilweise | Keine |",
             "|---|---:|---:|---:|"]
    for rev, count in counts.items():
        lines.append(f"| {rev} | {count['benannt']} | {count['teilweise']} | {count['fehlend']} |")
    lines += ["", "`benannt` bedeutet nur: Die zugewiesenen Vertragsantworten existieren im Entwurf.",
              "Es bedeutet weder beherrscht noch implementiert. `offen` eingeführte Kontrollen bleiben fehlend.",
              "Auch vollständig benannte Szenarien können Verlust oder einen bewussten Betriebsstopp bedeuten.",
              "", "## Restbetriebsarten (unabhängig vom Obligationenstatus)", ""]
    for mode, count in sorted(Counter(s["response_mode"] for s in stressors).items()):
        lines.append(f"- {mode}: {count}")
    lines += ["", "## Inzidenz je Residue", "", "| Residue | Direkte Stressoren |", "|---|---:|"]
    for r in sorted(rids, key=lambda r: (-sum(r in m for m in memberships.values()), r)):
        lines.append(f"| {r} — {residues[r]['name']} | {sum(r in m for m in memberships.values())} |")
    pairs = sorted(((sum(a in m and b in m for m in memberships.values()), a, b)
                    for i, a in enumerate(rids) for b in rids[i + 1:]), reverse=True)
    lines += ["", "## Häufigste gemeinsame Inzidenzen", "",
              "Keine statistische Korrelation und keine gemessene Ausfallabhängigkeit.", ""]
    for count, a, b in pairs[:8]:
        lines.append(f"- {a} / {b}: {count} gemeinsame Stressoren")
    lines += ["", f"## Weiterhin fehlende Obligationen in {revision}", ""]
    for s in stressors:
        _, _, missing = assess(s["required_controls"].split(), controls, revision)
        if missing:
            lines.append(f"- {s['id']}: {' '.join(missing)} — {s['remaining_limit']}")
    files["summary.md"] = "\n".join(lines) + "\n"
    catalogue = ["<!-- " + OWNER.strip() + " -->", "# Stressor-Katalog", "",
                 "Lesesicht von `../stressors.csv`; ausschließlich hypothetische Szenarien.", ""]
    for p, perspective in perspectives.items():
        rows = [s for s in stressors if s["perspective"] == p]
        if not rows:
            continue
        catalogue += [f"## {p} — {perspective['perspective']}", "", perspective["question"], "",
                      "| ID | Stressor | Gebrochene Annahme | Was bleibt | Residues | Modus / Grenze |",
                      "|---|---|---|---|---|---|"]
        for s in rows:
            catalogue.append("| " + " | ".join([s["id"], s["stressor"], s["broken_assumption"],
                s["surviving_capability"], " ".join(sorted(memberships[s["id"]])),
                s["response_mode"] + ": " + s["remaining_limit"]]) + " |")
        catalogue.append("")
    files["stressors.md"] = "\n".join(catalogue) + "\n"
    files["OWNERSHIP.txt"] = OWNER
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--revision", choices=REVISIONS, default="A3")
    parser.add_argument("--expect", type=int, default=200)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    stressors = load("stressors.csv")
    controls = {r["id"]: r for r in load("controls.csv")}
    residues = {r["id"]: r for r in load("residues.csv")}
    perspectives = {r["id"]: r for r in load("perspectives.csv")}
    validate(stressors, controls, residues, perspectives, args.expect)
    files = render(stressors, controls, residues, perspectives, args.revision)
    out = ROOT / "generated"
    if args.check:
        stale = [name for name, text in files.items()
                 if not (out / name).exists() or (out / name).read_text(encoding="utf-8") != text]
        if stale:
            raise SystemExit("Missing or stale generated files: " + ", ".join(stale))
    else:
        if out.exists() and (not (out / "OWNERSHIP.txt").exists()
                             or (out / "OWNERSHIP.txt").read_text(encoding="utf-8") != OWNER):
            raise SystemExit("Refusing to overwrite a directory without this generator's ownership marker")
        out.mkdir(exist_ok=True)
        for name, text in files.items():
            (out / name).write_text(text, encoding="utf-8")
    print(f"{len(stressors)} stressors; {len(residues)} residues; {len(controls)} obligations; "
          f"{len(files)} derived files {'checked' if args.check else 'written'} through {args.revision}")


if __name__ == "__main__":
    main()
