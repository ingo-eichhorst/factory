#!/usr/bin/env python3
"""Explicitly disposition A5 outputs after the independent methods audit.

A5 remains historical. These are authored review dispositions, not a new
attractor-discovery algorithm or evidence that a proposed structure survives.
"""
import argparse
import csv
import io
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
A5 = ROOT.parent / 'a5'
OWNER = 'Generated exclusively by docs/residuality/a6/build_a5_corrections.py.\n'
# Relation to positive/held regimes. Harmful regimes override this to hazard-motivation.
PROPOSALS = {
 'N01': ('executed-model-behavior', 'A scalar backlog can remain bounded', 'Task identity fairness bytes durability and usable completion are absent', 'Track old and new job identities under FIFO and adversarial service'),
 'N02': ('executed-model-behavior', 'Backlog drains under changed intake parameters', 'No separate real control path or safe treatment of in-flight effects is represented', 'Remove fresh arrivals and retries independently while preserving operation identities'),
 'N03': ('proposed-intervention', 'Retries contribute to the scalar incoming-work law', 'No cross-layer attempt owner durable budget or opaque SDK is represented', 'Inventory and count every actual attempt producer'),
 'N04': ('executed-model-behavior', 'The phase reaches run with the chosen heat and cooling rules', 'Real resource isolation descendants and viable cold-start service are unmeasured', 'Vary saturation cooling and real startup resource boundaries in an isolated fixture'),
 'N05': ('proposed-intervention', 'Finite attempts can end in a quarantine phase', 'Diagnostic evidence is not captured or retained by the toy state', 'Verify no hidden restart path and read retained diagnostics without the integration'),
 'N06': ('unmodelled-prerequisite', 'The old scalar alarm model can retain a count of one', 'Mute and grouping discard represented backlog and no durable incident identity exists', 'Model distinct incidents with explicit retained acknowledged merged and lost dispositions'),
 'N07': ('proposed-intervention', 'Grouping limits a disposable notification counter', 'Safe grouping of distinct incidents and actual delivery are unmodelled', 'Distinguish notification suppression from retained unresolved incidents'),
 'N08': ('proposed-intervention', 'A timer reopens notification production', 'Ownership independent enforcement and actionable re-entry are unmodelled', 'Withdraw the incident burst then test expiry and recurrence with retained backlog'),
 'N09': ('executed-model-behavior', 'A stipulated check places an artifact truth bit in held phase', 'Actual content contradiction evidence legal retention and durable access are absent', 'Verify retained artifact bytes and independently justified rejection evidence'),
 'N10': ('unmodelled-prerequisite', 'A perfect oracle directly changes the model truth bit', 'Availability adequacy and independence of a real oracle are not established', 'Use independent domain evidence and stop-on-acceptance countermodels'),
 'N11': ('stipulated-prerequisite', 'The participant accepts only the configured writer in the model', 'Real acceptance-boundary enforcement is supplied as an assumption', 'Verify actual participant rejection and competing-writer semantics'),
 'N12': ('proposed-intervention', 'Sender uncertainty and eventual response are represented abstractly', 'No durable journal or independent receipt inventory is modelled', 'Compare indistinguishable accepted and unaccepted histories and remove evidence sources'),
 'N13': ('stipulated-prerequisite', 'Repeated model requests keep effect count at one when memory is assumed retained', 'Memory retention business identity and participant conformance are not established', 'Expire participant history and vary business-equivalent operation keys'),
 'N14': ('executed-model-behavior', 'A recognized restore flag can prevent the next dispatch', 'Historical work-package content persistence and recognition of arbitrary copied snapshots are absent', 'Restore missing-tail snapshots and test independent current evidence before dispatch'),
}
HAZARDS = {'G03','G04','G05','G07','G09','G10','G13','G16','G20'}


def read_csv(path, delimiter):
    with path.open(encoding='utf-8', newline='') as f:
        return list(csv.DictReader(f, delimiter=delimiter))


def csv_text(headers, rows):
    out = io.StringIO(newline='')
    w = csv.writer(out, lineterminator='\n')
    w.writerow(headers)
    w.writerows(rows)
    return out.getvalue()


def render():
    basins = read_csv(A5 / 'generated/basins.csv', ',')
    regimes = read_csv(A5 / 'regimes.csv', ';')
    candidates = read_csv(A5 / 'candidates.csv', ';')
    rows = []
    for b in basins:
        matches = [r['id'] for r in regimes if b['case'] in r['cases'].split() and b['kind'] == r['kind']]
        if b['case'] == 'D06':
            phase = json.loads(b['recurrent_states'])[0][3]
            matches = [r for r in matches if r == ('G06' if phase == 'run' else 'G05')]
        disposition, claim, exclusion = 'qualified', b['meaning'], 'No empirical Factory attractor or preserved architectural structure established'
        if not matches:
            if b['case'] == 'D01' and b['kind'] == 'fixed-without-local-attraction':
                disposition, claim = 'explicitly-excluded-from-residue-evidence', 'Separator fixed state without demonstrated local attraction'
                exclusion = 'Not silently dropped and not merged with the neutral D04 family'
            else:
                disposition, claim = 'unresolved-interpretation', 'No authored regime matches this computed set'
        if b['case'] in {'D10','D11','D12','D13'}:
            claim = 'Disposable notification-counter behavior with reset/grouping; no unresolved-incident preservation established'
            exclusion = 'Do not infer durable incidents or retained human backlog; see SK01'
            if b['case'] == 'D10':
                claim += '; permanent mute is policy-maintained suppression'
        if b['case'] == 'D14':
            claim = 'Confidence reinforces around a fixed artifact while post-acceptance review continues'
            exclusion = 'No attraction of artifact content toward falsehood; see SK06'
        if b['case'] == 'D16':
            claim = 'Stipulated oracle correction plus continued review; work-level completion is an alternative projection'
            exclusion = 'No demonstrated independent real oracle or autonomous productive service'
        if b['case'] in {'D06','D07','D08'}:
            exclusion = 'Ordinal capped pressure and chosen cooling law do not prove physical boundedness; see SK05'
        if b['case'] == 'D04' and b['kind'] == 'fixed-without-local-attraction':
            claim = 'Neutral queue balance under exact equality of fresh arrivals and degraded service'
            exclusion = 'Not the separator behavior of D01; not evidence that recovery is draining'
        if 'G01' in matches:
            claim = 'Empty-backlog projection under this case input; productive ongoing demand is not present in every case'
        rows.append([b['set_id'], b['case'], b['kind'], ' '.join(matches) or 'none', disposition, claim, exclusion])
    edges = []
    for c in candidates:
        if c['id'] not in PROPOSALS:
            raise ValueError('New candidate needs an explicit audited edge disposition: ' + c['id'])
        relation, behavior, missing, test = PROPOSALS[c['id']]
        for regime in c['regimes'].split():
            edges.append([c['id'], regime, 'hazard-motivation' if regime in HAZARDS else relation,
                          behavior, missing, test, 'not-observed-preservation'])
    return {
      'OWNERSHIP.txt': OWNER,
      'a5-set-dispositions.csv': csv_text(['set_id','case','source_kind','source_regimes','review_disposition','retained_claim','excluded_inference'], rows),
      'a5-typed-candidate-edges.csv': csv_text(['candidate','regime','relation','represented_behavior','missing_preservation_variables','discriminating_test','preservation_status'], edges),
    }


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--check', action='store_true')
    args = p.parse_args()
    files, out = render(), ROOT / 'audit-derived'
    if args.check:
        stale = [n for n,t in files.items() if not (out/n).exists() or (out/n).read_text(encoding='utf-8') != t]
        if stale:
            raise SystemExit('Missing/stale audited dispositions: ' + ', '.join(stale))
    else:
        if out.exists() and (not (out/'OWNERSHIP.txt').exists() or (out/'OWNERSHIP.txt').read_text() != OWNER):
            raise SystemExit('Refusing to overwrite unowned review files')
        out.mkdir(exist_ok=True)
        for n,t in files.items():
            (out/n).write_text(t, encoding='utf-8')
    print('A5 computed sets and candidate edges explicitly dispositioned')


if __name__ == '__main__':
    main()
