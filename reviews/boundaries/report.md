# Independent meaning and boundaries review: S276–S350

Run: `ac373bc5-f57e-44fb-aca3-feb20895edb6`  
Coordinator: `8f83af48-f692-47e6-bec2-cf69f55ac040`  
Phase: independent state discovery only

## Result and reading guide

**No actual Factory attractor was empirically established.** This is conditional analysis of all 75 assigned scenarios, supported in a few places by read-only source inspection. No production observations, toy dynamics experiments, live faults, external actions, implementation edits, peer reports or previous residue analyses were used. No residues are proposed.

- `states.csv`: 37 distinctions discovered from mechanisms, not a prescribed taxonomy. Each has entry conditions, persistence mechanism, exit, surviving capability, loss, and a falsifier.
- `trajectories.csv`: one individually written branching trajectory per scenario, `BDT001`–`BDT075`. For example, `BDT001` is S276, `BDT075` is S350. A row can contain several competing branches.
- `coverage.csv`: exactly S276–S350 once each. There are 69 **conditional** dispositions and six **unresolved** destinations: S319, S323, S338, S341, S344 and S347. Coverage is not a claim that the scenario is solved.
- `build_review.py`: hand-authored analysis plus CSV serialization, **not a simulation**. It writes only the three CSVs in this directory.
- `validate_review.py` and `validation.json`: mechanical schema, coverage and reference checks, not validation of the hypotheses.

Destination IDs are a space-separated **union of possible branch states**, not a claim that all occur, or that each is a terminal endpoint. Sequence text specifies branch order. In a compound incident, multiple states can coexist on different dimensions: a correct local hold does not mean an already buffered outside action is held. The state catalogue is deliberately not a mutually exclusive finite-state machine. `BD01` is verified completion of a **bounded operation**, never universal system health.

An **attractor hypothesis** here means a candidate self-maintaining regime with a stated basin and possible exit, not a proved mathematical attractor. Positive feedback alone is insufficient: it can instead produce unbounded drift, resource exhaustion, a finite burst or eventual service termination. Establishing attraction would additionally require observed recurrent/bounded behavior or return after small perturbations under specified background demand. None was measured. The proposed loops explicitly name the extra feedback needed beyond each initial stressor.

All scenario-level coverage remains conditional even where a narrow source transition supports a branch. Calling the whole business scenario source-supported would overstate the evidence. `BD16` classifies inadequate test evidence; S298's conditional mapping to it makes **no prediction of a production destination**. Unresolved coverage uses `none`, as required.

## What the trajectories say

### 1. Correct bytes and agreeing witnesses can still mean the wrong thing

S279–S280, S285, S287, S294, S300–S302, S331 and S339 separate three possibilities:

1. A mismatch is detected before an effect: an explicit validity hold (`BD02`).
2. A single plausible but wrong result is accepted (`BD04`). This is an erroneous completion, not automatically an attractor.
3. The wrong record becomes the source for the next check, backup and decision. Agreement increases trust and eliminates alternatives; reuse reproduces the original error (`BD05`). Only this third branch supplies a self-reinforcing mechanism that can survive removal of the original fault.

A checksum computed over an erroneous export confirms that export, not its correspondence to the world. Three reviewers consulting one bad reference table are not three independent factual sources. Conversely, an independent original can refute apparent consensus. Repeating a check with the same decoder is not that experiment.

Old prices can be correct when an order legitimately locks an earlier quote (S301). An amount-display defect need not imply that an actual payment used the wrong quantity (S300). Two equal names need not identify the same real person (S302), even if task IDs and signatures are impeccable. These counterexamples prevent treating syntactic validity, display meaning, subject identity and actual effect as one property.

### 2. A restored record is neither current authority nor a restored world

S281–S289, S297–S299, S318, S324–S327, S337 and S342–S349 repeatedly cross a time gap:

`authority checked / attempt recorded -> delay or lost evidence -> context changes -> participant may still accept`

The exposure is `BD09`, not proof of harm. Effective rejection at the **last effect-capable boundary** can yield `BD02` or `BD25`. Withheld retries and insufficient effect evidence yield `BD03`. Actual irreversible effects yield `BD10`. A rollback cannot un-send a communication, undo knowledge or reverse injury; a compensating payment may fix a balance without erasing the past event.

An old snapshot can retain enough evidence to block an old attempted task and still omit yesterday's invoice and today's revocation. Source-supported blocking therefore does not license the inference “no attempt in this snapshot means no real-world attempt.” Original and clone may both produce valid heartbeats. Unknown writer transfer (`BD11`) is distinct from active competing repairs (`BD08`): two writers alone give a conflict; an attractor hypothesis additionally needs each to react to the other's changes.

S324 distinguishes request reachability from reply reachability. S326 distinguishes monitor visibility from business reachability (`BD36`). A monitor timeout does not prove that the original is dead. Starting another instance without effective exclusion can transform a visibility problem into competing authority.

For S349, cold-cache retry overload (`BD24`) and an old payment's outcome are separate dimensions. Warming the cache need not settle the payment. Settling the payment need not drain the retry load.

### 3. Maintenance and scope coupling can change the system's effective authority

S286, S290–S296 distinguish:

- Circular bootstrap requirements (`BD12`): repairing a plugin requires the only config reader, which is that broken plugin.
- A merely unavailable prerequisite (`BD37`): a toolchain or legitimate account is missing. No circularity is implied.
- Maintenance lock-in (`BD13`): costly incidents consume learning/rebuild effort, encouraging patches that make the next repair harder.
- Authority erosion (`BD14`): unclear exceptions produce bypasses that become further unclear precedents.

The two last interpretations require repeated behavior; the departure of one maintainer or the presence of many rules does not establish a loop. Independent successor operation and consistent resolution of conflicting authorization cases would refute them.

An optional monitoring feature that requires a cloud login to start every child scope couples failure domains (`BD15`). A repair helper with independent write authority can become a competing writer, but a helper submitting commands through one authority is a counterexample. The instructions' per-scope activation and single-writer boundaries are **design obligations**, not evidence that every deployment enforces them.

### 4. Detection, understanding, acknowledgment and repair are different states

S303–S308 do not all mean “monitoring failed.” A warning can exist but be unusable because it is color-only, names the wrong truncated instance, mistranslates unknown as failed, or arrives at an off phone (`BD17`). An operator can understand and acknowledge correctly yet defer repair for three months (`BD19`). Neither is automatically a feedback loop.

Alert overload (`BD18`) becomes an attractor hypothesis only if unresolved incidents and reminders keep generating work faster than responders can clear it **after the initial burst stops**. Ten thousand finite alerts can instead drain. The discriminating observation is post-forcing backlog behavior, not the burst size.

S278's alert probing requires an ongoing attacker (`BD07`). Learned recipient/timing information can persist after probing ends (`BD06`); this is historical confidentiality loss, not continuing autonomous probing.

### 5. Authority, incentives and provider concentration are not just uptime

A sole monitoring account controlled by a departed or conflicted operator can leave independent oversight unavailable while business execution continues (`BD21`, S310–S311). Two legitimate representatives disagreeing (S309) present an authority-ordering question (`BD20`), not necessarily a forged signature.

S333–S335 supply a candidate incentive loop (`BD28`): easy-case selection, failure relabelling or unchecked artifacts improve the measured proxy; rewards increase the actor's ability to control that proxy. Honest triage or agreed maintenance accounting can be legitimate counterexamples. Independent accounting of rejected work and real outcomes is the discriminator, not a generic demand for more metrics.

S312–S314 concern common cloud or payment prerequisites (`BD15`) and permanent loss of a particular provider capability (`BD22`). Different vendor names do not prove independent infrastructure or funding. Existing offline backup bytes need not disappear when a subscription stops. A provider shutdown is not proof that no future substitute could ever work.

S315's delayed bill leaves liabilities still arriving (`BD23`). A local low cost total does not bound committed cost when the provider reports a week late. S316's speculation can improve latency under spare capacity, or produce `latency -> copies -> congestion -> latency` (`BD24`). The feedback law and capacity, not the ambitious percentile alone, decide which branch applies.

### 6. Privacy, law and physical safety impose different limits

Metadata can disclose customer identity or business hours (S276); a diagnostic export can disclose conversation content (S277). Historical unauthorized learning is `BD06`: stopping further distribution does not guarantee forgetting. A geographic transfer (S320) can violate location rules **without** new plaintext disclosure; conversely, a lawful location does not make every reader authorized.

No legal precedence is invented for simultaneous deletion and immutable retention (S319). No optimizer is authorized to exchange a human life for a compute allowance (S323). Complete erasure and proof-preserving secret export (S344) need clarified data boundaries, legitimate recipients, sequence and applicable duties. Export first and erase local copies may be coherent; retain a readable export while asserting that no readable copy exists anywhere is not. These three destinations remain unresolved rather than being assigned an imaginary lawful recovery.

S321–S322 concern actuators and hazard deadlines. A command to stop is not proof the plant stopped. A cloud-dependent emergency stop that answers tomorrow offers no demonstrated protection against a hazard today. `BD25` describes only an **actually enforced refusal**; it does not assert that abstention alone is physically safe or that buffered old actions disappear. No physical-system experiment was performed.

### 7. Long time horizons expose the limits of evidence

Suspend behavior, wall-clock reversal, civil-time reform and carbon deferral (S327–S329, S337) separate elapsed time, clock epoch, intended appointment and authorization validity (`BD26`). “Monotone” does not settle whether a clock counts suspend. A fixed-instant appointment and a civil recurrence can legitimately respond differently to new time-zone rules.

Native artifact interpretation can fail while bytes remain (`BD30`, S339). Case-insensitive restore can alias names without destroying information if the collision is held; `BD31` requires an actual overwrite and no surviving source of the overwritten content (S340).

S341's year 2080 does not identify what specifications, interpreters or institutions survive. S338's high monitoring-to-service energy ratio does not identify absolute costs, environmental effects or the criticality of the service. Both remain unresolved rather than being forced into decay or inefficiency attractors.

Local shutdown and monitoring contract expiry do not terminate participant queues (`BD32`, S342–S343). For S347, arbitrarily late **arrival is not mandatory acceptance**. Under additional assumptions of unbounded distinguishable operations, continued admission, finite retained distinguishing information and no effective participant expiry, forgotten histories can become observationally indistinguishable. That is a conditional information-limit argument, not proof that every finite-storage architecture is impossible. Bounded workload, enforceable retired epochs, or stopping admission changes the premise. The trajectory remains unresolved.

S345 is different: it literally removes every recovery source and legitimate actor inside the stated boundary (`BD33`). There is no surviving system whose attraction could be measured. S348 does **not** say all backups and every lawful successor vanish; its sequence of mute, host destruction and account lock is therefore not automatically total loss.

S346's perfectly consistent false world (`BD34`) is an epistemic indistinguishability limit. It specifies observations, not long-term physical dynamics. S350 additionally supplies installed malicious code capable of perpetuating trust through forged health and verification (`BD35`). Valid signing proves provenance, not benevolence. With no independent observation, internal perfect reports cannot adjudicate actual damage.

## Evidence ledger: code versus promise

Source was read from the working tree with repository HEAD `e93597f4145306a51c1867f1dfb9a2a505df7383`. HEAD alone does not prove the tree was clean; file digests are below. Tests were **read, not run**. Functions in uninspected ranges are not evidence for this review.

| Source, relative to Factory project | Inspected support | Limit |
|---|---|---|
| `crates/factory-task/src/deliver.rs` (full) | `deliver` commits an attempt before calling a writer, counts failed/unknown attempts against the budget, then records outcome. `authorise_resume` increments budget and requeues transactionally. | The shipped writer displays a prompt for manual paste. This is not external payment idempotency, participant fencing, authenticated human authorization, or expiry enforcement. Some historical comments disagree with the current resume body; the body determines the claim. |
| `crates/factory-recovery/src/restore.rs` (full) | Retained running/attempted queued tasks become blocked; unattempted queued assignments are cleared; lease-holding sessions disconnect. | It does not consult participant evidence or reconstruct a missing snapshot suffix. A repeated no-change reconciliation is a function fixed point, **not** an observed production attractor. Report writing follows commit, so report failure does not imply no state change. |
| `crates/factory-recovery/src/reconnect.rs:1–230` | `identifies_same_session` compares two known harness IDs, but accepts the identity comparison when either ID is absent. | A missing-ID fallback is not strong clone identity. This reading does not establish behavior of the whole reconnect implementation. |
| `crates/factory-recovery/src/evidence.rs` (full) | `may_promote_from_disconnected` checks `Confidence::Authoritative` only. | The predicate deliberately does not answer identity or general semantic truth. |
| `crates/factory-adapter/src/lib.rs` (full) | Hook marker/status parsing distinguishes authoritative, degraded and unavailable readings; idle/done do not complete tasks; degraded task signals are suppressed. Tests use `FakeHerdr` and payload fixtures. | Hook-reported provenance is not independent corroboration. Fixture tests do not establish real provider fencing or the timing coverage of every repository test. |
| `crates/factory-task/src/complete.rs:1–230` | Completion requires nonempty result/paths, bounded serialized size and permitted status transition. | The inspected path does not verify artifact existence, content, business quantities, recipient identity or truth. It does not prove that no upstream/downstream verifier exists. |
| `crates/factory-store/src/migrations.rs` (full) | Five forward migration entries delegate to `to_latest`; source tests cover seeded prior-schema upgrades and constraints. | No old released binary was run against a newer store. Local migration transactions are not distributed writer handoff. |
| `crates/factory-store/src/backup.rs` (full) | Snapshot uses `VACUUM INTO` and refuses an existing destination. | It is not the hypothetical checksum-producing exporter, nor a semantic validation of outside effects. |
| `crates/factory-paths/src/lib.rs:1–200` | Device/inode identity checks runtime aliasing; comments explicitly deny durable inode identity and hostile-process security. | Does not establish portable restoration of distinct case-sensitive names or recovery after overwrite. |

The inspected paths are library-level local mechanisms, including direct mutable table updates and a CLI-backed observation adapter. The project `AGENTS.md` describes a broader daemon/plugin/event-store target. Neither that target nor the existence of these libraries demonstrates a deployed transport-neutral kernel, semantic verifier, external-monitor system or provider enforcement. This review does not adjudicate the full implementation's architecture from a limited source sample.

### Source SHA-256 snapshot

```text
ac2257f260f2d33d80e68a4a35230d6ccaafb9e60725dbd59e538b616e7d0114  crates/factory-recovery/src/restore.rs
6d09a534dc6b2d0262c5d5a8d71210072d59aa74c3825111c2dec33fb46dfc56  crates/factory-recovery/src/reconnect.rs
8bb105ed22838bf08629d407af939911440d19f51531fdba6d7b26d7d851a482  crates/factory-recovery/src/evidence.rs
49bd4c6acb689c08e90b4381e963e081cd14199262f66abed2374f9837778014  crates/factory-adapter/src/lib.rs
4f24bf137fe68d764ea763213f426464d5540e94b1597d69c7a376daf81af2af  crates/factory-store/src/migrations.rs
ca047d16212c0ac13a5498bbdb8bfb111eb02bc9b29f2101db6ba3e1af204f55  crates/factory-store/src/backup.rs
84eae065a588c788ab5caf3dd8bffec0442988eabd4d3ed57788781271912322  crates/factory-task/src/deliver.rs
96b680dd8dd5161f7ff27d77ff07cd94f548d743dee668d95caed9c0a7a4232c  crates/factory-task/src/complete.rs
fb758150f590dd311e70f560edfe778e701712d8fbb87eba259448266e88d9ac  crates/factory-paths/src/lib.rs
```

## Next discriminating investigations — not authorized execution

These are questions for separately approved, isolated investigations, not implementation prescriptions or live-test authority.

1. **Semantic independence:** synthetic amount/unit/locale, duplicate-name and stale-quote fixtures checked against an independently authored domain answer. Distinguish a single wrong output (`BD04`) from repeat reuse (`BD05`). Test a shared wrong table against a genuinely separate origin, not another wrapper around it.
2. **Late acceptance:** isolated participant model with separately controlled clocks and retained operation identities. Vary response loss, revocation, restore age, ordering and queue lifetime. Observe whether a refusal occurs at acceptance, whether evidence remains unknown, and whether an old action still occurs. Any later real-provider test needs explicit approval and a non-harmful sandbox.
3. **Persistence after forcing removal:** hold base demand constant, stop flapping, warm caches, and count newly generated retries/alerts versus clearance. Backlog that drains refutes the corresponding attractor interpretation. A stipulated model would support only its own assumptions until calibrated.
4. **Authority and maintainability:** synthetic conflicting representative decisions plus a successor rebuild/repair exercise using only documented inputs. Measure whether missing prerequisites, actual circularity, or new exception creation explains failure. Do not access departed users' accounts or invent lawful authority.
5. **Human interpretation:** consented, synthetic mobile/color/translation scenarios that ask a representative operator to identify the correct instance, uncertainty and permitted next action. Separately measure receipt, comprehension, acknowledgment and verified repair; do not send real alerts.
6. **Dependency independence:** an offline dependency/account graph followed, only if separately authorized, by safe sandbox evidence of claimed independence. Different vendor logos and mock fencing responses are not observations of independent acceptance paths.
7. **Physical and normative boundaries:** qualified domain/legal review before defining any actuator, life-safety or deletion/export experiment. A cloud reply delay is not enough information to infer physical safety. No real valves, emergency stops, secret exports or erasures should be used to discover this boundary.
8. **Long-horizon reconstruction:** synthetic archives on differing encodings, CPUs, namespace rules and timing epochs; explicitly inventory surviving specifications and authority. Distinguish interpretable bytes, unavailable interpreters, detected collisions and actual information loss. For S347, specify bounded admission and participant expiry before claiming either closure or impossibility.
9. **Hostile self-certification:** an isolated evidence model with an independent observation boundary. Compare a malicious signer-controlled view to that independent record. Without the independent channel, S346/S350 cannot establish truth by asking the same substrate for another perfect report.

Phase 1 ends here. Peer comparison, residue derivation, implementation changes and external experimentation require a separate follow-up task.
