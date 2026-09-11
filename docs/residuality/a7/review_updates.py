"""Hash-bound corrections of frozen review inputs, without rewriting the authors."""
import copy
import hashlib
import json


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify_freeze(root, project):
    frozen = json.loads((root/'discovery-freeze.json').read_text())
    for path, digest in frozen['sha256'].items():
        p = project/path
        require(p.is_file() and hashlib.sha256(p.read_bytes()).hexdigest()==digest,
                'A7 frozen source changed: '+path)
    return frozen


def adjustments(root):
    data = json.loads((root/'review-adjustments.json').read_text())
    require(hashlib.sha256((root/'discovery-freeze.json').read_bytes()).hexdigest()==data['base_freeze_sha256'],
            'Correction base freeze changed')
    return data


def apply(root, cases, residues, additional, kinds, statuses):
    data = adjustments(root)
    cases = copy.deepcopy(cases)
    residues = copy.deepcopy(residues)
    for r in additional:
        require(r['id'] not in residues, 'Duplicate added residue')
        residues[r['id']] = dict(r, reviewer='coordinator')
    by_case = {c['stressor_id']:c for c in cases}
    seen = set()
    for p in data['patches']:
        bid = p['branch_id']; sid, position = bid.split('.B')
        require(bid not in seen, 'Duplicate correction target')
        seen.add(bid)
        branch = by_case[sid]['branches'][int(position)-1]
        require(all(branch.get(k)==v for k,v in p['expected'].items()), 'Correction original mismatch: '+bid)
        require(set(p['set']) <= set(branch), 'Unknown correction field')
        branch.update(p['set'])
    for a in data['appends']:
        branches = by_case[a['stressor_id']]['branches']
        require(len(branches)==a['after_count'], 'Append would rename an existing branch')
        branches.extend(copy.deepcopy(a['branches']))
    used = set()
    for c in cases:
        for b in c['branches']:
            require(b['kind'] in kinds and b['residue_status'] in statuses, 'Invalid reviewed classification')
            require(set(b['residue_ids']) <= residues.keys(), 'Unknown reviewed residue')
            require(b['residue_ids'] or b['residue_status'] in {'keines','ungeklaert'}, 'Unexplained reviewed absence')
            require(not b['residue_ids'] or b['residue_status']!='keines', 'Contradictory reviewed absence')
            used.update(b['residue_ids'])
    require(used==residues.keys(), 'Reviewed residue became unreferenced')
    return cases,residues


def references(root):
    data = adjustments(root)
    refs = {p['branch_id']:p['finding'] for p in data['patches']}
    for a in data['appends']:
        for i,_ in enumerate(a['branches'],a['after_count']+1):
            refs[f"{a['stressor_id']}.B{i:02}"] = a['finding']
    return refs


def validate_dependencies(modules, rows):
    graph = {m['id']:set() for m in modules}
    for r in rows:
        require(r['source'] in graph and r['target'] in graph, 'Unknown dependency module')
        require(r['phase'] in {'bootstrap','command','optional'}, 'Unknown dependency phase')
        require(r['contract'].strip() and r['failure_response'].strip(), 'Empty dependency contract')
        if r['phase']=='bootstrap':
            graph[r['source']].add(r['target'])
    while graph:
        ready = {m for m,deps in graph.items() if not deps & graph.keys()}
        require(ready, 'Bootstrap dependency cycle')
        graph = {m:deps for m,deps in graph.items() if m not in ready}
