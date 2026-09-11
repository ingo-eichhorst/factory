# Independent operations review: S001–S100

Assigned run: `ad6de62d-17ea-42c3-b119-70357426b6a8`  
Coordinator: `8f83af48-f692-47e6-bec2-cf69f55ac040`  
Phase: state and trajectory discovery only, 2026-09-09.

## Result and limits

**No actual Factory attractor was empirically established.** This is a read-only source review plus conditional reasoning, not a production incident study. No live faults, external actions, implementation edits, worktree operations or executable dynamics models were performed. I did not read `docs/residuality/`, old analyses or peer reports. No residues are proposed or counted.

The deliverables contain:

- `states.csv`: 35 distinct state/regime descriptions, including ordinary processing, rejection, externally maintained outages, policy holds, information waits and losses.
- `trajectories.csv`: 100 scenario-specific branching analyses, `OPT001`–`OPT100`. The suffix corresponds to its scenario, not a ranking.
- `coverage.csv`: exactly one disposition for every S001–S100: 86 conditional, 5 source-supported and 9 unresolved.

A source-supported coverage status means the named **library transition** is directly supported under its stated conditions. It does not mean the whole scenario was experimentally reproduced. Conditional rows can also contain source-supported fragments alongside design-only or hypothetical branches. There are no toy-supported rows because no toy dynamics were executed.

The catalogue is not a partition of all possible system configurations. A physical effect and an observer's knowledge can coexist: an email can already be delivered (`OP028`) while Factory cannot establish its outcome (`OP022`). A task can have a useful artifact but no accepted completion (`OP034`). Space-separated destination IDs enumerate branches or simultaneous dimensions; the sequence column explains which. `OP026` ends a refused **request**, not necessarily the queued task it tried to change. `OP001` includes continuing ordinary work and finite recovery, not only terminal completion.

The nine `none` dispositions are deliberate. Their trajectories still state the alternatives and the missing discriminator. Explicit disposition is coverage, not resolution.

## What the inspected implementation actually supports

Working-tree source was inspected at repository HEAD `e93597f4145306a51c1867f1dfb9a2a505df7383`. This identifies the surrounding checkout, **not a guarantee that every inspected file equals that commit**. Source locators below are path/function based. No source files were changed.

There are concrete task, session, store, adapter, delegation and recovery libraries. Their bodies matter more than stale explanatory prose. The README says some slices are not implemented even though their source files exist. Delivery's module commentary also contains older statements about resume, while the present `authorise_resume` body explicitly increments the budget. I used the bodies for those claims.

The sampled task implementation mutates relational `tasks` and `delivery_attempts` rows. It is not evidence that the ADR's complete canonical event store, logical command-idempotency service, plugin supervisor or external saga runner is already operating. Those capabilities did not appear as implemented hosts in the inspected crate layout. I did not infer a complete repository-wide absence from that sample, or test an operational CLI. ADR 0003 and ADR 0007 are **accepted design promises**, not measured runtime behavior.

Important supported boundaries:

1. **Delivery journals before calling a writer** (C1). Failed prerequisite commit prevents that writer call. An attempt consumes authorization even if the writer reports failure. The shipped writer renders text for a human to paste; it does not automatically type into a harness.
2. **Restore reasons only from the restored records** (C2). Running and queued-with-attempt tasks become interrupted blocks. Queued-without-attempt tasks stay queued, with assignment cleared. Lease-holding sessions become disconnected and retain leases. The report file is written after commit, so a reporting error is not proof reconciliation rolled back.
3. **One-store assignment is serialized** (C3). A queued or blocked task already assigned to a session makes it non-idle. Starting/disconnected sessions are also not idle. Stable-directory alias reservations are checked under the store write transaction using filesystem identity (C10).
4. **Weak observations and turn completion are not task completion** (C11). Missing hook authority suppresses task signals. `idle` and `done` both yield `NoChange`.
5. **There are explicit identity and recovery limitations** (C4). A known harness-UUID mismatch is rejected. If either UUID is missing, the helper returns true. The common Herdr-or-machine-restart path can presume a session gone when positive reconnection evidence is absent, then record a replacement session in an existing workspace. It records, rather than launches, the replacement.
6. **Result shape is not result truth** (C5). Size and nonempty summary/path requirements exist. Artifact paths are stored as strings, without validating their bytes or task-specific correctness.

## Key trajectories and competing basins

### 1. A crash has several materially different cut points

S001, S008, S011, S014, S015 and S095 do not all lead to one recovery state.

- Before a successful intent commit, the inspected writer is not invoked. Recoverable queued work may survive; new mutation is unavailable while storage fails (`OP008`).
- After an attempt commit but before trustworthy receipt/result evidence, the prompt may have gone out. Ordinary delivery is held (`OP014`). Repeated refusal is a **policy fixed point**, not evidence of attraction.
- A week-old restore can lack the attempt entirely (`OP011`). The correct local restore rule then leaves the task eligible. This is a counterexample to interpreting *no attempt in this snapshot* as *no delivery ever happened*.
- A missing WAL is not invariably corruption. A checkpointed, consistent copy may already contain everything needed. Otherwise there may be an omitted tail or an inconsistent copy.

The surviving structure is the actual retained prefix and any independent evidence, not the instruction “restore safely.” An omitted fact cannot be recovered by repeatedly querying the same prefix. A verified later journal or participant receipt changes the information available and therefore the basin.

### 2. Cancellation is not the same hold as ambiguous nonterminal delivery

S072 exposes a specific possible library interleaving:

1. `deliver` commits an attempt while task status remains queued.
2. `cancel` sees queued and commits terminal `cancelled`.
3. `deliver` invokes its already-prepared writer without rereading status.

This is `OP035`, not merely `OP014`. A cancelled task cannot directly use the blocked-only `authorise_resume` exit. A rendered prompt could still be pasted later, but **source inspection does not prove actual delivery or an external effect**. A mandatory caller lock spanning both transactions and the writer would falsify the integrated race. No such end-to-end race was executed here.

This separation preserves the cancellation record and attempt as surviving evidence without pretending the record itself extinguishes physical write capability.

### 3. Loss of observation can preserve work while making reuse unsafe

S031–S040, S096 and S099 distinguish four basins:

- Genuine disconnected uncertainty retains a lease (`OP015`), and the old harness may still produce useful files.
- A positively different UUID prevents same-session promotion.
- A missing UUID can remove that discriminator and permit wrong association (`OP025`). A renamed transcript alone does not prove an incorrect association; a reused pane plus missing identity is the stronger counterexample.
- Presuming old writers gone and later launching another writer can create overlapping work (`OP016`). The same risk arises from surviving descendants or a human editing after pane closure.

An exclusive database lease is useful retained coordination evidence. It is not a process-tree kill, filesystem capability revocation or control over human edits. Conversely, two stable case aliases in one store are covered by the inspected serialized identity check (S037). Two cloned databases on different machines (S065) lie outside that guarantee.

### 4. Backlog is not automatically an attractor

S003, S024, S043, S060 and S068 suggest different dynamics:

- A finite wake-up batch drains if useful service exceeds new arrivals: ordinary recovery (`OP001`).
- Continuing arrivals above reduced thermal/provider capacity maintain congestion (`OP003`). Remove the forcing and it should drain.
- A possible internally sustained trap (`OP004`) requires backlog to cause latency, latency to cause retries or equivalent task creation, and that new load to prevent useful completion from catching up.

One can state the discriminator without claiming a simulation: after external arrivals stop, compare endogenous generated work with completed removals at low and high backlog. A genuine different high-load basin requires recurrence or a sustained bounded regime after the original perturbation ends. **Unbounded growth ending in disk exhaustion is not by itself an attractor.** The current evidence establishes neither boundedness nor the feedback gains.

Similarly, deterministic plugin crash/restart (`OP019`) is a **policy-driven cycle**. ADR 0007's finite nonresetting restart budget would end it in quarantine (`OP018`). Endless restart requires a missing/reset budget or continued operator resets, not merely a crashing executable.

### 5. Remote uncertainty, irreversible work and wrong compensation differ

S046, S049–S051 and S081–S090 share uncertainty but have different exits:

- A lost response produces local uncertainty (`OP022`), not proof of failure.
- A known delivered email is an ordinary irreversible completed effect (`OP028`), even if now regretted.
- Blind retry against a provider ignoring idempotency can leave duplicate effects (`OP023`). Two different tasks ordering one intended purchase can do the same without any retry bug.
- A failed inverse leaves a manual disposition wait (`OP029`). An inverse that overwrites newer human work creates a different damaged world (`OP030`).

An intent record survives only if actually retained; its existence cannot force participant durability or reconstruct expired participant history. The decisive observations are durable participant identities, acceptance evidence and versioned postconditions, not another timeout or a success string.

### 6. Approval can be valid when clicked and wrong when exercised

S027, S028, S083, S088, S089 and S094 separate approved intent from accepted meaning (`OP027`). Expiry during startup, changed invoice, retry repricing, new confidential rows and the wrong installation all require exact semantic and temporal boundaries. A current content/terms check could refuse the action (`OP017`/`OP026`). The sampled code does not establish that this external approval binding exists.

A late revocation after acceptance (S084) does not make the original accepted effect disappear. A draft refresh is also not disclosure unless transmission actually occurs to an unauthorized recipient. These are counterexamples to treating every changed payload as the same realized harm.

S092 adds a second **unestablished attractor hypothesis**, `OP032`: alarm load reduces review quality; bad approvals create errors; those errors create more approval load. Both links must exist. A finite burst followed by ordinary recovery, or dialogs that merely acknowledge information without authorizing effects, refutes the proposed loop.

### 7. Durable labels, durable bytes and durable meaning are different

S004, S016, S075, S076, S078, S080 and S098 show why these distinctions matter:

- A path can remain after its original bytes vanish (`OP012`).
- Correct durable bytes may survive worker death before result commit (`OP034`).
- A structurally valid result can be semantically wrong (`OP005`).
- A zero exit code can be valid for an intentionally silent task, or invalid evidence for a task that required output.
- Oversized completion is refused before mutation, but the caller may already have allocated the input text.

No validation rule is counted as a surviving work product. The actual task identity, summary, original verified bytes and independently readable versions are the candidate usable material. Their usefulness is conditional on the particular work.

### 8. Loss, inaccessible material and inaccessible control are not synonyms

S002, S005, S006, S009, S010, S017, S020, S059 and S100 separate physical execution outage (`OP002`), compromised authority (`OP007`), owner access wait (`OP013`), remote control outage (`OP033`) and absorbing information loss (`OP006`).

Absorbing loss requires a closed inventory: every recoverable copy of the needed information is gone. A discovered readable independent copy refutes that inventory. Losing the known key is not proof every key-recovery path is gone. Local recovery software helps only someone who can reach the machine and obtain authority; instructions behind the lost access boundary cannot supply those prerequisites.

S091, S093 and S097 additionally preserve the distinction between a durable blocker string and an intelligible handover. The operator bottleneck (`OP031`) can persist without any dynamical attraction. Independently authorized tasks may still complete.

## Unresolved cases

| Scenarios | Missing discriminator |
|---|---|
| S021, S022 | DST fold/gap occurrence identity, intended schedule semantics and misfire policy. |
| S023, S029 | Exact wall/monotonic clock primitives, suspend accounting, backwards-step handling and restart persistence. |
| S025 | Pinned versus current timezone rules and treatment of previously materialized occurrences. |
| S026 | Remote causal relationships and whether timestamp order matters to the business invariant. |
| S036 | Observation generation/sequence and mandatory caller binding; confidence is not freshness. |
| S048 | Concrete changed optional-field meaning, schema negotiation and affected invariant. |
| S069 | Retirement/tombstone transition, old-task disposition, memory retention and access semantics. |

D3's “one run per execution window” does not define a DST fold or skip policy. A logical event cursor orders local recorded facts, not remote physical acceptance times. Registration checks do not define scope retirement. These gaps should not be filled by renaming their endpoints as attractors.

## Next discriminating experiments — proposed, not run

All experiments below belong in isolated fixtures with fake participants and synthetic content, not this live instance. These are evidence requests, not residue or implementation proposals.

| Experiment | Observation that separates branches |
|---|---|
| Instrument writer cut points and a second-store cancellation hook | Whether cancel in the exact post-intent/pre-writer gap can coexist with a writer call; caller serialization must be included before claiming a product bug. |
| Restore two known snapshots, one omitting a later accepted operation | Whether local restore can distinguish never-attempted from unseen-tail tasks without independent evidence. Keep fake participant history separate. |
| Fake reconnect matrix: same/different/missing UUID and stale/new observation | Which identity and freshness combinations promote a session; whether generation filtering exists outside the sampled helper. |
| Fake runtime with a surviving descendant and unavailable observer | Whether replacement admission requires evidence about all write-capable processes, not merely pane absence. |
| Same-store concurrent assignment/alias reservation regression | Exactly one admission for a stable identity; repeat on distinct stores to expose the boundary, without starting real workers. |
| Finite shock load experiment with external arrivals then set to zero | Whether backlog drains, runs away, or settles into a distinct recurrent high-load regime; separately measure retry generation and useful completions. |
| Synthetic alarm/decision workflow trace | Whether erroneous approvals actually cause further approval load. Do not substitute a fixed error probability for observed causal evidence. |
| Fake plugin framing, giant declared length and crash-budget matrix | Allocation before/after validation, continued local progress and persistence of the restart budget across host restart. No host implementation claim before it exists. |
| Fake participant accepts then loses response/history or lies about durability | Exact local knowledge state, number of accepted effects and whether not-found is distinguishable from expired history. |
| Fake approval payload/price/content mutation and external version change | Accepted content matches actual approved terms; stale compensation cannot claim preservation merely from a returned success flag. |
| Fake clock fold/gap/backstep/suspend/tzdb-version table | Defined occurrence keys, run counts and expiry decisions, with contractual intended time made explicit first. |
| Artifact overwrite/orphan-result and case-collision fixtures | Whether original verified bytes remain available independently of path names and status strings. |
| Replay identical synthetic history under two current registries | Whether output depends on mutable registry; a pure reducer should be invariant when historical input is unchanged. |
| Synthetic retirement and unfamiliar-operator handover exercises | Exact old-task/content/access dispositions and whether a substitute can decide from retained evidence without owner-only knowledge. |

## Source register

All paths below are relative to this review directory. `C` means inspected **source code**, not an executed test. `D` means **design promise**. `scenarios.csv Snnn` identifies stipulated stressors, not observed incidents. Function names remain the primary locators if line numbers drift.

| Ref | Inspected location and claim boundary |
|---|---|
| C1 | `../../crates/factory-task/src/deliver.rs`, whole file: `deliver`, `OperatorPromptWriter`, `mark_running`, `authorise_resume`. |
| C2 | `../../crates/factory-recovery/src/restore.rs`, whole file: `reconcile`, per-row restore rules and post-commit report writing. |
| C3 | `../../crates/factory-task/src/assign.rs`, `is_idle` (around 222–259), `assign` (487–555), module/interface search. One-store assignment, not external authentication. |
| C4 | `../../crates/factory-recovery/src/reconnect.rs`, `identifies_same_session` (141–168), `workspace_exists_on_disk`, `reconnect_after_herdr_or_machine_restart` (409–533). No proof of process-tree death or caller-wide freshness. |
| C5 | `../../crates/factory-task/src/complete.rs`, `validate_result` and `complete_with_result` (88–195): byte/shape validation and transactional result write. |
| C6 | `../../crates/factory-task/src/create.rs`, `Task` fields via source search, `create` (109–174), `cancel` (301–359). Caller UUID and plain insert are not the ADR logical idempotency service. |
| C7 | `../../crates/factory-store/src/backup.rs`, whole file: `VACUUM INTO`, no overwrite and no retention deletion. |
| C8 | `../../crates/factory-store/src/pragma.rs`, whole file: WAL, FULL, foreign keys, busy timeout. `../../crates/factory-store/src/lib.rs`, `transaction` uses `BEGIN IMMEDIATE`. Hardware durability is still an assumption. |
| C9 | `../../crates/factory-store/src/migrations.rs`, `migrations` and `apply` (1–70): `rusqlite_migration` and FK checks. Not a future event-payload reducer or an executed power-cut test. |
| C10 | `../../crates/factory-session/src/lib.rs`, `find_aliasing_conflict` (413–476), `begin_start` (806–862), lease state declarations via search. Stable `(device,inode)` aliases in one store, not arbitrary later filesystem writes. |
| C11 | `../../crates/factory-adapter/src/lib.rs`, `parse_harness_session_id` (239–250), `PiAdapter::observe` (294–381): hook confidence and task-signal suppression. |
| C12 | `../../crates/factory-delegation/src/rule.rs`, whole file: registration/kinship then retained-chain membership; authenticated caller is an input precondition. |
| D1 | `../../docs/adr/0003-logical-api-and-event-sourcing-v1.md`, read completely: logical idempotency, event replay, trusted invocation and saga contracts. |
| D2 | `../../docs/adr/0007-supervised-plugin-process-protocol-v1.md`, read completely: framing, bounded supervision, safe mode, reconciliation, subscriptions and compatibility. |
| D3 | `../../../../.specs/agent-task-scheduler-design.md`, “Cron-Ausführung” around lines 80–95: minute dispatcher and at-most-one run per execution window. Only this scheduling promise is used, not unrelated older delegation prose. |

Also read completely: local `AGENTS.md`, `../REVIEW-PROTOCOL.md`, all rows of `scenarios.csv`, and project `../../README.md` for orientation. Path/source discovery excluded prohibited analysis and peer directories.

## Validation

A local Python CSV validation (not a dynamics model) passed:

- exact required semicolon-delimited schemas and UTF-8 decoding;
- every field populated and no extra/missing CSV fields;
- unique OP/OPT identifiers and no undefined destination-state references;
- exactly S001–S100 once in coverage and once in scenario trajectories;
- matching trajectory destinations and coverage IDs;
- `none` exactly for unresolved coverage, with missing dynamics stated;
- every catalogue state used by at least one trajectory.

No source tests or production fault drills were run. The present evidence supports the distinctions and falsifiable branches above, not empirical convergence or a claim that every case is solved.
