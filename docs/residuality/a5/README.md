# A5 — states first, residues afterwards

**Independent audit update:** [A6](../a6/README.md) supersedes several interpretations below. The alarm model clears represented pending items and therefore does not establish incident preservation. Confidence reinforcement, imposed mute, capped pressure and candidate links also require narrower claims. Historical equations and tables remain unchanged; [explicit claim dispositions](../a6/a5-claim-dispositions.csv) and [typed links](../a6/audit-derived/a5-typed-candidate-edges.csv) record the corrections.

## The important correction

**We should not ask which of 17 boxes a stressor fits. We should ask where the system goes, why it stays there, and what can still be used in that state.**

This pass follows that order. It finds genuinely different *conditional dynamics*, including harmful self-reinforcing states and cycles. It does **not** establish empirically observed Factory attractors. That would require instrumented observations or controlled tests of Factory itself.

### What was actually done

- Read all **350 scenario texts**, without using their old residue/control mappings as model inputs.
- Build **six small explicit transition models**, with **26 parameter cases** and **164 initial-state runs**.
- Follow each trajectory until it repeats or leaves the observation window; perturb the resulting states and check whether they return.
- Interpret the resulting behavior into **22 conditional regimes**, carefully separating attractors, forced cycles, policy holds, missing-information waits, completion and damage.
- **Only afterwards**, propose surviving structures and architectural consequences. The model-stage hashes were recorded in the task before the candidate file was written.

The toy cases link to mechanisms in **55 stressors**. The other **295 were text-screened only**. No one-to-one terminal state has been invented for them. The author already knew the previous analysis, so removing old labels from the input is **not** an independently blinded review.

## Six examples of the reversed reasoning

### 1. Overload can sustain itself after the original demand stops

**Stressors:** rate limiting, task multiplication and retries at several layers (S060, S196, S237).

**What happens in the model:**

```text
Small queue:       3 → 1 → 0 → 0
Large queue:       8 → 12 → 18 → 27 → 40 → …
New demand stops:  8 → 11 → 15 → 21 → 30 → …
Only retries stop: 8 → 8 → 8
Both sources stop: 8 → 7 → 6 → 5 → 2 → 0
```

These are exact outputs of the toy rules, **not Factory measurements**. At high backlog, service capacity is lower and retries add more work than can be cleared.

**States found:** a productive low-backlog attractor; runaway growth; and a queue plateau where growth has stopped but recovery has not begun. Runaway growth is **not** mislabeled a stable attractor.

**What can survive:** useful accepted work under bounded admission; separately, a way to drain old work without replenishing it.

**Architecture consequence:** “normal versus stopped” is too coarse. Investigate an explicit **draining transition** that restricts both new intake and retry feedback below actual degraded service. It must not manufacture capacity by freeing an uncertain workspace or replaying unknown effects. This would be an admission/retry operating restriction, not a new task lifecycle state or a second daemon.

**Why this changes the old grouping:** the broad “bounded capacity” category hides prevention, bounded useful service and recovery out of a trapped backlog. Those are different contracts.

### 2. Automatic restart can be the thing keeping the system broken

**Stressors:** repeated plugin crashes, cold startup and thermal/resource pressure (S003, S044, S240, S255).

```text
Viable process + low initial heat → running
Same process + high heat + immediate restart → failed boot → more heat → failed boot …
Same process + enough cooling → running
Permanently broken binary + cooling → wait → failed boot → wait → failed boot …
Finite attempt rule → quarantined, not running
```

**States found:** two basins for the same immediate-restart policy; a harmful hot-restart attractor; a recurring bounded restart cycle; and a deliberately imposed quarantine.

**What can survive:** a bounded startup compartment that can genuinely reach running operation, or a non-running integration with safe diagnostic evidence preserved.

**Architecture consequence:** distinguish **cooling**, **repair** and **quarantine**. Backoff is not repair. Quarantine is not recovery. A working startup path needs different budgets and tests from steady-state execution.

### 3. An expiring mute can create a permanent alarm cycle

**Stressors:** alarm overload, indefinite suppression and flapping (S092, S209, S307).

```text
Too many alarms → permanent mute → silence
Too many alarms → temporary mute → expiry → too many alarms → temporary mute …
Grouped incidents + available person → serviceable queue
Grouped incidents + nobody available → one pending incident forever
```

**States found:** a suppression fixed point, a six-step flood/mute cycle in the fixture, a serviceable incident queue and a bounded but unserved human wait.

**What can survive:** a grouped incident delivery lane; an unacknowledged incident record for later handover; and an inspectable suppression/re-entry policy. These are not interchangeable.

**Architecture consequence:** do not treat “the mute expires” as sufficient. Test what happens **after** expiry. Grouping needs to preserve distinct urgent incidents, and escalation needs an actually reachable legitimate person.

The external-monitoring decision from A4 remains a proposal. This model studies the human alert loop, **not** real heartbeat delivery, independent infrastructure or a real pager.

### 4. A wrong answer can become an attractor

**Stressors:** correlated reviewers and optimizing for reported success (S104, S184, S331, S335).

```text
Wrong artifact + echo verification:
confidence 0 → 1 → 2 → 3 → 4 → 4
artifact stays wrong throughout
```

A small reduction in confidence returns to the same confidently wrong state because the evidence process has not changed.

An independent contradiction produces a **held draft**, not automatically a corrected result. In a separate case, a stipulated perfect repair oracle produces a corrected accepted artifact. That oracle is an assumption, not a delivered Factory capability.

**What can survive:** the draft and legitimate contradiction evidence; potentially a work package whose acceptance criteria and evidence can be inspected independently.

**Architecture consequence:** preserve useful rejected work and its reason for rejection. Do not equate more reviewer agreement with independent information. Do not infer a universal verifier from a model that was given the answer.

### 5. Not every recurring state is an autonomous attractor

**Stressors:** competing clones and interrupted writer handover (S065, S185, S198, S283).

```text
Alternating unfenced writers: business value A → B → A → B …
Participant-enforced fence:  business value A → A → A → A …
```

The first cycle is driven by an **explicit external alternating schedule**. In the second model, the scheduler's turn still alternates, but the business value is stable. Calling both two-state cycles would hide the important difference.

**What can survive:** a participant-enforced single-writer binding, not merely a local identity or a green heartbeat.

**Architecture consequence:** inspect the business-state projection and the actual participant rejection. Do not count scheduler phase as business instability. A mocked fence does not establish a real one.

### 6. The same apparent “unknown” state can contain different behavior

**Stressors:** lost responses, expired participant history and old restores (S051, S090, S231, S282).

| Trajectory | Persistent or terminal condition |
|---|---|
| Accepted action, response lost, no repeat | Outcome unknown; no further calls |
| Same, repeated requests with retained deduplication | Outcome still unknown; no second effect, but repeated calls continue |
| Same, participant has forgotten the key | Duplicate-effect damage occurs |
| Response eventually arrives, no repeat | Ordinary confirmed completion |
| Old restore forgets prior acceptance | Even the apparent first send can duplicate an effect |
| Restore gate before dispatch | Historical intent is held pending current evidence |

**These are not six attractors.** They include information-starved waits, completion, a policy hold and an irreversible damage fact.

**What can survive:** a local pending-operation journal, participant deduplication memory, or a restore-quarantined work package. Local history and participant memory can fail separately, so treating them as one “durable intent” capability is too broad.

**Architecture consequence:** distinguish these structures and their retention contracts. A sender may not know that damage happened; the toy model's observer has ground-truth access that Factory does not. No duplicate-effect audit record is assumed to magically exist after a lost response.

## What this means for residues

The useful variety is **not just more names**. It is the distinction between:

- states entered from different basins under the same policy;
- stable operation versus bounded failure versus self-sustaining failure;
- recovery entry and steady-state behavior;
- local knowledge and actual participant state;
- physical/service availability and legitimate human availability;
- a surviving structure and a prerequisite that may not exist.

The [downstream candidate table](derived/candidates.md) proposes **14 provisional structures for these investigated mechanisms**, with conditions and counterexamples in [candidates.csv](candidates.csv). **This is not a new global claim that 14 replaces 17.** Most of the corpus has not received this depth of analysis. The count is neither an objective nor a convergence result.

### Explicit split / merge / reject decisions

| Earlier broad grouping | State-first result |
|---|---|
| R07 bounded capacity | Split candidate contracts for admission-bounded service, draining and cross-layer attempts. Queue growth stopping is not the same as recovery. |
| R06 localized integration failure | Split viable cold startup from permanent quarantine with retained diagnostics. They have different exit conditions. |
| R10/R17 human control and notification | Distinguish delivery grouping, a durable unacknowledged incident and bounded suppression/re-entry. An external observer is a separate unmodelled dependency here. |
| R14 result quality | Distinguish retained contradicted drafts from justified acceptance. Perfect independent judgement is an unproven prerequisite, not an automatic residue. |
| R02 durable effect intent | Split local pending evidence from participant deduplication memory. The same local state can consume different resources or permit different damage. |
| R04/R05/R11 historical work and recovery | Combine their relevant pieces into a narrowly defined restore-quarantined work package for this mechanism. This does not merge their entire domains. |
| “Every stable endpoint is an attractor” | Reject: a fixed separator, externally driven cycle, policy stop, ordinary completion or irreversible damage needs its own classification. |
| “Every attractor is a desirable residue” | Reject: a restart trap and confidently wrong output are attractors we may need to escape. They are not capabilities to preserve. |

## What remains unproven

**Zero actual Factory attractors have been empirically established by this pass.** The demonstrated behavior belongs to the explicit toy dynamics and chosen parameters. Those rules determine what can emerge; they are not learned from production traces.

The [screening file](generated/screening.csv) lists every scenario and whether a toy mechanism was linked. Important unmodelled mechanisms include genuine external-monitor failure, inaccessible encryption keys, legal retention conflicts, hostile same-user compromise and long-term loss of executable formats. A powerless machine or irrecoverable archive may be an externally maintained outage or absorbing loss, not an attractor at all.

Insisting that **all** useful residues must first come from an autonomous attractor would lose those cases. We should retain separate analyses for terminal loss, authority constraints and required recovery capabilities.

## Next: test whether these basins exist in Factory

1. Measure queue age, offered/admitted work, actual service and every retry producer. Under an isolated load fixture, remove fresh arrivals separately from retries. Does a backlog plateau or continued growth persist?
2. Observe restart attempts and shared resource pressure. Compare cooling, unchanged retry and explicit quarantine without assuming backoff repairs the cause.
3. Exercise an isolated alert chain with repeated symptoms, mute expiry and an unavailable recipient. Check the recurrence, not just the first notification.
4. Test a known wrong artifact with genuinely independent evidence. Keep rejection, correction and acceptance distinct.
5. Verify participant behavior and the distinction between local uncertainty and actual accepted effects using an isolated fake, then an explicitly approved real integration test if needed.

No live failure injection, real alarm, provider mutation or production change was performed here. Numerical fixture values are not proposed production settings.

## Files and reproduction

- [Model assumptions, equations and limits](models.md)
- [Parameter cases](cases.json) and [transition code](dynamics.py)
- [Exact trajectories](generated/trajectories.csv), [end sets and seed basins](generated/basins.csv), [model report](generated/model-results.md)
- [Interpreted regimes](regimes.csv)
- [Candidate structures](candidates.csv) and [stressor → case → regime → candidate trace](derived/traceability.csv)
- [Downstream derivation code](derive.py) — does not feed candidates back into the models

```sh
# From the Factory project root; only Python standard library:
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a5/test_analysis.py
python3 docs/residuality/a5/dynamics.py --check
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a5/derive.py --check

# Regenerate only after deliberate source changes:
python3 docs/residuality/a5/dynamics.py
PYTHONDONTWRITEBYTECODE=1 python3 docs/residuality/a5/derive.py
```

**Executed:** 22 tests of these toy rules and their documentation passed. Both model and downstream derivations reproduce their repository-owned analysis files. This is evidence about the analysis implementation, not a Factory load, recovery or monitoring test.

Task run: `95a0f308-3ed2-41cb-80a9-15ca144ffa04`. A0–A4 are retained as historical hypotheses, not rewritten into retrospective proof.
