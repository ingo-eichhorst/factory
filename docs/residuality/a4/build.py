#!/usr/bin/env python3
"""Documentation-only A4 derivation. No runtime, network or plugin calls."""
import argparse
import csv
import io
from collections import Counter
from pathlib import Path

ROOT = Path(__file__).resolve().parent
BASE = ROOT.parent
REVISIONS = ('A0', 'A1', 'A2', 'A3', 'A4')
OWNER = 'Generated exclusively by docs/residuality/a4/build.py.\n'
MODES = {'halten', 'degradiert', 'verlust', 'grenze', 'offen'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load(path):
    with path.open(encoding='utf-8', newline='') as f:
        rows = list(csv.DictReader(f, delimiter=';'))
    require(bool(rows), f'Empty table: {path}')
    require(all(None not in r and all(v and v.strip() for v in r.values()) for r in rows),
            f'Malformed/empty fields: {path}')
    if 'id' in rows[0]:
        require(len({r['id'] for r in rows}) == len(rows), f'Duplicate IDs: {path}')
    return rows


def indexed(rows):
    require(len({r['id'] for r in rows}) == len(rows), 'Overlapping baseline/extension IDs')
    return {r['id']: r for r in rows}


def model():
    old = load(BASE / 'stressors.csv')
    new = load(ROOT / 'stressors.csv')
    for r in old:
        r['corpus'], r['lens'] = 'A0-A3', r['perspective']
    for r in new:
        r['corpus'], r['lens'] = 'A4', r['strategy']
    return {
        'stressors': old + new,
        'strategies': indexed(load(ROOT / 'strategies.csv')),
        'controls': indexed(load(BASE / 'controls.csv') + load(ROOT / 'controls.csv')),
        'residues': indexed(load(BASE / 'residues.csv') + load(ROOT / 'residues.csv')),
        'experiments': indexed(load(BASE / 'experiments.csv') + load(ROOT / 'experiments.csv')),
        'incidents': indexed(load(BASE / 'incidents.csv') + load(ROOT / 'incidents.csv')),
        'dependencies': load(BASE / 'residue-dependencies.csv') + load(ROOT / 'residue-dependencies.csv'),
    }


def validate(m):
    s, q, c, r, t = (m[k] for k in ('stressors', 'strategies', 'controls', 'residues', 'experiments'))
    require([x['id'] for x in s] == [f'S{i:03}' for i in range(1, 351)], 'Expected S001-S350')
    require(len({x['stressor'].casefold() for x in s}) == 350, 'Duplicate stressor text')
    for values, prefix, count in [(q, 'Q', 50), (c, 'C', 48), (r, 'R', 17), (t, 'T', 29)]:
        require(set(values) == {f'{prefix}{i:02}' for i in range(1, count + 1)}, f'Expected {prefix} IDs')
    for number, row in enumerate(s, 1):
        expected = f'P{(number - 1) // 10 + 1:02}' if number <= 200 else f'Q{(number - 201) // 3 + 1:02}'
        require(row['lens'] == expected, f'Wrong strategy/perspective: {row["id"]}')
        required = row['required_controls'].split()
        require(bool(required) and len(required) == len(set(required)), 'Empty/duplicate controls')
        require(set(required) <= c.keys(), f'Unknown control: {row["id"]}')
        require(row['response_mode'] in MODES, 'Unknown response mode')
    for row in c.values():
        require(row['residue'] in r and row['test'] in t, 'Unknown residue/experiment')
        require(row['introduced'] in (*REVISIONS, 'offen'), 'Unknown revision')
    require(all(c[f'C{i:02}']['introduced'] == 'A4' for i in range(37, 49)),
            'New obligations belong to A4, not a rewritten baseline')
    require({x['id'] for x in c.values() if x['introduced'] == 'offen'} ==
            {'C33', 'C34', 'C35', 'C36'}, 'Do not silently resolve open baseline obligations')
    require(all(x['execution_status'] == 'vorgeschlagen' for x in t.values()),
            'Experiments are not executed by this documentation generator')
    require({c[key]['residue'] for row in s for key in row['required_controls'].split()} == r.keys(),
            'Unused residue')
    for dep in m['dependencies']:
        require(dep['residue'] in r and dep['requires'] in r, 'Unknown dependency residue')
    for key, incident in m['incidents'].items():
        # Baseline incidents call the sequence column ordered_stressors.
        sequence = incident.get('sequence', incident.get('ordered_stressors', ''))
        require(bool(sequence) and set(sequence.split()) <= {x['id'] for x in s}, f'Bad incident: {key}')
        if 'architectural_response' in incident:
            require(set(incident['architectural_response'].split()) <= c.keys(), 'Bad incident controls')
        if 'tests' in incident:
            require(set(incident['tests'].split()) <= t.keys(), 'Bad incident tests')
    for row in r.values():
        require(set(row['strengthening'].split()) <= c.keys(), 'Unknown strengthening control')
        require(all(c[key]['residue'] == row['id'] for key in row['strengthening'].split()),
                'Wrong strengthening residue')


def csv_text(headers, rows):
    out = io.StringIO(newline='')
    writer = csv.writer(out, lineterminator='\n')
    writer.writerow(headers)
    writer.writerows(rows)
    return out.getvalue()


def assess(required, controls, revision):
    named = sorted(k for k in required if controls[k]['introduced'] != 'offen' and
                   REVISIONS.index(controls[k]['introduced']) <= REVISIONS.index(revision))
    missing = sorted(set(required) - set(named))
    return ('benannt' if not missing else 'teilweise' if named else 'fehlend'), named, missing


def render(m):
    s, q, c, r = (m[k] for k in ('stressors', 'strategies', 'controls', 'residues'))
    memberships = {x['id']: {c[k]['residue'] for k in x['required_controls'].split()} for x in s}
    rids = sorted(r)
    files = {'OWNERSHIP.txt': OWNER}
    files['incidence-matrix.csv'] = csv_text(['stressor', *rids],
        [[x['id'], *[int(key in memberships[x['id']]) for key in rids]] for x in s])
    files['traceability.csv'] = csv_text(
        ['stressor', 'corpus', 'lens', 'residue', 'control', 'introduced', 'experiment'],
        [[x['id'], x['corpus'], x['lens'], c[k]['residue'], k, c[k]['introduced'], c[k]['test']]
         for x in s for k in x['required_controls'].split()])
    assessments, counts = [], {}
    for rev in REVISIONS:
        counts[rev] = Counter()
        for x in s:
            status, named, missing = assess(x['required_controls'].split(), c, rev)
            counts[rev][status] += 1
            assessments.append([x['id'], rev, status, ' '.join(named), ' '.join(missing),
                                x['response_mode'], x['remaining_limit']])
    files['architecture-assessments.csv'] = csv_text(
        ['stressor', 'revision', 'obligations', 'named_controls', 'missing_controls',
         'target_response_mode', 'remaining_limit'], assessments)
    strategies = ['<!-- ' + OWNER.strip() + ' -->', '# 50 Suchstrategien', '',
                  'Drei kuratierte Gegenproben pro Strategie. Keine Zufallsstichprobe oder Normzertifizierung.', '',
                  '| ID | Qualitätsziel | Suchbewegung | Leitfrage | Stressoren | Quellen |',
                  '|---|---|---|---|---|---|']
    catalogue = ['<!-- ' + OWNER.strip() + ' -->', '# 150 zusätzliche Stressoren', '',
                 'Die verbleibende Fähigkeit ist eine Zielhypothese, keine gemessene Eigenschaft.',
                 'C01–C36: A3-Baseline. C37–C48: A4-Vertragspräzisierungen. C33–C36 bleiben offen.', '']
    for key, strategy in q.items():
        rows = [x for x in s if x['lens'] == key]
        strategies.append('| ' + ' | '.join([key, strategy['quality'], strategy['strategy'], strategy['question'],
            ' '.join(x['id'] for x in rows), strategy['source_basis']]) + ' |')
        catalogue += [f'## {key} — {strategy["strategy"]}', '',
                      f'**Qualitätsziel:** {strategy["quality"]}. {strategy["question"]}?', '',
                      '| ID / Stressor | Warum problematisch | Was bleiben soll | Residues / Regeln | Grenze |',
                      '|---|---|---|---|---|']
        for x in rows:
            catalogue.append('| ' + ' | '.join([x['id'] + ': ' + x['stressor'], x['broken_assumption'],
                x['surviving_capability'], ' '.join(sorted(memberships[x['id']])) + ' / ' + x['required_controls'],
                x['response_mode'] + ': ' + x['remaining_limit']]) + ' |')
        catalogue.append('')
    files['strategies.md'] = '\n'.join(strategies) + '\n'
    files['stressors.md'] = '\n'.join(catalogue) + '\n'
    summary = ['<!-- ' + OWNER.strip() + ' -->', '# A4: gemeinsame Auswertung', '',
        f'{len(s)} Stressoren (200 historisch + 150 neu), 50 neue Suchstrategien, {len(r)} Residues.',
        f'{sum(map(len, memberships.values()))} direkte Inzidenzen; 1750 Revisionsbewertungen.', '',
        '**Nur Benennung zugeordneter Entwurfsregeln, keine Erfolgsquote oder unabhängige Validierung.**',
        'Alle Revisionen werden hier auf denselben 350 Karten bewertet. Die alte 200er-Auswertung bleibt unverändert.',
        'Der additive Bewertungsmechanismus steigt konstruktionsbedingt. Er misst keine tatsächliche Verbesserung.', '',
        '| Revision | Alle Regeln benannt | Teilweise | Keine |', '|---|---:|---:|---:|']
    for rev, count in counts.items():
        summary.append(f'| {rev} | {count["benannt"]} | {count["teilweise"]} | {count["fehlend"]} |')
    summary += ['', '## Direkte Residue-Zuordnung', '', '| Residue | Baseline | Neu |', '|---|---:|---:|']
    for key in rids:
        old = sum(key in memberships[x['id']] for x in s[:200])
        new = sum(key in memberships[x['id']] for x in s[200:])
        summary.append(f'| {key} — {r[key]["name"]} | {old} | {new} |')
    summary += ['', '## Weiter offene Verpflichtungen auf dem Gesamtkorpus', '']
    for x in s:
        _, _, missing = assess(x['required_controls'].split(), c, 'A4')
        if missing:
            summary.append(f'- {x["id"]}: {" ".join(missing)} — {x["remaining_limit"]}')
    summary += ['', 'Auch bei vollständig benannten Regeln kann der Zielmodus Verlust oder Nicht-Unterstützung sein.',
                'S345 ist die explizite Grenze: Sind alle Beobachter und Empfänger weg, kommt kein Alarm.', '']
    files['summary.md'] = '\n'.join(summary)
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    m = model()
    validate(m)
    outputs = render(m)
    target = ROOT / 'generated'
    if args.check:
        stale = [name for name, text in outputs.items()
                 if not (target / name).exists() or (target / name).read_text(encoding='utf-8') != text]
        require(not stale, 'Missing/stale derived files: ' + ', '.join(stale))
    else:
        require(not target.exists() or ((target / 'OWNERSHIP.txt').exists() and
                (target / 'OWNERSHIP.txt').read_text(encoding='utf-8') == OWNER),
                'Refusing regeneration without matching ownership marker')
        target.mkdir(exist_ok=True)
        for name, text in outputs.items():
            (target / name).write_text(text, encoding='utf-8')
    print(f'350 stressors; 50 strategies; 17 residues; {len(outputs)} derived files ' +
          ('checked' if args.check else 'written'))


if __name__ == '__main__':
    main()
