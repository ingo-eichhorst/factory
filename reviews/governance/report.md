# Independent state-first review: governance and change, S101–S200

Run: `5438a2d2-2cce-4c1f-ada8-57f9d5ad47ae`  
Coordinator: `8f83af48-f692-47e6-bec2-cf69f55ac040`  
Review date: 2026-09-09

## What this review establishes

All 100 neutral scenario rows have an explicit disposition. There are **48 state descriptions and 100 scenario-specific conditional trajectories**, not 48 proven attractors or 100 solved incidents. Coverage comprises 89 conditional entries, six narrowly source-supported entries, and five unresolved entries. There are no toy-supported or empirically established production attractors.

The central finding is that **remaining able to store or process tasks does not establish remaining able to decide legitimately or know correctly**. An intact database can coexist with lost human authority, circular false evidence, unlawful retention, meaningless historical language, or economically pointless execution. Conversely, a stopped workflow can preserve a valid boundary without being an attractor or a recovered business.

This is phase 1 only. No residue categories or proposals were derived. No peer reports or prior residuality analyses were read. Source was inspected read-only. No live fault, runtime operation, external action, software build, production test, or recovery drill was performed. The local Python program only serializes and structurally validates these review files; it is not an executable model of Factory dynamics.

## Reading the artifacts

- `states.csv` records entry conditions, sustaining mechanisms, exits, surviving capabilities, losses and falsifiers. State IDs are `GV01`–`GV48`.
- `trajectories.csv` contains one row per scenario, `GVT001`–`GVT100`, corresponding to `S101`–`S200`. Initial states are explicit prose assumptions. Sequences contain conditional branches rather than asserting a single inevitable destination.
- `coverage.csv` has exactly one row per input. Its state IDs are the union of that scenario's branches, **not a claim that every listed state occurs simultaneously or inevitably**. Some states can coexist: a budget hold and missing approval authority are independent constraints.
- `none` means that the supplied scenario does not yet support selecting a destination. A conditional row can still contain substantial unknown dynamics; only its stated branch has been analyzed.
- `build_review.py` is the reproducible serializer and `validation.txt` records its checks. Passing those checks means the ledger is well formed, not that Factory is resilient.

State counts arose from separating materially different mechanisms. The register is neither a mutually exclusive partition nor a minimal basis. A general permission hold does not replace the distinct questions of who may decide, what evidence exists, and what law permits. These differences determine different exits.

## Evidence boundary: source is not the architecture promise

References in CSV evidence use the following labels. Paths below are relative to the Factory project root; scenario references mean `reviews/governance/scenarios.csv`. The inspected checkout reported HEAD `e93597f4145306a51c1867f1dfb9a2a505df7383`. This was a working-tree inspection, not a claim that every inspected byte was committed or that this version runs in production. `source-snapshot.txt` fingerprints the selected files as read during review, without copying their contents.

| Label | Inspected material | What it supports, and what it does not |
| --- | --- | --- |
| C1 | `crates/factory-context/src/lib.rs`, particularly `compile` | Reads explicit source files at compilation time, returns `UnreadableSource` on failed reads, follows no knowledge links, appends supplied task text, reports section bytes without imposing a model-window bound. Does not establish when a scheduler compiles, immutability between queue and start, secret filtering upstream, or harness truncation behavior. |
| C2 | `crates/factory-task/src/complete.rs`, `validate_result`, `complete_with_result`, cancellation acknowledgement | Requires result content and bounds byte size; stores submitted artifact path strings without verifying their existence. Running cancellation acknowledgement requires a recorded request. Does not prove all callers lack verification or that a recorded request forcibly stops a worker. |
| C3 | `crates/factory-recovery/src/restore.rs`, `reconcile` | Uses only the restored database. Running tasks and queued tasks with a recorded attempt become blocked/interrupted; queued tasks without an attempt remain queued with assignment cleared; lease-holding sessions become disconnected and retain leases. Repeated reconciliation leaves already reconciled rows unchanged. Does not observe post-snapshot external effects or fresh revocation. The report is written after transaction commit and can fail independently of committed reconciliation. |
| C4 | `crates/factory-store/src/schema.rs:1–15`; `migrations.rs`, including `apply` | Schema commentary explicitly states that the current tables are mutable, not event-store projections. Five forward SQL migrations are listed. This is not evidence of implemented event upcasters or side-effect-free domain replay. Old historical binary compatibility was not tested. |
| C5 | `crates/factory-store/src/backup.rs`; selected `lib.rs` methods | `VACUUM INTO` snapshots the SQLite store and refuses existing destinations; the backup function does not implement retention deletion. Library callers can open a database and access a local connection. This does not prove complete business-content backup, independent keys, OS isolation, or root-resistant evidence. |
| C6 | `crates/factory-delegation/src/rule.rs::check`, `queue.rs::queue_from_session` | Rejects a target already in inherited lineage and checks scope relationship. A running task supplies the inherited chain. This bounds revisiting a scope within one lineage, not aggregate fan-out, arrivals, resource use, or separately introduced roots. |
| D1 | `docs/adr/0003-logical-api-and-event-sourcing-v1.md`, sections 7–9 | **Design:** canonical append-only events, pure upcasters, replay without effects, retained replay content, operation-specific sagas, preconditions, authenticated invocation context and sensitive-data references. These were not credited as current executable safeguards. |
| D2 | `docs/adr/0007-supervised-plugin-process-protocol-v1.md`, sections 6–7 and Consequences | **Design:** transport authenticates the peer, kernel authorizes that supplied context, plugin activation follows validation. The ADR itself warns that same-user process separation is not a full sandbox. No running plugin host or updater was inspected. |
| D3 | `docs/adr/0020-run-telemetry-and-the-evaluation-bench.md`, decisions 1–5, 8–9 and Open items | **Design:** fixture-owned executable verdicts, pinned comparison tuples, incompatible cross-instrument friction metrics, bounded production excerpts rather than full transcripts. Attribution across model switches is explicitly open. This is not proof of present telemetry collection or redaction. |
| R1 | `README.md` | Documents subtree publication and says later slices are absent; inspected task, delegation and recovery libraries plus five migrations show that implementation-status prose is stale. Publication instructions do not prove a publisher used the correct boundary. |
| Q | Neutral scenario plus explicit qualitative assumptions | Organizational, legal, economic, epistemic and long-horizon reasoning. No sampled production behavior and no adjudicated legal conclusion. |

The six `source-supported` coverage rows are S105, S111, S112, S114, S158 and S191. This status applies to the specific static boundary described in their reason cells, not to all long-term branches in the same trajectory. In particular, S191 does **not** report an observed duplicate effect.

## Key trajectories and competing interpretations

### 1. Agreement can destroy the means of detecting error

S102–S104, S110, S116, S157, S184, S195 and S200 distinguish two loops:

- **GV05, self-confirming false knowledge:** an accepted error becomes a source; later reviewers cite it; contradictory observations lose authority. The initiating model need not remain present once copied false claims carry the loop.
- **GV06, captured evaluation:** passing a known or defective check earns trust; trust removes independent tests; the missing tests make the favorable claim harder to challenge.

Two reviewers sharing a blind spot are not independent evidence. Replacing a model judge with an executable fixture removes one dependency, but a recognized fixture or intentionally unsafe oracle can still reward the wrong behavior. S195 compounds format drift with correlated verification: a parser may reject the format immediately, while a permissive parser plus same-family reviewer can accept wrong meaning.

**Competing interpretation:** a single wrong answer is a transient error. A benchmark specialized to its honestly narrow claim can be valid. Neither becomes an attractor merely because it passed once. Entry to the hypothesized basin requires authority to reuse the error and exclude corrective evidence. Exit requires an independent observation that the community actually allows to revise its beliefs—not simply a third reviewer sharing the premise. S184 does not say every physical experiment fails; it says all available reviewers share the mistake.

S200 applies reflexively here: marked coverage cells describe dispositions. Stopping tests because this file covers every ID would instantiate precisely the unsupported assurance mechanism under review.

### 2. Changed behavior can remain useful but unmeasurable

S102, S107, S109, S111, S116, S150 and S166 can enter GV07 without producing a bad task result. A stable alias is not immutable weights; a URL is not historical content; a per-run model field is not attribution of every call; token counts are not prices. A fleet replacement creates GV35 when the instruments' meanings change.

Current compilation reads the files when `compile` runs. This supports two incompatible S111 branches: late compilation sees changed root instructions, while an earlier cached compilation may retain the old ones. The inspected function does not choose which policy revision must govern queued work. Likewise, its byte accounting does not establish which input a shrunken harness window actually consumes.

**Exit:** independent time-specific identity evidence, or a deliberately narrower comparison. Rerunning today cannot reconstruct an uncaptured historical model or page. Correct output and attributable output remain different claims.

### 3. The kernel boundary is not automatically the security boundary

S117–S128 and S172/S177 distinguish an attempted instruction, accepted privileged execution, false records, latent copies and actual disclosure. These are not one universal compromised state.

A same-user subprocess can fail to isolate credentials or SQLite even if protocol commands enforce scope checks. D2 acknowledges this limitation. A transport that supplies principal identity is part of the authentication trust boundary: validating scopes against its fabricated actor does not prove which human approved. Root-resistant proof needs a trust basis outside what the hostile root controls; a local append-only convention is not such evidence.

S122 is about binding checked bytes to executed bytes across replacement. S159 moves the problem earlier: verification after activation cannot recall effects of already executed code. No current updater behavior was observed.

**Competing interpretation:** a rejected native config, safely bound SQL parameter or inert escape display need not cause compromise. S125 is therefore unresolved without its actual sink. Similarly, a secret merely present in a restricted backup is GV19, not necessarily GV08. Once an unauthorized party has actually learned it, later deletion cannot restore the never-disclosed past. Credential rotation can end future authentication power without reversing disclosure of other information.

### 4. Valid local history can authorize the wrong present

S127, S191 and S198 expose the snapshot boundary. Current `reconcile` is conservative about attempts **that its snapshot contains**. It cannot know that an apparently unsent queued action was executed after the snapshot, or that a credential was revoked later. Restoring a consistent old database can therefore leave GV43 if local historical evidence is treated as fresh authorization.

The competing branch is GV13: an attempt present in the snapshot leads to blocked/interrupted work and retained disconnected leases. That is a source-supported policy fixed point. It is not evidence of spontaneous recovery or a production attractor. Repeating the same function cannot produce the missing external fact.

S198 adds a backup clone starting a legacy dispatcher. A local database lock cannot fence a dispatcher acting from an independent copy. Two internally consistent databases can still issue conflicting external effects. S197 is different: even a correctly identified old operation can have an unsafe inverse after a human edit and plugin change. A conditional inverse can hold at GV44; an unconditional overwrite can create irreversible loss. A new compensating action is not time travel.

### 5. Authority, knowledge and money can fail independently

S161–S169 and S189 concern who may decide, not only who has a credential. A founder's permanent absence yields GV14 if no legitimate successor exists. Two plausible executives or successors give GV15 until an accepted rule settles the concrete effect. Old password possession does not adjudicate a company sale or split. Successful succession can instead produce the ordinary organizational transition GV33.

S193 makes these dimensions simultaneous: Herdr absence prevents observation, retained leases exclude reuse, the sole operator's death removes authority, and budget exhaustion removes affordable execution. Runtime recovery alone does not exit the other holds. Automatically dropping leases would not supply legitimacy or prove that work stopped.

S144 and S167 show a potentially self-sustaining organizational loop, GV16. When approval costs exceed task value, legitimate nonuse is economically rational. When rewards instead depend on a low blocked count, shortcuts can improve scores and become the accepted way of working. Independent consequences and valid measurements determine whether this normalization persists. Merely reducing unnecessary approvals is a beneficial competing explanation and must not be called bypass without evidence.

### 6. Legal duties can constrain the use of surviving information

S131–S139, S164 and S194 cannot be answered by saying that immutable history survives. The applicable law, jurisdiction, effective time, purpose and exact data must be known. A request is not itself a conclusion that every byte must be destroyed; a legal hold is not permission for all future use.

GV17 records constrained disposition, whereas GV18 records missing indispensable provenance after lawful deletion and no independently recoverable representation. S194 is especially sharp: a complete manifest and hash cannot reconstruct the only content copy after it is removed. If that content is unnecessary for the requested reducer, incomplete content need not prevent that narrower computation. This distinction is missing from the stressor and remains conditional.

S164 can preserve project code while losing the legal right to use company context, GV34. Recopying the prohibited context is not recovery. S136 may retain an actor field while lacking proof of the particular approving human. S186, read literally as deleting all information needed for eternal proof while preserving that proof, has no joint technical solution. No legal precedence was invented to make it converge.

### 7. Load, economic pressure and internally generated work are different

GV10 describes capacity exhaustion sustained by continuing demand. A finite burst can drain. GV11 requires work reproduction or retry feedback that remains above replacement; a backlog counter by itself does not establish this. S130 and S196 explicitly respect the current chain gate: the same lineage cannot ping-pong between two scopes. Many new roots, broad trees or repeated schedules can still be large without a graph cycle. Cron catch-up behavior was not inspected.

GV23 adds a different feedback: a dominant scope's output earns future allocation and political leverage, starving others of both capacity and evidence of value. Without that allocation mechanism, S143 is ordinary contention, not capture. Refused budget disclosure in S168 may be a legitimate confidentiality claim; independently admissible resource accounting could settle allocation without revealing the budget.

GV21 and GV22 distinguish an enforced spending hold from unknown postpaid liability. A zero local budget does not unspend an accepted provider call. S148's late result may be technically correct and economically valueless: GV24 is a missed opportunity, not a stable attractor. A guarantee of real-time response on one host likewise requires an explicit outage and timing contract; a request does not establish capability.

### 8. Long-term survival has several different boundaries

- **Bytes versus meaning:** S183 can retain text but lose the language and tacit references needed to interpret it, GV39. Fluent invented expansions are not recovered historical intent.
- **Copies versus usable recovery:** S129, S181 and S199 can enter GV48 when no known copy/key/decoder combination recovers essential content. Surviving ciphertext, metadata and memories mean these are not automatically total information disappearance. A missing key is not proof against every future recovery method.
- **Total disappearance:** only the full S190 premise supports GV42 directly. It removes every system-specific trace and memory. A new unrelated installation cannot establish continuity with what vanished.
- **Authenticity versus bytes:** S188 withdraws cryptographic assurances, GV41, without itself proving an actual forgery or decryption. A new primitive cannot prove old ciphertext was never read.
- **Correctness versus timeliness:** S178 can finish a correct finite replay after RTO, GV47. Snapshot availability does not prove that validation, tail replay and content retrieval fit the deadline.
- **Externally forced cycles:** S187 supports GV40 only if boot plus useful checkpointable work fits the five-minute energy window. If startup consumes every window, there is no useful recurrent service. The power schedule, not an internal loop, supplies recurrence.
- **Ordinary lifecycle completion:** S179 can reach GV46 when a usable permitted export is independently accepted and all required Factory services stop. A full database dump may omit artifacts or their meaning. Shutdown does not itself establish remote-copy erasure.

## Which descriptions are attractor hypotheses?

Only GV05, GV06, GV11, GV16, GV23 and GV29 are explicitly labeled **attractor hypotheses**. Each specifies a feedback and an entry basin in qualitative terms: evidence reuse, assurance-driven test removal, supercritical work reproduction, rewarded bypass, allocation capture, or complexity-driven service expansion. These hypotheses may describe a coarse organizational regime persisting after the initiating shock, but there is no observed return after perturbation, measured basin, probability of entry, equilibrium, or convergence proof.

Positive feedback can instead produce a runaway that exhausts resources, an unstable transition, or a finite episode. Those alternatives remain open. GV12 is a separate idealized nontermination hypothesis; finite physical budgets interrupt literal endless execution. GV09 requires a continuing attacker foothold to persist; one injected output is not durable attacker control. GV45 is a mutual-wait deadlock under strict startup prerequisites, not a demonstration of attraction from nearby states.

Policy holds, missing-evidence waits, obsolete dependency islands, custody constraints and exposure risk are deliberately not relabeled attractors. Terminal losses need no feedback. A surviving file, permission rule or hypothetical exit is not by itself evidence of a persistent surviving structure.

## Explicitly unresolved scenarios and remaining gaps

| Scenario | What must be supplied before selecting a destination |
| --- | --- |
| S125 | Exact plugin parameter sink, SQL binding or interpolation, path resolution, allowed root and effective privileges. Existing quote escaping elsewhere does not settle this new parameter. |
| S174 | Smartphone acceptance contract: remote-client use versus local autonomous service, allowed daemon placement, suspension behavior, offline work and required latency. |
| S175 | Active-active contract: partition behavior, permitted unavailability, mutation/effect ownership, fencing, quorum, latency and failover authority. |
| S185 | Whether apparently contradictory world states share object identity, jurisdiction and effective time, and whether the clones are required to agree at all. |
| S186 | Binding definitions of deletion and proof, applicable legal precedence and any permitted retained representation. Literal incompatible obligations remain incompatible. |

Other conditional rows still lack decisive details. In particular, no provider contract, real price series, fairness scheduler, plugin resource measurement, regulatory determination, real-time plant model, cryptographic trust inventory or organizational incentive history was available. No statement of “not found in inspected source” should be generalized to the entire project or operational compatibility tooling. This is not a comprehensive implementation audit.

## Next discriminating experiments — proposed only

These are ways to distinguish the branches, not residue proposals or authorization to change a live system. Use synthetic data and disposable files under this review scope for any separately assigned local exercise. Any use of people, providers, real credentials, actuators or external systems requires separate approval.

1. **Context timing and validation:** in an isolated harness fixture, change a synthetic AGENTS file between queue and start, make an included file unreadable, add cyclic note links, and vary the simulated model window. Record exact compiled and consumed bytes. This separates C1's actual compiler behavior from caller caching, stale fallback and truncation assumptions.
2. **Completion and independent truth:** submit a nonexistent synthetic artifact path to the library boundary and separately inspect any caller verification. For S104/S110/S195, compare shared-model acceptance with a withheld semantic oracle and a strict parser. A parser-only failure and a plausible wrong answer predict different destinations.
3. **Historical snapshot versus world:** model a mock participant ledger and revocation timeline outside a copied synthetic store. Restore a snapshot before an already executed action and revocation, comparing it with a snapshot containing the delivery attempt. Observe whether any proposed dispatcher relies solely on snapshot-local absence. Do not send an actual effect.
4. **Governance tabletop:** ask authorized representatives, under a separate task, to apply explicit succession and conflict rules to S161/S162/S165/S189. Distinguish lack of legal authority from missing external evidence. Measure whether resolving one actually permits the decision or leaves the other unresolved.
5. **Incentive and evaluation evidence:** use synthetic longitudinal decisions first, then separately approved organizational evidence if available. Track whether favorable scores really remove independent tests, bypass brings reward, or occupancy determines future allocation. Persistence after the initiating workload ends would discriminate GV06/GV16/GV23 from a short pressure response.
6. **Finite work versus reproduction:** an isolated analytical model could vary finite scope count, roots, fan-out, retry identity and catch-up duration. Stop external arrivals and count remaining admitted obligations. Drainage refutes a self-maintaining work loop; new work created by the backlog itself supports only the explicitly stipulated feedback. No such model was run here.
7. **Legal/content dependency exercise:** with invented data and an explicit hypothetical legal ruling, enumerate which requested proofs and reducers require content subject to deletion. A manifest-only restore must not be scored as content recovery. Real legal interpretation remains a human legal question, not a simulation output.
8. **Version/cutover/compensation fixture:** if historical binaries are lawfully available, use synthetic databases and a fake external participant to compare refusal, semantic misread, cloned dual dispatch and stale inverse refusal after a human edit. Test executable identity across a simulated upgrade rather than trusting a version label.
9. **Recovery and retirement evidence:** inventory synthetic content, keys, decoders and interpretation aids, then try an independently readable export with no Factory service. Separately measure complete recovery time at representative sizes and constrained duty cycles. Losing only a decoder, only a key and every trace should produce different outcomes.
10. **Security-boundary analysis before fault tests:** identify the actual transport identity binding, plugin execution identity, same-user file privileges, renderer and injection sink. A harmless string should remain harmless without calling it an exploit; any later isolated adversarial fixture must exercise the exact discovered boundary.

## Validation and handoff

The serializer checks exact S101–S200 input and coverage identity, unique state/trajectory IDs, nonempty fields, allowed status vocabulary, referential integrity, state use, matching trajectory/coverage destinations, explicit `none` for unresolved rows, exact column schemas and UTF-8 semicolon CSV round-trip. A separate review check confirms that every state ID mentioned in a trajectory sequence is listed among that row's possible destinations.

These checks establish artifact consistency only. The next reviewer should challenge the feedback assumptions and legal/organizational premises before accepting any persistent regime, and should keep source-supported transitions separate from system-level survival claims. No claim of full resilience is made.
