# Independent observation and timing review: S201–S275

Assigned run: `c5009bc2-2c67-42d7-874e-929d9c3a4106`  
Coordinator: `8f83af48-f692-47e6-bec2-cf69f55ac040`  
Phase: state-first discovery only.

## Result and limits

All 75 neutral scenario rows have an explicit trajectory and coverage disposition. The analysis produced 36 state descriptions, without a prescribed count. Four scenarios remain unresolved: **S218, S224, S230, S274**. Their trajectories explain what can be inferred and what additional dynamics are missing; their coverage state IDs are `none`.

**No actual Factory attractor was empirically established.** This is static source inspection, design-contract reading, and conditional qualitative analysis. No production measurements, fault injections, model stress tests, alerts, external actions, or executable dynamical models were performed. The arithmetic examples are stated assumptions, not measured workloads. No implementation was changed. No old residuality analysis or peer report was read. No residues are proposed.

The CSVs are semicolon-delimited UTF-8:

- `states.csv`: mechanisms, persistence classification, entry conditions, exits, surviving capabilities, losses, falsifiers, and evidence.
- `trajectories.csv`: `OBT201` through `OBT275`, one per input row. A row contains conditional branches rather than asserting a single inevitable endpoint.
- `coverage.csv`: exactly one disposition per input ID. State IDs include the relevant branch states, not just the worst outcome. Coverage is not a claim that the scenario is solved.

`source-supported` in coverage is deliberately narrow: S203 has an explicit restore branch, S245 a disconnected-lease branch, S256 a journal-before-writer crash gap, and S260 a direct-library counterexample to false stop acknowledgement. It does **not** mean the full wake-up system, process fencing, UPS behavior, or CLI is implemented or tested. Other rows can cite source while retaining `conditional` status because their principal dynamics remain hypothetical. There are no `toy-supported` rows.

## State distinctions that matter

### Silence is not one state

S201–S207 and S257 concern loss of an observation path, not necessarily loss of durable data. Separate hardware can share a power outlet; separate providers can share an account; a distant alarm transport can still rely on the one unavailable human. A dead observer cannot establish that Factory is alive. Conversely, independently delivered outage evidence ends total blindness while the host can remain completely off.

OB02 is externally maintained blindness/outage, OB04 is the wait between detection and human receipt, and OB05 is an administrative mute. None needs autonomous feedback. A ten-year suppression setting is a policy hold, not evidence of a basin of attraction. A send acknowledgement is not inbox placement, human acknowledgement, or corrective action.

OB03 is different: evidence positively claims health while useful service is absent. A free health thread, stale proxy response, replayed heartbeat, or aggregate across healthy scopes can all generate this mismatch. Each has a different falsifier. A source-fresh heartbeat still need not prove commits work; a successful read still need not prove writes work. Even source-attributed lifecycle evidence is not by itself business-progress evidence.

Clone identity in S215 is OB19: two sources can be alive while attribution is wrong. Key mismatch in S270 is OB20: both sources may be legitimate but unable to authenticate each other. These are not equivalent to a crashed host. Network restoration alone need not resolve either identity collision or incompatible key epochs.

### Legitimate waiting, unbounded holding, and feedback are different

An approval wait (OB07), unknown-effect wait (OB16), and retained ambiguous lease (OB06) have no demonstrated restoring feedback. Their exits require a decision or new evidence. Calling them attractors would obscure what is missing.

S212 has a conditional *human feedback* branch: nuisance alarms cause dismissal, dismissal leaves the false criterion uncorrected, and subsequent nuisance alarms reinforce dismissal (OB34). This requires observed responder behavior. Mere noise is insufficient. Legitimately blocked work also does not prove the underlying service is healthy.

S225's all-of barrier is OB13, not automatic system collapse. It becomes accumulating slot loss OB10 only when it retains finite resources and similar requests continue. In S228, positive-probability permanent hangs can eventually consume a finite pool under continued arrivals. Stop new arrivals and growth stops, although existing occupied slots remain. Pool size, hang probability, rate, and reclamation determine the timescale; the scenario does not supply years-to-exhaustion arithmetic.

Strict priority under endless emergency arrivals (OB14) is externally maintained starvation. Stop emergency demand and ordinary service can resume. A finite huge export is a finite obstruction, not an autonomous loop. Late descendant writes (OB18) can continue for the stated two days without any self-reproduction.

### The strongest feedback hypotheses are conditional

- **OB11, overload maintained by retries:** entry requires offered work to cross sustainable capacity and timeouts to generate enough additional work. Slow service produces retries, retries consume capacity, and failures persist after the original provider fault ends. Its basin depends on backlog, retry budgets, and the service curve. Exit requires that admitted plus retry work fall below available capacity long enough to drain. A finite bounded retry tree may simply drain and refute an attractor claim.
- **OB22, compromise reproducing trusted evidence:** the attacker must control both a persistence/update path and the evidence used to approve recovery. A clean-looking in-domain restart can then reinstall the compromise. A signed malicious one-shot update alone establishes neither this loop nor persistence. Independent clean recovery that stays clean despite the alleged persistence path refutes it.
- **OB34, alarm fatigue:** the behavioral assumptions above are load-bearing. Independent response to actionable alerts despite unchanged noise refutes the proposed loop.
- **OB36, recursive work expansion:** admitted children generate more admitted children. This is a growth hypothesis, not a stable attractor claim. Source delegation forbids a repeated scope within an inherited chain, limiting depth; finite depth can still permit a very large branching tree. A global tree budget was not established by inspecting the chain rule.

S238, S240, S252, and S253 initially describe finite synchronized cohorts (OB12/OB30). They enter OB11 only if they generate further load or keep an ongoing demand stream above capacity. S255 supplies physical thermal feedback, but sustained input maintains the low-frequency regime or thermal cycle (OB31). Heat does not automatically rewrite contractual deadlines.

## Time and correctness trajectories

Eight serial nine-second operations consume 72 seconds; eight overlapping waits need not. S219 therefore branches on the actual critical path rather than adding every nested timer. S220 distinguishes time since creation from time since dispatch. S221 distinguishes 200 milliseconds of compute from twelve years awaiting approval. All can lose business timeliness without being an unstable dynamical system.

Completed-only dashboards and closed-loop load generators can hide the uncompleted denominator (OB15). Their observations can remain perfectly accurate about the subset they actually measured. That does not support a whole-system claim. The inspected task listing includes every status; a completed-only dashboard must introduce a filter somewhere else.

S224 is unresolved as a long-term system trajectory, but the statistical objection is specific. Under independent stationary Bernoulli trials, zero failures in 1,000 observations gives a one-sided 95% upper failure-probability bound of

`1 - 0.05^(1/1000) ≈ 0.002991`,

not `0.00001`. A latency quantile and time availability also have different denominators. Neither a thousand quick results nor this bound establishes production stationarity, includes hung requests, or identifies a persistent regime.

S218's debug mode may remove a race by changing scheduling, contention, or exposure. S230's repeated collection/deadline overlap may be phase locking, sampling aliasing, biased observation, or chance. Neither has enough timing detail to choose among them.

## Late effects and retained knowledge

A local durable attempt followed by a nontransactional effect has a gap in which recovery cannot infer acceptance. Source delivery explicitly has this shape, although the shipped writer is a **manual operator handoff**, not an external invoice/mail participant. The source prevents an unauthorized second prompt attempt; it does not demonstrate remote exactly-once execution.

S231 and S251 require separate local and participant retention horizons. Erasing local evidence (OB28) can make a late reply unclassifiable without creating a duplicate by itself. A duplicate additionally requires another accepted effect and failed deduplication at the participant. Conversely, a participant ledger can disambiguate a reply after local history expires.

S233 adds a genuine retention conflict: legally deleted associations must not be silently reconstructed or retained forever just to simplify recovery. A late sensitive payload can remain unassociated, be rejected at a boundary, or be leaked through logging or guessed routing. These are separate branches. The analysis does not assume a lawful retention policy that was not supplied.

S234's microsecond cancellation race is decided at the external acceptance boundary, not by the local cancel timestamp. An unwanted irreversible action is absorbing historical loss OB17, even if future work stops correctly. Compensation can mitigate consequences but cannot make a delivered message never have been delivered.

S235 and S245 connect timing to authority: a parent can die while descendants still write, and a lease TTL cannot establish that those descendants lost authority. Source `stop` relies on caller-confirmed process unusability. Releasing a lease on elapsed time alone is a counterfactual branch into OB32, not current source behavior.

## Capacity and security boundaries

A valid JSON field can still be an invalid-size protocol message. A bounded queue per subscriber can still have excessive aggregate memory with an unbounded subscriber count. Registry capacity does not imply context-window capacity. Source context compilation accepts explicit context files and reports bytes; it neither enumerates all registered names nor enforces a harness-sized cap.

Full replay may be correct, side-effect-free, and far too slow for the recovery-time objective. The three-week replay is finite under fixed input. If new event production outruns replay, nonconvergence needs an additional rate assumption. Restore feasibility depends on the **peak simultaneous** original/export/scratch/index live set, not just final database size. The source backup creates a `VACUUM INTO` destination and refuses overwrite; that is not a demonstrated multi-terabyte staging plan.

Authentication is not the same as authorizing the specific scope credential, recipient identity, immutable operation, or approval. An imported task ID, plausible log confirmation, or artifact instruction can be valid data while supplying no authority. Source context headings and byte stability are presentation properties, not prompt-injection enforcement. Suspicious text only becomes confidentiality loss if effective read and egress capabilities let it do so.

ADR 0007 explicitly acknowledges that same-user subprocess separation is not a complete security sandbox. Thus scope API authorization alone cannot refute S273's direct OS-read branch. S274 is unresolved: sharing CPU and queues does not establish useful inference without an attacker observation model and evidence of accuracy beyond the prior baseline.

A valid update signature establishes origin, not benign behavior. Shared updater authority can destroy monitoring independence even when its power and network are independent. Granting the observer shell access changes it from a witness into a possible actuator. The mere grant is not proof of compromise, but compromise can then cross a much larger boundary.

S271 and S272 are deliberately asymmetric: readable ciphertext with permanently lost keys can mean irrecoverable authorized history (OB24), while a future breakthrough against an adversary's retained copy can mean irreversible secrecy loss (OB21). Present inability to find a key is only a wait unless all feasible recovery paths are actually excluded.

## Evidence ledger

Paths below are relative to this directory. CSV `C` references are **existing source inspected read-only**, not claims that the whole architecture is implemented. Symbols identify the relevant inspected portions. `D` references are **design promises**, not runtime evidence.

| Ref | Path / inspected boundary | What it supports and does not support |
|---|---|---|
| C1 | `../../README.md` and the `../../crates/` layout | Libraries exist and README says no binary. Its slice-status summary is stale relative to inspected task/session/recovery crates; it is not a deployment inventory. |
| C2 | `../../crates/factory-recovery/src/restore.rs`, `reconcile` | Attempt-aware task reconciliation and retained disconnected leases. Explicit call, not automatic wake behavior. Report is written after database commit and can fail separately. |
| C3 | `../../crates/factory-store/src/lib.rs`, `connection`, `transaction` | Separate read access and BEGIN IMMEDIATE mutation transactions. No measured SSD failure or end-to-end service health. |
| C4 | `../../crates/factory-task/src/create.rs`, `list`, `cancel` | Lists every task status. Running cancellation records a request and stays running. Commit errors propagate before success. No external cancellation fence. |
| C5 | `../../crates/factory-task/src/deliver.rs`, `deliver`, `authorise_resume`, `OperatorPromptWriter` | Durable attempt before writer and outcome afterward. Per-authorization attempt guard. Human resume adds one authorization. Writer is manual handoff. |
| C6 | `../../crates/factory-recovery/src/evidence.rs`, `may_promote_from_disconnected` | Authoritative confidence required by this gate. It explicitly does not decide identity correlation or session liveness. No heartbeat protocol guarantee. |
| C7 | `../../crates/factory-store/src/pragma.rs`, `apply` | WAL, foreign keys, 5,000 ms busy timeout, synchronous FULL. Busy timeout is not an I/O deadline or proof of power-loss behavior on arbitrary media. |
| C8 | `../../crates/factory-session/src/lib.rs`, `holds_lease`, `count_live_sessions`, `begin_start`, `transition`, `stop` | Disconnected holds lease and capacity; stop bookkeeping commits before success and relies on caller-confirmed unusability. No OS process-tree fencing demonstrated. |
| C9 | `../../crates/factory-context/src/lib.rs`, `compile`, `SourceReport` | Explicit source chain, no knowledge-link walk, full text allocation, reported byte counts, no context-size limit. No automatic registry-name expansion. |
| C10 | `../../crates/factory-store/src/backup.rs`, `backup_to` | VACUUM INTO copy and refusal to overwrite. No twenty-year replay or peak-space guarantee. |
| C11 | `../../crates/factory-delegation/src/rule.rs`, `check` | Registration/kinship and already-in-chain refusal. Does not imply bounded tree width or an OS security boundary. |
| C12 | `../../crates/factory-context/src/format.rs` | Section formatting and newline normalization. Not trust enforcement against adversarial content. |
| D1 | `../../docs/adr/0003-logical-api-and-event-sourcing-v1.md` (read fully) | Logical API, committed response cursor, append-only replay, scoped idempotency, saga reconciliation, immutable inputs and trusted invocation context. |
| D2 | `../../docs/adr/0007-supervised-plugin-process-protocol-v1.md` (read fully) | Supervised subprocesses, frame limits, capability binding, deadlines, bounded restart backoff, cursor subscriptions/backpressure, same-user sandbox limitation. |

Inspected libraries directly manipulate durable SQLite rows and delivery journals. Those facts must not be silently relabeled as proof of the broader canonical event-store/outbox/plugin-host architecture in D1/D2. Likewise the compatibility task tool used to record this review is not evidence that the promised production daemon or all its transports exist. No claim about absence throughout every uninspected source file is intended.

## Next discriminating experiments — proposed only

All experiments below would require a separate authorized task, isolated fixtures, synthetic content, and no real notifications or external effects.

1. **Observation truth table:** simulate source power loss, observer loss, cache replay, two clone epochs, key rotation, and unavailable human receipt independently. Record what exact evidence advances freshness and which point is called delivered. This separates OB02/03/04/19/20; it does not merely test a green endpoint.
2. **Critical-path and measurement experiment:** feed synthetic admitted timestamps, completed timestamps, and permanently open requests to a dashboard/test harness. Compare completed-only, closed-loop, and fixed offered-arrival views. For S219 vary serial versus overlapping work. For S218/S230 preregister exposure and phase changes so absence of a race is not confused with absence of opportunity.
3. **Fault-removal load experiment:** use a bounded queue simulator with explicit pool sizes, retries, deadlines, charging, and arrivals. Remove the original provider fault and then separately stop new arrivals. Measure whether retry work decays or maintains overload. Sweep the initial backlog to distinguish an entry threshold from an inevitable endpoint. No toy results are claimed here.
4. **Irreversible participant stub:** use a synthetic acceptance ledger and no external effect. Enumerate crash/cancel before acceptance, after acceptance, before local outcome commit, and after both local and participant history expiry. Count accepted operation identities rather than just responses. Add legally erased associations without sensitive content and verify whether the late item is unknown, rejected, or wrongly attributed.
5. **Control versus data experiment:** simulate bulk output, hung calls, and stalled subscribers while issuing a synthetic stop. Measure control service, total retained bytes, and actual descendant authority after parent termination. Distinguish per-queue limits from total limits and process exit from inability to write.
6. **Recovery capacity accounting:** create a small synthetic dataset with known byte growth and enumerate original/export/scratch/index overlap. Separately measure replay throughput with fixed and continuing input. State scaling assumptions rather than extrapolating a small benchmark into a twenty-year RTO guarantee.
7. **Authority table with inert canaries:** submit wrong-scope credential references, swapped recipient mappings, imported approval IDs, forged log confirmations, and artifact-contained action requests to non-effectful stubs. The outcome to measure is whether a trusted action binding changes, not whether a model repeats malicious words.
8. **Trust and side-channel counterexamples:** in isolated synthetic OS principals, test whether API-denied direct reads are also OS-denied. For S274 declare attacker-visible timing, victim schedule, baseline prediction, and false-positive tolerance before collecting data. For updater independence, model compromise of one signing/update authority and test whether the supposedly independent witness and clean recovery source remain outside it.

These experiments discriminate assumptions and persistence mechanisms. They are not a selected repair architecture or a phase-2 residue proposal.

## Artifact validation

Local CSV parsing verified UTF-8 semicolon formatting, exact required headers, unique state and trajectory IDs, exactly S201–S275 in both trajectory and coverage order, valid state references, and a coverage reference for every defined state. Dispositions: 67 conditional, 4 narrowly source-supported, 4 unresolved. Source paths in the state table resolve locally. These are artifact-consistency checks, not runtime or hypothesis-validation tests.
