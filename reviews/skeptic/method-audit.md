# A5 skeptical methods audit

Assigned run: `f0720d4c-9639-4201-a33f-7261031b113a`  
Coordinator: `8f83af48-f692-47e6-bec2-cf69f55ac040`

## Verdict

**Retain A5 as a reproducible, conditional toy-mechanism catalogue. Do not treat it as discovery of Factory attractors or derivation of surviving capabilities.** A5 itself largely respects that boundary. Its 22 tests and both generated-file checks pass. The substantive corrections are narrower than rejecting the whole exercise:

1. The alarm transition silently clears unresolved backlog. Its bounded cycle depends on that reset, not just mute expiry.
2. The exclusive classification scheme inconsistently handles a policy-imposed permanent mute.
3. Two computed fixed sets disappear from the interpreted regime/traceability stage without disposition.
4. Downstream links need to distinguish an observed toy behavior, a failure motivating an intervention, and a proposed structure whose preservation was never modelled.

Other findings mainly strengthen already-present qualifications: chosen adjacency is not general stability, capped heat is not physical boundedness, a confidence attractor is not an artifact-quality attractor, and information starvation is not proof of irreversible loss. **No residue count is prescribed.** Findings are indexed in `findings.csv`.

## Scope, evidence and reproduction

I read this scope's `AGENTS.md`, the entire `../REVIEW-PROTOCOL.md`, and A5's README, specification, cases, transitions, authored regime/candidate tables, derivation, tests and generated model report. Original checks also read A5's historical scenario-input tables through its loader; I did not consult historical residue/control mappings as evidence. No peer reports were read. No production implementation claim needed validation: the findings below concern A5's own analytical source, not Factory runtime behavior. No production tests, live faults, external actions or source mutations were performed.

All paths beginning `a5/` below mean `../../docs/residuality/a5/`. Local artifacts:

- `baseline-checks.txt`: 22 passing tests, six model files checked, three downstream files checked.
- `sensitivity_probes.py`: ten assertion-backed probes, with alternative assumptions identified in code.
- `probe-results.json`: exact outputs and input SHA-256 hashes.
- `findings.csv`: semicolon-delimited findings and dispositions.

Reproduce from this workspace:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 ../../docs/residuality/a5/test_analysis.py
PYTHONDONTWRITEBYTECODE=1 python3 ../../docs/residuality/a5/dynamics.py --check
PYTHONDONTWRITEBYTECODE=1 python3 ../../docs/residuality/a5/derive.py --check
PYTHONDONTWRITEBYTECODE=1 python3 sensitivity_probes.py > probe-results.json
```

The last command writes only here. The probes import read-only A5 functions, make local/in-memory variants, and never invoke generator write modes. The hashes of `dynamics.py` and `cases.json` match `models.md`. Matching present contents does **not** independently authenticate the claimed historical ordering of model and candidate authorship.

Severity refers to the methodological inference, not a production security defect. **High** materially changes the interpreted dynamics; **medium** affects classification, traceability or permitted inference; **low** concerns auditability/procedure. Dispositions explicitly separate corrections from sensitivity work and retained caveats.

## Findings

### SK01 — High — Muting deletes the queue it is supposed to suppress

**Claim:** G09–G12 describe pending incidents, a flood/mute cycle, serviceable grouping and a retained human wait.

In `a5/dynamics.py:88–100`, every muted step returns pending count zero; triggering mute also returns zero. Grouping applies `min(1, pending + arrival)`, not just grouping new duplicates. Thus six old items plus four arrivals become one with **zero** human service in D13. The specification says muted steps suppress *new* items, but does not declare clearance of all outstanding items (`models.md`, §5).

Probe **P03** preserves unserved backlog while still suppressing arrivals during mute and retaining the same active arrival/service rates. Instead of D11's bounded six-state cycle, the pending counts at successive mute entries are `9, 12, 15, 18, 21, 24, …`. Each active reopening adds three; nothing drains the retained backlog while muted. This is an analytical countermodel, not a prediction about a real alarm system.

There are two legitimate interpretations, requiring different corrections:

- If pending means unresolved incidents, track acknowledgment, distinct identity and losses separately. Do not remove incidents merely because notification is muted.
- If pending means a disposable notification counter for one repeated symptom, rename it accordingly. The recurring set is then a **notification projection**, not a bounded unresolved-incident queue or evidence of a retained incident record.

The many-to-one reset loses information inside the represented state. A5 contains no hidden ledger from which to reconstruct it. Its existing warning that grouping may hide distinct incidents is useful, but does not cure the silent reset semantics. **Correction required before relying on G10's cycle or G11/G12's survival interpretation.** Retest both backlog-preserving and explicitly dropping policies; do not merely append another general toy-model disclaimer.

### SK02 — Medium — Policy, forcing and recurrence are mixed into one label

`models.md` §1 says policy-created holds remain separate from attractors because persistence comes from a chosen prohibition. D09 quarantine and D15 rejection follow that rule. D10's permanent mute is equally imposed: `mute == -1` has no exit transition, yet `interpretation()` labels it `model-attractor` and G09 inherits that label (`dynamics.py:173–190`). Mathematically, both quarantine and mute can attract in their specified projections; the inconsistency is **semantic**, not a failure to detect repetition.

Probe **P04** separates three mechanisms:

- Remove D11's arrivals: pending `6 → 5 → … → 0`; the flood cycle disappears.
- Remove arrivals after D10 is muted: permanent mute remains. The prohibition sustains it.
- Remove arrivals with D13's incident already pending: the incident remains with no human service. It does not require continuing alarm generation.

D11 is **not** the same kind of forcing as D17's external alternating writer schedule. Constant load plus a mute controller can legitimately have a conditional limit cycle in an autonomous reduced model. It is not a self-sustaining flood after the load is withdrawn. A5 already states sustained arrivals, and correctly separates D17/D18's business projection from scheduler phase.

**Correction:** classify permanent mute consistently with other policy holds, or use separate fields for recurrence/return evidence, persistence mechanism, and input assumptions. Record whether input withdrawal breaks recurrence. Do not erase valid conditional cycles just because they consume external inputs.

### SK03 — Medium — Non-attracting fixed sets are found, then lost downstream

Probe **P01** confirms A5 correctly refuses to call D01's `q=4` and `q=5` attractors under its unit-neighbor rule:

| Initial queue | D01 destination |
|---:|---|
| 3 | 0 |
| 4 | 4 |
| 5 | 5 |
| 6 | escape |

These are boundary/separator fixed states, not two demonstrated attracting equilibria. D04 differs: each represented high queue stays at its own value when arrivals equal congested service. Interior neighboring queues stay displaced; this is a neutral family, with a lower boundary at six. Changing congested service from one to zero gives growth; changing it to two drains `q=8`. A generic “fixed without attraction” label is conservative but loses this distinction.

Probe **P07** finds **D01.3 and D01.4 have no matching authored regime at all**. G04 covers only D04. `derive.model()` checks that each declared regime kind exists in its cases, not that every computed set receives a disposition. Its trace has no set identity or seed condition (`derive.py:26–53`). It consequently cannot expose this omission.

**Correction:** disposition every computed set as interpreted, intentionally excluded, or unresolved; carry exact set references and conditions. Keep separator behavior distinct from neutral balance even if both remain under a broad non-attractor heading. This does not require one candidate per set or any particular number of categories.

### SK04 — Medium — Basin results are restricted return tests, not stability certificates

`neighbors()` perturbs only queue/pending count, heat or confidence; it does not test restart phase/wait, mute policy, artifact truth or evidence provenance. The writer test flips business value; the effect test only reduces bounded age. `analyze()` additionally requires a transient among the chosen initial seeds (`dynamics.py:154–164,205–219`). These choices are declared, and A5 explicitly disclaims general mathematical stability. There is **no implementation defect relative to that advertised heuristic**.

Probe **P06** nevertheless identifies two important interpretive limits:

- Keeping D14's transition and neighbor tests unchanged, but selecting only its endpoint as seed, changes its label from `model-attractor` to `model-fixed-point`. The dynamics did not change; the sampled evidence did.
- A separate toy map `1 → 100 → 0`, `2 → 0`, `0 → 0` passes A5's return heuristic for zero. The check does not limit excursion magnitude or recovery cost/time. This is not a claim about Lyapunov stability under every possible discrete topology; it shows the operational property the test does **not** check.

**Qualification/sensitivity, not blanket retraction:** surface tested axes, outside-set seeds, return time and maximum excursion beside each result. Use “return-tested on this grid” as the evidential meaning. Before stronger resilience claims, perturb relevant hidden/control state and separate parameter interventions from state perturbations. No selected seed cardinality is an incident probability; A5 already gets that right.

### SK05 — Medium — Hot fixed-point attraction depends on saturation; cooling has neutral cases

In D06, `heat = min(6, heat + 2)` makes the hot branch land on a fixed point. Probe **P02** removes only the cap and obtains `4 → 6 → 8 → … → 20` in eight steps, with continued failure. Harmful restart feedback survives; bounded attracting heat does not. Conversely, with D08's permanently bad binary and cooldown **two**, the seven heat seeds reach five recurrent sets which fail full local-return testing. Cooling exactly offsets boot cost away from saturation. Cooldown three contracts them to the published cycle.

A5 expressly calls heat ordinal/capped, not a physical thermal simulation (`models.md`, §4). A saturated ordinal abstraction can be legitimate; unlike queue escape it deliberately merges all sufficiently hot states. It cannot establish physical boundedness or structural robustness.

**Qualification plus sensitivity case:** retain the exact fixture result but distinguish “repeated failed starts in a saturated pressure projection” from “bounded resource regime.” Include balanced cooling and uncapped/alternative saturation probes before generalizing the basin. The robust lesson is that cooling need not repair a bad binary, not that failed restart dynamics must have this attractor.

### SK06 — Medium — The wrong artifact is invariant; only confidence is attracted

D14 increments confidence, caps it at four and never changes the truth bit. It does not contain a mechanism pulling a corrected artifact back toward error. Probe **P05** flips truth to correct and stays at the other accepted endpoint; reducing confidence while retaining the wrong artifact returns to confidently wrong acceptance. The published result is therefore attraction **within a fixed-artifact/evidence-process slice**, not attraction of artifact content toward falsehood.

Moreover, `step()` keeps verifying after `phase == 'accepted'`. A separate stop-on-accept variant reaches accepted confidence three and stays there. A confidence perturbation of an already accepted artifact no longer rebounds. For D16 that gives ordinary corrected completion, not demonstrated self-maintaining service. Whether accepted work remains in an active review loop is a necessary operational assumption, not something inferred from acceptance itself.

The repair oracle directly sets `wrong=False`. A5 already discloses this, and N10 is correctly `unproven-prerequisite`; no evidence supports charging A5 with secretly discovering a universal verifier.

**Correction to the claim's object, plus sensitivity:** label G13 as confidence reinforcement around a fixed wrong artifact. For G15, explicitly justify continued post-acceptance dynamics or give it ordinary-completion semantics at the work-product level. Keep oracle-driven correctness as a stipulated intervention, not an independently derived survivor. Merely increasing the reviewer count or replaying the oracle fixture supplies no independent evidence.

### SK07 — Medium — A join traces proposals, not causal preservation

`derive.py` is an authored relation join. It validates references and coarse kinds, not the substance of `retained_function`, evidence preservation or interventions. Probe **P10** substitutes a plainly unsupported G01 name (“Every accepted task has completed with independent correctness proof”) in memory; validation still passes. That is expected for this validator, not evidence of fraudulent output. `models.md` explicitly acknowledges the limitation.

The consequential issue is that the same trace edge serves different meanings:

- G07 repeatedly restarts a broken binary; N05 proposes **ending** retries and preserving diagnostics, which G07 neither does nor records.
- G12 has one scalar count; N06 proposes a durable, accessible handover record, absent from the state.
- G20 records duplicate acceptance; it is a **counterexample/motivation** for N13/N14, not a demonstration of deduplication or safe restore.
- G01 includes D03/D26's eventual empty queues with no sustained fresh work. Empty backlog is not by itself productive steady-state operation or proof of bounded admission.

Probe **P08** supplies a projection counterexample: both FIFO and a newest-first scheduler realize D05's constant `q=2`. FIFO completes the original two jobs; newest-first services three new jobs every tick while starving the originals indefinitely. This **does not refute N01's weak “some accepted work” wording**. It refutes using the same scalar trace to infer fair useful completion of retained work. Durability, permission, identity and quality are also absent.

**Correction to trace semantics before architectural reliance:** type edges as behavior-supported, hazard/counterexample, proposed intervention, or unmodelled prerequisite. Attach the preservation predicate, missing variables and discriminating test to each retained-function claim. Keep the existing proposal status; do not relabel every proposed control an observed residue. A candid authored design proposal can remain useful without being an algorithmically derived survivor.

### SK08 — Medium — Information starvation is not a demonstration of irrecoverable loss

The effect model usefully distinguishes ground truth from sender knowledge, repeated deduplicated calls from silent waiting, and historical duplication from future service dynamics. These distinctions should be retained. The saturation of age at three is behaviorally valid for its present time-insensitive lost-response rules, not for an extension with age-dependent dedup expiry.

Probe **P09** contrasts two worlds: initial dispatch accepted versus never accepted, both permanently silent. Their sender observations `(phase, age)` are identical while accepted-effect counts differ. A5 models only the accepted branch for these cases. Local “unknown” is an equivalence class of possible external histories, not an observable count of effects. D19/D22's identical state endpoints also hide different repeated-call costs, as A5 already notes.

Neither `response='lost'` nor `remember=False` demonstrates that every independent receipt, backup, witness or recovery path is unavailable. Nor does “a duplicate historically happened” prove uncompensable net business harm: it proves an irreversible **historical fact** within the chosen effect semantics. This is consistent with A5's caution about the `damage` stop marker.

**Scope qualification/new analysis, not relabelling the existing waits as loss:** keep information-starved holds and historical duplication. For irreversible-information-loss claims, model accessible evidence sources and indistinguishable histories after their destruction; state what reconstruction is impossible and why. Test recovery with and without an independent receipt. Do not force unavailable keys, destroyed records or lost formats into autonomous-attractor boxes or claim they were covered by these effect cases.

### SK09 — Low — No hard residue-count target found; independence and screening remain unaudited

The transition module does not read candidate labels; no candidate count is required by the derivation validator. The literal existing counts in prose are reported outputs, not an optimization objective. The historical-count comparison is downstream. **I found no code evidence of a forced residue count.**

However, content hashes and a one-way code dependency cannot blind the analyst who chose the mechanisms, seeds, state coordinates and interventions. A5 acknowledges prior familiarity. Its `test_model_is_independent_of_downstream_candidates` tests runtime data dependence, not psychological or causal independence (`test_analysis.py`).

Likewise, `render()` marks every unlinked scenario `text-screened-only` solely because it has no case link (`dynamics.py:259–266`). The file contains no authored per-scenario rationale showing that screening occurred. This does not disprove the author's statement that all texts were read; it limits what the generated evidence can audit.

**Procedural correction/retained caveat:** describe the guaranteed property as pipeline separation, not independent discovery. Separate automatically computed “no toy link” from an authored screening disposition. Record mechanism-selection and exclusion reasons, retain unresolved states and use the separately authorized independent review to challenge them. Do not normalize, split or merge outputs to meet an expected count. I did not read those independent reports in this run.

## What survives this audit

- Queue growth versus draining is a real distinction **within the stipulated equations**. Removing fresh demand alone can leave retry growth; removing retries alone can leave exact balance. The existing tests support that conditional claim.
- A5 does not mistake its queue separators for attractors or its observation cutoff for a stable queue.
- Immediate-restart feedback and a permanently bad binary are different causes; backoff need not repair the latter.
- The unfenced writer cycle is explicitly driven by a periodic schedule. The fenced business value is stable even though the clock still alternates. That projection distinction is correct.
- Unconfirmed outcome, completed operation and known duplicate acceptance are not interchangeable. The effect model's ground-truth disclaimer is essential and present.
- Candidate statuses and the perfect-oracle warning appropriately prevent a production-capability claim.

These are toy-supported distinctions, not measured Factory regimes. Passing the checks establishes reproducible execution and reference integrity, not causal identification or survival under real stress.

## Recommended next discriminating work

1. **Correct and rerun the alarm model first.** Compare retained unresolved incidents with dropped notification counters. Account for every old incident as pending, acknowledged, safely merged, suppressed-but-retained, or lost.
2. **Repair the evidence representation.** Add complete end-set dispositions, typed candidate edges and orthogonal persistence/input/return fields. This changes evidence quality, not a target category count.
3. **Broaden isolated counterfactuals.** Balanced cooling, cap changes, input withdrawal, post-acceptance stopping, queue identity/fairness and evidence-source removal test materially different mechanisms. Preserve unresolved outcomes.
4. **Only then seek source-backed or controlled implementation evidence** for any selected architectural proposal: actual retry producers, retained diagnostic/incident artifacts, participant dedup/fencing and receipt availability. Real integrations remain separately approval-gated.

The appropriate result is correction where the represented state contradicts its meaning, and narrower claims where the model intentionally stipulates the answer—not more confident labels or a different prescribed number of residues.
