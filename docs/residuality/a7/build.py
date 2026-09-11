#!/usr/bin/env python3
"""A7 documentation compiler: preserves branches, never discovers or proves residues."""
import argparse
from collections import Counter
import csv
import hashlib
import io
import json
from pathlib import Path
import review_updates

ROOT = Path(__file__).resolve().parent
PROJECT = ROOT.parents[2]
ROLES = {'operations': (1, 100, 'OPR'), 'governance': (101, 200, 'GVR'),
         'observability': (201, 275, 'OBR'), 'boundaries': (276, 350, 'BDR')}
KINDS = {'attraktorhypothese', 'transient', 'extern-erzwungen', 'halt', 'ungewissheit',
         'abschluss', 'verlust', 'eskalation', 'offen', 'konflikt'}
STATUSES = {'bedingt', 'teilweise', 'keines', 'ungeklaert'}
RESIDUE_FIELDS = 'id name object survival prerequisites failure_boundary module_hint architectural_change validation'.split()
BRANCH_FIELDS = 'state kind conditions persistence residue_ids residue_status reason validation'.split()
OWNER = 'Generated exclusively by docs/residuality/a7/build.py. Not evidence of product resilience.\n'


def read_csv(path, delimiter=';'):
    with path.open(encoding='utf-8', newline='') as f:
        rows = list(csv.DictReader(f, delimiter=delimiter))
    if not rows or any(None in r or any(v is None for v in r.values()) for r in rows):
        raise ValueError(f'Malformed/empty CSV: {path}')
    return rows


def require(condition, message):
    if not condition:
        raise ValueError(message)


def load_analysis():
    review_updates.verify_freeze(ROOT, PROJECT)
    cases, residues, texts, hashes = [], {}, {}, {}
    for role, (first, last, prefix) in ROLES.items():
        folder = PROJECT / 'reviews' / role
        inputs = read_csv(folder / 'scenarios.csv')
        expected = {f'S{n:03}' for n in range(first, last+1)}
        require({r['stressor_id'] for r in inputs} == expected, 'Input range changed: '+role)
        texts.update({r['stressor_id']: r['scenario'] for r in inputs})
        states = {r['state_id'] for r in read_csv(folder / 'states.csv')}
        local_residues = read_csv(folder / 'a7/residues.csv')
        local_ids = set()
        for r in local_residues:
            require(set(r) == set(RESIDUE_FIELDS), 'Residue fields: '+role)
            require(all(v.strip() for v in r.values()), 'Empty residue field: '+r['id'])
            require(r['id'].startswith(prefix) and r['id'][len(prefix):].isdigit(), 'Residue prefix: '+r['id'])
            require(r['id'] not in residues, 'Duplicate residue: '+r['id'])
            residues[r['id']] = dict(r, reviewer=role)
            local_ids.add(r['id'])
        local_cases = json.loads((folder / 'a7/cases.json').read_text())
        require(isinstance(local_cases, list), 'Cases must be a list')
        require(len(local_cases) == len(expected) and {c['stressor_id'] for c in local_cases} == expected,
                'Missing/duplicate/extra scenarios: '+role)
        used = set()
        for c in local_cases:
            sid = c['stressor_id']
            require(set(c) == {'stressor_id','source_states','branches','architectural_consequence'}, 'Case fields: '+sid)
            require(isinstance(c['source_states'], list) and set(c['source_states']) <= states, 'Unknown source state: '+sid)
            require(isinstance(c['branches'], list) and c['branches'], 'No branches: '+sid)
            require(isinstance(c['architectural_consequence'], str) and c['architectural_consequence'].strip(), 'No consequence: '+sid)
            for b in c['branches']:
                require(set(b) == set(BRANCH_FIELDS), 'Branch fields: '+sid)
                require(all(isinstance(b[f], str) and b[f].strip() for f in BRANCH_FIELDS if f!='residue_ids'), 'Empty branch field: '+sid)
                require(b['kind'] in KINDS, 'Unknown state kind: '+sid)
                require(b['residue_status'] in STATUSES, 'Unknown residue status: '+sid)
                require(isinstance(b['residue_ids'], list) and set(b['residue_ids']) <= local_ids, 'Unknown residue: '+sid)
                require(len(b['residue_ids']) == len(set(b['residue_ids'])), 'Duplicate branch residue: '+sid)
                require(bool(b['residue_ids']) or b['residue_status'] in {'keines','ungeklaert'}, 'Unexplained missing residue: '+sid)
                require(not b['residue_ids'] or b['residue_status']!='keines', 'No residue cannot assert preservation: '+sid)
                used.update(b['residue_ids'])
            cases.append(dict(c, reviewer=role))
        require(used == local_ids, 'Unused residues: '+str(local_ids-used))
        for name in ('a7/cases.json','a7/residues.csv','a7/report.md','scenarios.csv','states.csv','trajectories.csv','coverage.csv','report.md'):
            p = folder / name
            hashes[str(p.relative_to(PROJECT))] = hashlib.sha256(p.read_bytes()).hexdigest()
    require({c['stressor_id'] for c in cases} == {f'S{n:03}' for n in range(1,351)}, 'Not all 350 scenarios')
    additional = read_csv(ROOT/'additional-residues.csv')
    for r in additional:
        require(set(r)==set(RESIDUE_FIELDS) and all(v.strip() for v in r.values()), 'Added residue schema')
    cases,residues = review_updates.apply(ROOT,cases,residues,additional,KINDS,STATUSES)
    return sorted(cases, key=lambda c:c['stressor_id']), residues, texts, hashes


def load_architecture(residues):
    modules = read_csv(ROOT / 'modules.csv')
    mids = {m['id'] for m in modules}
    require(len(mids) == len(modules), 'Duplicate module')
    bindings = read_csv(ROOT / 'module-bindings.csv', ',')
    require(len(bindings) == len(residues) and {b['residue_id'] for b in bindings} == set(residues), 'Missing/duplicate residue module binding')
    for b in bindings:
        require(b['primary_module'] in mids, 'Unknown primary module')
        support = [] if b['support_modules']=='none' else b['support_modules'].split()
        require(set(support) <= mids and b['primary_module'] not in support, 'Invalid supporting modules')
        require(b['reason'].strip(), 'Unreasoned module assignment')
    return modules, {b['residue_id']:b for b in bindings}


def csv_text(rows, fields):
    out = io.StringIO(newline='')
    w = csv.DictWriter(out, fields, lineterminator='\n')
    w.writeheader()
    w.writerows(rows)
    return out.getvalue()


def render():
    cases, residues, texts, hashes = load_analysis()
    modules, bindings = load_architecture(residues)
    review_updates.validate_dependencies(modules, read_csv(ROOT/'module-dependencies.csv'))
    notes = review_updates.references(ROOT)
    facets = read_csv(ROOT/'contract-facets.csv')
    for f in facets:
        require(f['residue_id'] in residues and f['owner'] in {m['id'] for m in modules}, 'Unknown contract facet')
    modes = read_csv(ROOT/'operation-boundaries.csv')
    require(all(m['module'] in {r['id'] for r in modules} for m in modes), 'Unknown operation boundary module')
    for name in ('modules.csv','module-bindings.csv','module-dependencies.csv','zielarchitektur.md',
                 'review-adjustments.json','additional-residues.csv','contract-facets.csv','operation-boundaries.csv',
                 'review_updates.py','build.py','bind_modules.py','discovery-freeze.json',
                 'addenda/S351-weiteres-betriebssystem.md','addenda/S351-branches.csv',
                 'addenda/test_s351.py','verify.py'):
        hashes[str((ROOT/name).relative_to(PROJECT))] = hashlib.sha256((ROOT/name).read_bytes()).hexdigest()
    rows, coverage, links = [], [], []
    by_residue = {rid:[] for rid in residues}
    for c in cases:
        ids = []
        for n,b in enumerate(c['branches'],1):
            bid = f"{c['stressor_id']}.B{n:02}"
            ids.append(bid)
            rows.append(dict(branch_id=bid, stressor_id=c['stressor_id'], reviewer=c['reviewer'],
                             review_findings=notes.get(bid,'none'),
                             interpretation_author='coordinator-review-amendment' if bid in notes else c['reviewer'],
                             source_states=' '.join(c['source_states']) or 'none',
                             **{k:(' '.join(v) or 'none') if k=='residue_ids' else v for k,v in b.items()},
                             architectural_consequence=c['architectural_consequence']))
            if not b['residue_ids']:
                links.append(dict(branch_id=bid, stressor_id=c['stressor_id'], residue_id='none',
                                  primary_module='none', residue_status=b['residue_status'], reason=b['reason']))
            for rid in b['residue_ids']:
                by_residue[rid].append(bid)
                links.append(dict(branch_id=bid, stressor_id=c['stressor_id'], residue_id=rid,
                                  primary_module=bindings[rid]['primary_module'], residue_status=b['residue_status'], reason=b['reason']))
        coverage.append(dict(stressor_id=c['stressor_id'], scenario=texts[c['stressor_id']], reviewer=c['reviewer'],
                             branch_ids=' '.join(ids), residue_ids=' '.join(sorted({r for b in c['branches'] for r in b['residue_ids']})) or 'none',
                             dispositions=' '.join(sorted({b['residue_status'] for b in c['branches']})),
                             architectural_consequence=c['architectural_consequence']))
    lines = ['# Alle 350 Stressoren: Zustände, Residues und Architektur', '',
             '**Bedingte Entwurfsanalyse, keine beobachteten Attraktoren oder bestandenen Resilienztests.**', '',
             'Jeder Zweig gilt nur unter seinen Bedingungen. `keines` bezeichnet ausdrücklich keine nutzbare Restfähigkeit für den betrachteten Gegenstand; `ungeklaert` eine nicht begründbare Zuordnung. Ein haltender oder geschädigter Zustand ist nicht automatisch ein Attraktor.', '',
             'Die Voraussetzungen der referenzierten Residues gelten zusätzlich zu den Zweigbedingungen. Anders vorbereitete Schutzentwürfe sind keine Überlebensbehauptung nach wörtlichem Totalverlust. Hashgebundene Nachträge des Koordinators sind bei den betroffenen Zweigen sichtbar.', '',
             'Die Zweig-IDs sind A7-Analysekennungen, keine Factory-Laufzeitzustände. Mehrere Fähigkeiten können gleichzeitig nötig sein. Eine vollständige Kartenzuordnung beweist keine vollständige Menge aller möglichen Verläufe.', '',
             '[Zielarchitektur](../zielarchitektur.md) · [Residue-Katalog](residues.md) · [Reviewauflagen](../review-dispositions.md)', '']
    for c in cases:
        sid = c['stressor_id']
        lines += [f'<a id="{sid.lower()}"></a>', f'## {sid} — {texts[sid]}', '',
                  f"**Ursprung:** {c['reviewer']}; A6-Zustände: {', '.join(c['source_states']) or 'damals offen'}. [Originalverläufe](../../../../reviews/{c['reviewer']}/trajectories.csv)", '']
        for n,b in enumerate(c['branches'],1):
            bid = f'{sid}.B{n:02}'
            refs = ', '.join(f"[{rid}: {residues[rid]['name']}](residues.md#{rid.lower()})" for rid in b['residue_ids']) or '**kein zugewiesenes brauchbares Residue**'
            lines += [f'<a id="{bid.lower()}"></a>', f"### {bid} — {b['state']}", '',
                      f"**Art:** {b['kind']}. **Residue-Status:** {b['residue_status']}.", '',
                      '**Voraussetzungen:** '+b['conditions'], '', '**Warum bleibt oder endet der Zustand?** '+b['persistence'], '',
                      '**Zugeordnete Residues:** '+refs, '', '**Was bleibt warum nutzbar?** '+b['reason'], '',
                      '**Zu prüfen:** '+b['validation'], '']
            if bid in notes:
                ref=notes[bid]
                lines += [f'**Nachtrag des Koordinators:** [{ref}](../review-dispositions.md#{ref.lower()}); ursprüngliche Abgabe unverändert.', '']
        lines += ['**Architekturfolge für diesen Stressor:** '+c['architectural_consequence'], '']
    catalogue = ['# Residue-Katalog A7', '',
                 '**Konkrete bedingte Kandidaten, keine nachgewiesenen Produktfähigkeiten.** Die IDs erhalten die unabhängige Herkunft; ähnliche Einträge sind nicht automatisch verschiedene globale Residues. Modulzugehörigkeit ist keine Gleichsetzung ihrer Schutzverträge.', '',
                 '[Zielarchitektur](../zielarchitektur.md) · [Reviewauflagen](../review-dispositions.md)', '']
    for rid,r in sorted(residues.items()):
        b = bindings[rid]
        catalogue += [f'<a id="{rid.lower()}"></a>', f"## {rid} — {r['name']}", '',
                      '**Herleitung:** '+r['reviewer'], '',
                      '**Erhaltener Gegenstand:** '+r['object'], '', '**Verbleibende Fähigkeit:** '+r['survival'], '',
                      '**Notwendige Voraussetzungen:** '+r['prerequisites'], '', '**Grenze:** '+r['failure_boundary'], '',
                      f"**Verantwortung:** {b['primary_module']}; Mitwirkung: {b['support_modules']}. {b['reason']}", '',
                      '**Architekturänderung:** '+r['architectural_change'], '', '**Nachweis:** '+r['validation'], '',
                      '**Zugeordnete Fälle/Zweige:** '+', '.join(f'[{bid}](stressoren.md#{bid.lower()})' for bid in by_residue[rid]), '']
        for f in facets:
            if f['residue_id']==rid:
                catalogue += [f"**Getrennter Teilvertrag: {f['facet']} ({f['owner']}).** {f['runtime_dependency']} Bei Ausfall: {f['failure_result']} Prüffall: {f['validation']}", '']
    kinds = Counter(r['kind'] for r in rows)
    dispositions = Counter(r['residue_status'] for r in rows)
    summary = ['# A7 Zuordnungsstand', '', f'- Stressoren: **{len(cases)}**',
               f'- Bedingte Zweige: **{len(rows)}** (keine Zahl verschiedener Attraktoren)',
               f'- Herkunftserhaltende Residue-Kandidaten: **{len(residues)}** (keine bewiesene minimale globale Menge)',
               f'- Modulverantwortungen: **{len(modules)}** (keine Zahl zusätzlich zu startender Dienste)', '',
               '## Zweigarten', '', *[f'- {k}: {v}' for k,v in sorted(kinds.items())], '',
               '## Residue-Dispositionen pro Zweig', '', *[f'- {k}: {v}' for k,v in sorted(dispositions.items())], '',
               '**Abdeckung ist hier eine explizite bedingte Zuordnung, nicht Wirksamkeit.**', '']
    module_lines = ['# Module und die ihnen zugeordneten Residues', '', '[Verträge und Zusammenspiel](../zielarchitektur.md)', '']
    for m in modules:
        module_lines += [f"## {m['id']} — {m['name']}", '', m['responsibility'], '',
                         '**Primär verantwortlich für:** '+', '.join(f"[{rid}](residues.md#{rid.lower()})" for rid,b in bindings.items() if b['primary_module']==m['id']), '']
    return {'OWNERSHIP.txt':OWNER, 'coverage.csv':csv_text(coverage,list(coverage[0])),
            'branches.csv':csv_text(rows,list(rows[0])), 'traceability.csv':csv_text(links,list(links[0])),
            'stressoren.md':'\n'.join(lines), 'residues.md':'\n'.join(catalogue),
            'modules.md':'\n'.join(module_lines), 'summary.md':'\n'.join(summary),
            'source-hashes.json':json.dumps(hashes,ensure_ascii=False,indent=2,sort_keys=True)+'\n'}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--check', action='store_true')
    p.add_argument('--inventory-only', action='store_true')
    args = p.parse_args()
    if args.inventory_only:
        cases,residues,_,_ = load_analysis()
        print(f'{len(cases)} scenarios, {sum(len(c["branches"]) for c in cases)} branches, {len(residues)} candidates; no semantics proven')
        return
    files = render()
    out = ROOT / 'generated'
    if args.check:
        stale = [n for n,t in files.items() if not (out/n).exists() or (out/n).read_text()!=t]
        require(not stale, 'Stale generated files: '+', '.join(stale))
    else:
        require(not out.exists() or ((out/'OWNERSHIP.txt').exists() and (out/'OWNERSHIP.txt').read_text()==OWNER), 'Refusing unowned output')
        out.mkdir(exist_ok=True)
        for n,t in files.items():
            (out/n).write_text(t,encoding='utf-8')
    print('A7 Freeze, Zuordnungen, Abhängigkeiten und erzeugte Dateien geprüft; kein Wirksamkeitsnachweis' if args.check else 'A7 artifacts generated')


if __name__ == '__main__':
    main()
