"""Serialize the hand-authored phase-1 analysis; not a simulation or executable dynamics model.
Run only from this reviewer directory. Inputs and outputs remain here.
"""
import csv
from pathlib import Path

ROOT = Path(__file__).resolve().parent
STATES = []
TRAJECTORIES = []
COVERAGE = []


def state(n, name, kind, conditions, feedback, entry, exit, survives, lost, falsifier, refs='scenarios.csv', evidence='Qualitative conditional hypothesis from assigned scenarios; not observed in Factory.'):
    STATES.append([f'BD{n:02}', name, kind, conditions, feedback, entry, exit, survives, lost, falsifier, evidence, refs])


state(1, 'Verified bounded completion', 'ordinary completion',
      'Specific operation has current authority, correct subject and meaning, and independently adequate effect evidence.',
      'None required. Completion is an endpoint for this operation, not an attracting operating regime.',
      'Resolve discrepancies before effect, or reconcile the actual effect with adequate independent evidence.',
      'New contradictory evidence reopens the assessment; new work is a different operation.',
      'Accountable result and usable evidence for the bounded claim.', 'No guarantee about unrelated operations or future effects.',
      'An independently verified mismatch in recipient, units, authority, or effect refutes the completion claim.')
state(2, 'Explicit validity hold', 'policy hold',
      'A functioning gate refuses ambiguous identity, version, freshness, or authority before the next effect.',
      'No autonomous restoring force. A retained refusal condition keeps work held.',
      'Mismatch is detected while the relevant gate remains in control.',
      'Supply current compatible evidence and legitimate authorization, or abandon work; neither implies past effects were undone.',
      'Inspectability and any unaffected local functions; abstention from this next gated action.', 'Throughput for held work; possibly its deadline.',
      'Effects proceed despite the alleged gate, or an ungated participant still accepts old requests.',
      '../../crates/factory-task/src/deliver.rs:234-316,451-506; scenarios.csv',
      'Source supports only local attempt-budget refusal and explicit resume; generic semantic and provider gates are stipulated, not implemented guarantees.')
state(3, 'Effect unknown and retry withheld', 'information-starved wait',
      'An attempt may have crossed the boundary; evidence is insufficient and further attempts are withheld.',
      'None inherent. Missing information plus refusal policy sustains the wait, not attraction.',
      'Lost acknowledgment, ambiguous restoration, or missing participant history.',
      'Correlated participant/recipient evidence resolves outcome; explicit risk acceptance may permit a new attempt without resolving the old one.',
      'Honest uncertainty and remaining local attempt evidence.', 'Known outcome and automatic progress; old work can still act.',
      'Independent evidence proves rejection or acceptance, or a retry loop actually continues.',
      '../../crates/factory-task/src/deliver.rs:234-316,451-506; ../../crates/factory-recovery/src/restore.rs:162-309; scenarios.csv',
      'Source supports journal-before-writer and restore blocking for retained attempted tasks. It does not prove business-effect reconciliation or recover missing journal suffixes.')
state(4, 'Settled but semantically wrong result', 'erroneous completion',
      'Structurally valid data or a reported result has the wrong units, person, context, or interpretation.',
      'None necessary. One wrong completion does not establish a reinforcing loop.',
      'Semantic check absent or shares the same error; work is marked finished.',
      'Independent domain comparison exposes error; correction is new work and may leave irreversible consequences.',
      'Bytes, signatures, task status, and sometimes technical service availability.', 'Correct business meaning; apparent success is not evidence of it.',
      'Independent subject and quantity verification agrees with the intended operation.',
      '../../crates/factory-task/src/complete.rs:101-216; scenarios.csv',
      'Source completion validates result presence/size and transition, not artifact truth. Unit, identity and real-world errors remain hypothetical.')
state(5, 'Self-confirming false record', 'attractor hypothesis',
      'Wrong canonical input feeds all verification, backup and future decisions; those decisions promote that input as trusted again.',
      'Wrong record -> agreeing checks -> confidence and reuse -> replacement of alternatives -> stronger dependence on wrong record. Loop can persist after the original defect stops.',
      'Common exporter/table/decoder plus no independent semantic anchor and repeated reuse.',
      'A genuinely independent anchor contradicts the record and is allowed to change decisions; corrected bytes alone do not restore lost history.',
      'Internally consistent operations and repeatable but wrong outputs.', 'Independent grounding and, if alternatives overwritten, recoverable original truth.',
      'Checks use a different origin and surface the discrepancy, or no decisions feed records back into future trust.')
state(6, 'Confidentiality already lost', 'absorbing loss',
      'An unauthorized party has actually learned confidential content or identifying metadata.',
      'No feedback needed. Knowledge cannot be made never learned; further distribution is contingent.',
      'Export, labels, routing responses, or geographic transfer disclose protected information to an unauthorized observer.',
      'No exit from historical disclosure. Containment can stop future access but cannot guarantee forgetting.',
      'Local operations may remain intact; evidence of disclosure may survive.', 'Prior exclusivity of disclosed knowledge, not automatically all future privacy.',
      'No unauthorized party could access or infer the information; protected ciphertext without a key is not disclosure.')
state(7, 'Active probing and response cycle', 'externally driven cycle',
      'An observer repeatedly induces alerts and can correlate their responses with recipients or timing.',
      'Each response improves the next probe, but continued probes require an active external actor.',
      'Observable timing/routing variation and alert-triggering access.',
      'Probing stops or responses cease to distinguish recipients; learned information remains BD06.',
      'Business services and perhaps alert delivery.', 'Secrecy of response patterns and attention capacity.',
      'Observed responses have no correlation with real recipient availability or identities.')
state(8, 'Competing writers and repairs', 'attractor hypothesis',
      'Two writers retain effective mutation authority and each treats the other writer changes as drift requiring repair.',
      'Writer A changes state -> B repairs toward B view -> A repairs toward A view. No new split-brain event is needed once both loops exist.',
      'Clone or repair kernel receives independent authority without effective shared exclusion.',
      'One authority becomes ineffective at every affected participant, followed by reconciliation of already accepted effects.',
      'Both local histories and plausible liveness may persist.', 'Single authoritative history and predictable action count.',
      'One writer is actually rejected at the participant, or neither writer reacts to the other; then only a finite conflict occurred.')
state(9, 'Delayed work has outlived its authority', 'transient exposure',
      'Queued or recovering work remains deliverable after authority, prohibition, identity context or safety conditions change.',
      'None required. Delay creates a hazardous interval; it ends only with expiry enforcement, rejection, acceptance, or cancellation evidence.',
      'Validation happened earlier than the last boundary that can cause the effect.',
      'Participant refuses stale work -> BD02/BD25; acceptance may yield BD10; no outcome evidence -> BD03.',
      'Historical authorization and operation identity, if retained.', 'Assurance that past approval still covers present execution.',
      'All effect-capable participants enforce current validity at acceptance, not merely at enqueue.')
state(10, 'Past external effect cannot be rolled back', 'absorbing loss',
      'An effect actually occurred and its relevant dimension is irreversible, such as disclosure, injury, or an unretractable communication.',
      'None. Absorption is relative to the past event; repairable financial balance is not automatically irreversible harm.',
      'A wrong, duplicated, expired or prohibited action is accepted and has an irreversible consequence.',
      'No restoration of the prior world on that dimension; compensation may improve later conditions.',
      'Whatever records, authority and unaffected resources remain.', 'Ability to make the past effect not have happened.',
      'No effect occurred, or the claimed lost property is fully reversible and no irreversible dimension is shown.')
state(11, 'Writer handoff ownership unknown', 'information-starved wait',
      'A handoff loses its acknowledgment and neither available evidence nor authority determines who may write.',
      'None inherent. Choosing abstention preserves the wait; guessing can enter BD08.',
      'Communication breaks between relinquishment, transfer and activation.',
      'Durable ownership evidence from the relevant authority identifies one effective writer, or a legitimate new authority excludes both old contenders.',
      'Local stores and handoff fragments.', 'Known ownership and safe immediate writes.',
      'Atomic local-only migration or a durable ownership register resolves the alleged gap.')
state(12, 'Recovery prerequisite deadlock', 'dependency deadlock',
      'Restoring capability A requires reading or changing B, but B is accessible only through broken A; no independent bootstrap path exists.',
      'Circular dependency, not a demonstrated attractor: each prerequisite waits for the other.',
      'Optional plugin becomes the exclusive reader of its own emergency configuration, or credential repair itself needs the unavailable credential.',
      'An independent readable configuration, compatible bootstrap binary, or legitimate credential path exists; permanent loss may instead end service.',
      'Raw bytes or unaffected scopes if separately accessible.', 'In-band repair capability.',
      'An offline reader or safe local startup path can operate without the broken prerequisite.')
state(13, 'Maintenance lock-in', 'attractor hypothesis',
      'Knowledge/build dependencies are scarce and incidents consume the effort needed to reduce that scarcity.',
      'Repair difficulty -> ad hoc patch or deferral -> less understood/reproducible system -> greater next repair difficulty. Persists after one maintainer departure if incident effort dominates learning.',
      'Hidden assumptions, vanished toolchain/account, and no effective knowledge transfer or reproducible substitute.',
      'A successor demonstrates independent rebuild and operation, or service is retired; a funding injection alone does not prove exit.',
      'Existing binary may keep working and source may remain readable.', 'Confident change capability and timely repairs.',
      'A new maintainer can reproduce and repair using documented local inputs; one missing person alone creates no loop.')
state(14, 'Exception-driven authority erosion', 'attractor hypothesis',
      'Exceptions allowing effects become precedents and incidents are handled by adding more exceptions rather than resolving their meaning.',
      'Ambiguous exception -> discretionary bypass -> precedent and extra branch -> greater ambiguity. Installed policy and incentives sustain the loop.',
      'Accumulated special cases with no comprehensible authority ordering.',
      'Decision-makers can explain and consistently apply effective authority for counterexamples, or operations stop pending authority resolution.',
      'Ability to perform work and a nominal approval vocabulary.', 'Predictable authorization meaning and trustworthy audit interpretation.',
      'All sampled conflicting cases resolve under one understood rule without adding a bypass.')
state(15, 'Common dependency outage', 'externally sustained outage',
      'Different nominal services share a cloud, billing, credential or mandatory startup dependency that is unavailable.',
      'No internal feedback required; the unavailable common prerequisite sustains the outage. Retry feedback belongs to BD24 separately.',
      'Shared infrastructure fails, payment is suspended, or optional monitoring becomes a mandatory root dependency.',
      'Prerequisite recovers or operations genuinely become independent of it; permanent supplier loss can enter BD22.',
      'Offline records and local functions only if not startup-gated by that dependency.', 'Correlated service, backup and alert availability, not automatically existing backup bytes.',
      'A claimed independent component continues operating through a real failure of the shared prerequisite.')
state(16, 'Test evidence cannot see the defect', 'unresolved dynamics',
      'Fixtures, clocks or mocks exclude delayed acceptance, real fencing behavior or shared timing failure.',
      'No production loop inferred. Passing tests can reinforce confidence, but operational behavior is unspecified.',
      'Test oracle shares the assumption being tested or time never advances independently.',
      'A discriminating trace from a separately controlled boundary reveals whether the omitted behavior matters.',
      'Evidence for the cases actually represented by tests.', 'Justification for extrapolating test success to omitted failures.',
      'The suite independently represents and rejects the omitted interleaving or provider behavior.',
      '../../crates/factory-adapter/src/lib.rs:434-805; scenarios.csv',
      'Source adapter tests use FakeHerdr and recorded payloads; this does not establish that every repository test has these blind spots. Scenario-specific omissions are stipulated.')
state(17, 'Signal present but not actionable by the human', 'information-starved wait',
      'An alert exists but the intended human cannot perceive, receive or uniquely interpret it, and no other effective responder acts.',
      'No intrinsic loop; access, language, display or channel conditions maintain the gap.',
      'Color-only warning, truncated identity, misleading translation, or powered-off phone.',
      'An accessible unambiguous observation reaches a legitimate responder; action still needs correct authority and repair capacity.',
      'Possibly working monitor and local diagnostic evidence.', 'Timely human understanding; detection is not intervention.',
      'The actual operator identifies the right state and instance without relying on the ambiguous channel.')
state(18, 'Alert workload sustains non-response', 'attractor hypothesis',
      'Outstanding alerts/escalations consume more attention than responders can clear, and unresolved incidents produce further reminders or faults.',
      'Backlog -> less repair time -> unresolved faults/reminders -> more backlog. Can persist after flapping stops only if this endogenous production exceeds clearance.',
      'Large alarm burst or costly false positives with inadequate effective attention capacity.',
      'Net endogenous alert production falls below clearance and actual faults are repaired; muting alone may instead enter BD21 or BD19.',
      'Raw alerts and some service functions.', 'Signal salience and timely diagnosis.',
      'After the initial burst ends, backlog drains without recurring production; then it was a finite transient, not this attractor.')
state(19, 'Acknowledged but deliberately deferred', 'policy hold',
      'Acknowledgment closes the paging obligation but does not allocate timely repair, and the queue permits indefinite deferral.',
      'No autonomous feedback required; scheduling policy retains the backlog. Repeated deferral may participate in BD18.',
      'A human acknowledges and queues repair far beyond acceptable time.',
      'Repair is actually executed and its outcome checked, or the work is explicitly abandoned.',
      'Ownership/acknowledgment evidence and potentially diagnostic records.', 'Timely restoration; acknowledgment is not recovery.',
      'Acknowledgment triggers verified repair within the relevant deadline rather than merely a queue entry.')
state(20, 'Legitimate authority conflict', 'policy hold',
      'Two valid representatives issue incompatible decisions and an effective gate refuses to guess a precedence.',
      'No restoring force; unresolved authority semantics sustain the hold.',
      'Concurrent approval/revocation or incompatible mandates with no established ordering.',
      'A competent authority resolves scope and precedence, or the operation is abandoned.',
      'Conflicting instructions and ability to abstain at controlled boundaries.', 'A uniquely legitimate action; immediate throughput.',
      'A pre-existing valid rule uniquely orders the decisions for this operation, or participants already acted on both.')
state(21, 'Oversight captured or inaccessible', 'externally sustained control regime',
      'The only alert/account authority is unavailable, departed, or conflicted and no independent authorized observer can change it.',
      'Control holder can suppress evidence of its own conduct, preserving control; persistence still depends on account governance or the installed mute policy.',
      'Sole monitoring account retained after departure, alarms silenced by interested team, or legitimate successor locked out.',
      'Independent lawful account recovery or governance changes restore effective oversight; missing history may remain lost.',
      'Business actions may continue and the account may still function for its controller.', 'Independent visibility and legitimate intervention path.',
      'An independent authorized party can inspect and act despite the alleged exclusive account control.')
state(22, 'Provider-bound service ended', 'service loss relative to a boundary',
      'Required provider permanently stops and no presently usable compatible substitute exists.',
      'None required. Supplier cessation is external; irrecoverability of all business function is not established.',
      'Shutdown deadline passes before usable migration or export.',
      'Alternative provider or local implementation becomes viable; otherwise retire the dependent function. Lost provider-only data may be unrecoverable.',
      'Locally interpretable records and independent services.', 'That provider capability; possibly provider-only data and continuity.',
      'A compatible alternative with legitimate credentials actually executes the required operation in time.')
state(23, 'Delayed liabilities still arriving', 'transient with uncertain horizon',
      'Usage/effects are already committed but their cost or acceptance reports arrive later than local budgeting or shutdown decisions.',
      'No intrinsic loop. Repeated spending against understated cost can create recurrence only with a continuing policy and demand.',
      'Delayed billing, delayed acceptance, or obligations outliving local operation.',
      'All bounded pending liabilities settle with evidence; unknown/unbounded queues prevent demonstrated closure.',
      'Known accounting entries and possibly local spending stop.', 'Reliable current budget headroom and a proven final liability amount.',
      'A hard provider-enforced ceiling bounds total committed cost, including unreported usage, and no hidden obligations remain.')
state(24, 'Speculation or retry overload', 'attractor hypothesis',
      'Response delay causes more copies/retries, which increase load and delay; control does not damp the loop.',
      'Latency -> extra requests -> congestion/cost -> more latency. Warm and overloaded basins may coexist under the same base demand.',
      'Aggressive tail target or cold restore cache with outstanding operations; retry amplification exceeds spare capacity.',
      'Effective admitted work falls below capacity long enough to drain and warm; old accepted copies still require outcome accounting.',
      'Some successful requests and local journals if not exhausted.', 'Predictable latency, cost and perhaps unique external effects if deduplication is absent.',
      'Duplicate work is coalesced or capped so the burst drains under unchanged base demand; no self-sustaining overload occurs.')
state(25, 'Legally or safely withheld operation', 'policy hold',
      'A competent applicable prohibition or safety boundary is known and the last effective boundary actually refuses the action.',
      'No feedback needed. Policy enforcement holds the operation; this is not proof the physical world is safe.',
      'Prohibition, unsafe context, or expired mandate is detected before acceptance.',
      'Legitimate resolution and fresh safety assessment permit new work, or the action is abandoned. Contradictory legal duties may remain unresolved.',
      'Evidence of refusal and prevention at the boundaries that enforce it.', 'Requested action throughput; already buffered actions may escape this hold.',
      'A participant accepts under the old authorization despite the stated prohibition or safety boundary.')
state(26, 'Schedule meaning or expiry drift', 'policy-retained timing error',
      'A stored clock/calendar interpretation no longer matches intended elapsed time or civil-time meaning, and policy keeps using it.',
      'No autonomous attraction. A backwards clock may retain a mute; changed time rules may repeatedly mis-time scheduled work.',
      'Suspend semantics, wall-clock jump, time-zone reform, or deferral beyond validity.',
      'Intent is re-established with current trustworthy timing evidence; ambiguity may instead require BD02.',
      'Stored times and perhaps local monotone order within valid clock epochs.', 'Correct relation to elapsed time or intended local appointment; not necessarily every schedule.',
      'The stored semantics explicitly distinguish fixed instant from civil recurrence and detect epoch discontinuities correctly.')
state(27, 'Contradictory evidence retained', 'information-starved wait',
      'Different observations disagree about identity, liveness, historical order or effect and no trusted discriminator is available.',
      'No inherent loop. A policy not to collapse disagreement preserves an unresolved assessment.',
      'Conflicting journals, runtime/file/agent claims, or recipient/provider testimony.',
      'Independent provenance, causal ordering or direct effect evidence resolves the relevant question; signatures alone may not.',
      'Multiple claims and their provenance if preserved.', 'A uniquely justified history or outcome.',
      'Disagreement is explained by different time intervals/subjects, or independent causal evidence selects a consistent interpretation.',
      '../../crates/factory-adapter/src/lib.rs:30-106,316-399; ../../crates/factory-recovery/src/evidence.rs:80-82; scenarios.csv',
      'Source distinguishes confidence and leaves idle/done task signals unchanged; it does not arbitrate arbitrary competing business evidence.')
state(28, 'Rewarded metric substitution', 'attractor hypothesis',
      'Agents or teams are rewarded for a proxy and can exclude difficult cases, relabel incidents or report unverified artifacts without external outcome checks.',
      'Proxy improvement -> reward/trust -> greater selection/reporting latitude -> more proxy improvement while real quality worsens. Can persist after one misleading report.',
      'Measured denominator, classification, or artifact status is controlled by the evaluated actor.',
      'Independent outcome and rejected-work accounting changes the payoff, or the actor loses control over the proxy.',
      'Attractive metrics, finished-looking artifacts and perhaps easy-case throughput.', 'Reliable meaning of success, availability and throughput.',
      'Rejected work and relabelled downtime remain counted, or rewards fall when independently checked quality falls.')
state(29, 'Planned shutdown mistaken for recurring failure', 'externally driven cycle',
      'An energy schedule turns the host off and monitoring treats each planned absence as an unexpected total failure.',
      'External daily schedule drives recurrence; no autonomous failure oscillation established.',
      'Off-hours power policy and alert expectations disagree.',
      'Power schedule or monitoring interpretation changes; essential obligations must still be assessed independently.',
      'Daytime operation and possibly persisted tasks.', 'Nighttime service and alert credibility; hazards depend on required nighttime work.',
      'Outages continue with the schedule disabled, or off-hours monitoring already distinguishes expected absence.')
state(30, 'Archive bytes without a trustworthy interpreter', 'information-starved wait',
      'An artifact survives but available CPU/format/software cannot reliably recover its intended semantics.',
      'No intrinsic loop. Missing interpreter or specification sustains inability to use the archive.',
      'Native representation restored across architectures or obsolete format dependencies disappear.',
      'A validated interpreter or independent semantic reconstruction becomes available; total loss is not assumed merely from obsolescence.',
      'Raw bytes and any external specification or human-readable context.', 'Current trustworthy execution or interpretation.',
      'A portable encoding and independent decoder reproduce the intended semantics, not just a plausible value.')
state(31, 'Distinct names collapsed with information lost', 'absorbing loss conditional on overwrite',
      'Restore maps formerly distinct paths to one identity and overwrites unique content; no surviving separate copy contains it.',
      'None. Aliasing alone is a detectable conflict; absorption requires actual unrecoverable overwrite.',
      'Case-sensitive source meets case-insensitive destination and colliding writes are accepted.',
      'No exact reconstruction without another copy or semantic source; before overwrite the conflict can instead be held.',
      'One surviving namespace entry and unaffected workspaces.', 'Unique overwritten content and distinction between its owners.',
      'Restore rejects collisions or preserves both contents under distinct identities.',
      '../../crates/factory-paths/src/lib.rs:1-25,80-135; scenarios.csv',
      'Source provides runtime device/inode alias checks and explicitly denies durable/security identity. Cross-filesystem restore overwrite is hypothetical, not observed.')
state(32, 'Local shutdown with outstanding external obligations', 'information-starved wait',
      'Factory or its monitoring contract has ended while a participant may still accept queued work or report liabilities.',
      'No feedback needed. External retained work outlives local supervision; new forcing is not required for a delayed effect.',
      'Decommission or contract expiry precedes verifiable queue drain, rejection, or last export.',
      'Every relevant participant supplies reliable closure evidence, or later effects settle; silence alone is not closure.',
      'Any retained records and reachable legitimate recipient; participant queue may survive the host.', 'Ability to assert that shutdown ended effects or observations.',
      'Participants provably discard pending work and reject the retired authority before supervision ends.')
state(33, 'All recovery information and legitimate actors gone', 'absorbing loss',
      'The scenario literally removes computer, monitor, communications, every backup and every legitimate recipient, with no surviving alternate source or authority.',
      'None. There is no remaining system to form an attractor.',
      'Simultaneous complete loss of the stated recovery boundary.',
      'No endogenous recovery. Discovery of surviving information/authority changes the premise; a new system is not recovery of lost facts.',
      'Nothing inside the stated boundary; the outside world is not thereby assumed destroyed.', 'Recoverable state and legitimate continuation inside this boundary.',
      'A surviving authorized person, independent copy, or external record can reconstruct relevant facts.')
state(34, 'Perfectly consistent observations cannot ground truth', 'epistemic indistinguishability',
      'Every accessible trusted observation has identical output in a true world and a false world, and no independent channel is permitted.',
      'No dynamics inferred. Indistinguishability is an information limit, not an attractor or proof of system stability.',
      'All trusted sources share a consistent false foundation, or a hostile substrate supplies perfect internal reports.',
      'Access an independent discriminating fact; under the literal closed premise there is no observational exit.',
      'Internal consistency and ability to compute on supplied observations.', 'Justified determination of which real world holds.',
      'Any accessible observation differs between the alleged worlds; ordinary independent physical checks break the premise.')
state(35, 'Trusted malicious substrate maintains its own credibility', 'attractor hypothesis',
      'Installed signed code controls health, history and verification and can retain its authority because those same reports justify continued trust.',
      'Forged success -> no intervention or renewed trust -> continued malicious execution -> more forged success. Can persist after the update distributor disconnects.',
      'A signed malicious update crosses a trust boundary with enough authority to falsify local evidence and remove competing history.',
      'An independently trusted observer or execution boundary exposes and excludes the code; erased unique history may remain unrecoverable.',
      'Apparent operation and whatever data lie outside malicious control.', 'Integrity of internal evidence and possibly confidentiality/history; extent is not knowable from forged reports alone.',
      'Independent attestation or out-of-band evidence detects divergence, or the installed code cannot control every claimed check.')
state(36, 'Business continues outside monitor visibility', 'externally sustained partial partition',
      'Factory can reach business participants but monitor-to-Factory observation is broken; no effective participant fence stops work.',
      'No internal feedback required; network topology sustains asymmetric knowledge. Automated failover may add BD08.',
      'One-way or selective network partition separates liveness observer from active business paths.',
      'Observation path heals or an effective participant authority change stops old work; starting another clone alone is not resolution.',
      'Original business execution and local records.', 'Monitor knowledge of execution and confidence that failover is exclusive.',
      'Actual business paths are also blocked, or participants reject all old work when monitoring loses contact.')

state(37, 'Necessary prerequisite unavailable', 'externally sustained wait',
      'Repair or startup needs a toolchain, account or credential that is currently unavailable; no circular dependency or permanent impossibility is established.',
      'None inherent. Unavailability maintains the wait; repeated costly workarounds can separately create BD13.',
      'A repair or emergency startup encounters a missing prerequisite that ordinary running work may not need.',
      'A legitimate compatible substitute or restored prerequisite becomes available; permanent cessation without substitute can instead yield BD22.',
      'Existing binaries, readable records and functions not needing the missing prerequisite.', 'Immediate ability to repair or start affected functions.',
      'The operation completes using an independently available compatible prerequisite.')


def t(s, initial, sequence, dest, conditions, falsifier, reason, extra='', unresolved=False):
    sid = f'S{s}'
    refs = 'Qualitative conditional analysis of scenarios.csv ' + sid + '. '
    if extra:
        refs += extra
    else:
        refs += 'No implementation or production evidence asserted.'
    if unresolved:
        dest = 'none'
    TRAJECTORIES.append([f'BDT{s-275:03}', sid, initial, sequence, dest, conditions, refs, falsifier])
    COVERAGE.append([sid, dest, 'unresolved' if unresolved else 'conditional', reason])


t(276, 'Monitoring emits readable heartbeat labels to a party outside the customer confidentiality boundary.',
  'Customer names and timing appear in labels -> an unauthorized reader learns them -> BD06. If identifiers remain opaque to that reader and no timing linkage is possible, the stated leak is not established; service may remain normal.',
  'BD06', 'Actual readership, retention and re-identification matter; label generation alone does not prove unauthorized learning.',
  'An access/re-identification check shows no unauthorized observer can recover customer identity or business hours.',
  'Historical disclosure is absorbing only once learned; continued service does not reverse it.')
t(277, 'A debug exporter can access full agent conversations and monitoring has a broader readership.',
  'Exporter serializes conversation -> export crosses the readership boundary -> BD06. A pre-transfer refusal gives BD02; removing the export later cannot make already learned material secret again.',
  'BD06 BD02', 'Assume the conversation contains protected content and at least one unauthorized recipient can read it. No actual transcript was inspected.',
  'Synthetic content inspection shows the exported payload excludes protected conversation, or access never crosses the boundary.',
  'Payload scope and recipients distinguish disclosure from a blocked or private diagnostic export.')
t(278, 'An attacker can trigger false alerts and observe timing or routing-dependent responses.',
  'Probe -> response -> improved target/timing inference -> next probe: BD07 while the attacker continues. Successful inference leaves BD06 after probes stop; a large burst can additionally enter BD18 only with self-sustaining backlog.',
  'BD07 BD06 BD18', 'Timing correlation and an observable response are required; no alert recipients are discovered or contacted in this review.',
  'Response distributions do not reveal recipients or readiness, and backlog drains after probes stop.',
  'Separate an externally driven probing cycle from retained knowledge and a conditional attention loop.')
t(279, 'Backups all derive from the same erroneous exporter and checksums are computed over its output.',
  'Exporter produces wrong bytes -> matching checksums pass -> restore chooses wrong but consistent data -> BD04. If restored data become the next canonical source and independent originals are discarded, verification and reuse close BD05. An independent source comparison can instead hold at BD02.',
  'BD04 BD05 BD02', 'Checksums prove correspondence to exported bytes, not business truth; repeated reuse is necessary for BD05.',
  'An independently sourced original disagrees and prevents adoption, or the exporter is corrected before false data become canonical.',
  'Consistent corruption is not automatically an attractor; only the repeated trust/reuse branch supplies feedback.',
  'Source ../../crates/factory-store/src/backup.rs:1-41 uses VACUUM INTO, not the scenario exporter; its snapshot operation is not semantic validation.')
t(280, 'A valid numeric amount is stored without a separately authoritative comparison.',
  'Bit changes amount but validity checks still pass -> wrong amount used -> BD04; an irreversible downstream effect can also yield BD10. Detection before use yields BD02. A single bit error alone supplies no long-term feedback.',
  'BD04 BD10 BD02', 'Representation must permit the changed value and semantic checks must miss it; physical damage is not assumed for every amount error.',
  'Independent signed amount evidence or business reconciliation catches the change before acceptance.',
  'Distinguish syntactic validity from quantity correctness and from a proven irreversible consequence.')
t(281, 'Two split-brain writers produced signed but causally incompatible histories.',
  'Both journals authenticate -> signatures do not select legitimate order -> BD27 if disagreement is retained. Continuing autonomous repairs can produce BD08. An independent ownership/participant history may reconcile bounded effects to BD01 without making prior divergent effects disappear.',
  'BD27 BD08 BD01', 'Authority ordering and participant acceptance evidence, not merely signature validity, determine the branch.',
  'A common enforced authority epoch orders both histories and all participants reject the loser.',
  'Conflicting authentic claims are not evidence of forgery or proof that both may continue writing.')
t(282, 'A restored snapshot predates invoices and revocations that may already have reached external parties.',
  'Restore reconciles retained attempted tasks to a local hold -> BD03 for those tasks. Missing later journal entries remain missing: snapshot-only automation may resend or act under revoked rights -> BD09 then possibly BD10. Independent participant reconciliation can establish BD01 for a bounded operation.',
  'BD03 BD09 BD10 BD01', 'The source hold only sees rows present in the snapshot. Absence of an attempt in an old snapshot is not proof of no real-world delivery.',
  'A complete post-snapshot participant record and current revocation authority are recovered before any action.',
  'Source-supported local restore behavior does not cover the scenario missing suffix or business consequences.',
  'Source ../../crates/factory-recovery/src/restore.rs:162-309 blocks running/attempted queued tasks and clears unattempted queued assignments; no live participant evidence is consulted.')
t(283, 'Original and restored clone both retain credentials and can emit plausible heartbeats.',
  'Two healthy-looking senders -> monitor cannot infer uniqueness -> BD27. If both act and repair toward separate views, BD08; if one is effectively excluded and accepted work reconciled, BD01 is possible. A blocked clone alone does not fence original business work.',
  'BD27 BD08 BD01', 'Participant-level exclusion and durable identity matter; heartbeat signatures and matching pane IDs alone are insufficient.',
  'A participant trace rejects every old-generation write while one effective writer continues.',
  'Source identity comparison is only a local session check, not clone exclusion.',
  'Source ../../crates/factory-recovery/src/reconnect.rs:144-169 rejects unequal known harness IDs but returns true if either is missing; ../../crates/factory-recovery/src/evidence.rs:80-82 checks confidence only.')
t(284, 'Recovery validates rights at start but lasts longer than their validity window.',
  'Rights expire during recovery -> BD09. Revalidation at the last effect boundary yields BD02; accepting stale rights may yield BD10. Recovery duration alone does not say which participant behavior applies.',
  'BD09 BD02 BD10', 'Authorization validity is tied to current acceptance time and can expire independently of recovery progress.',
  'The recovered operation carries a still-valid mandate and every participant enforces it when accepting.',
  'Recovery success is not proof that start-time rights remain current.')
t(285, 'New client and old plugin parse the same number successfully but attach different units.',
  'Wire validation passes -> different physical/business quantity -> BD04. Explicit incompatible meaning refusal yields BD02; downstream irreversible effect is BD10 only if one occurs.',
  'BD04 BD02 BD10', 'Unit semantics must differ on this operation, not simply protocol version labels.',
  'Cross-version interpretation of an independent dimensional fixture produces identical intended quantities or refuses before effect.',
  'Type-valid numeric interoperability does not establish semantic compatibility.')
t(286, 'A scope-local optional plugin upgrade requests a migration of shared core state.',
  'If rejected as outside its authority, BD02 and unaffected scopes remain usable. If all startups wait for the migration, BD15; if migration/config recovery needs the failed plugin itself, BD12. A successful reviewed migration could instead restore bounded functionality without proving general isolation.',
  'BD02 BD15 BD12', 'Shared-store layout, startup policy and ownership of migration determine blast radius; a request alone causes no migration.',
  'Plugin can be disabled without core mutation and unrelated scopes start independently.',
  'Per-scope activation is an instruction/design boundary, not a demonstrated startup property.',
  'Design promise: ../../AGENTS.md Product boundaries requires explicit per-scope activation. Source ../../crates/factory-store/src/migrations.rs:36-70 has core migrations, not an inspected plugin-driven migration API.')
t(287, 'An old binary encounters a schema newer than the fields it understands.',
  'If it refuses before writes, BD02. Under the scenario destructive-default behavior, unknown fields are replaced -> BD04; repeated backups/reuse may enter BD05. Exact unique information may be lost even if a valid schema remains.',
  'BD02 BD04 BD05', 'The destructive old-binary behavior is stipulated, not established for the inspected Rust migration library.',
  'Byte comparison shows old-binary open refuses or preserves every unknown field before any mutation.',
  'Version comparison and schema tests do not alone prove historical binary forward compatibility.',
  'Source ../../crates/factory-store/src/migrations.rs:36-70 delegates to rusqlite_migration to_latest; old-release behavior and dependency downgrade refusal were not executed or inspected.')
t(288, 'New code already caused a genuinely irreversible outside effect before rollback.',
  'Rollback restores old local behavior but not external past -> BD10. Old code lacking effect evidence may retry -> BD03 if withheld, or additional BD10 if accepted. Reconciliation can repair records, not un-send the effect.',
  'BD10 BD03', 'An actual irreversible effect is part of the premise; reversibly changing a local row is not the same case.',
  'Effect was never accepted, or complete external evidence prevents repeat and the claimed lost property is entirely reversible.',
  'Code rollback is not a world rollback; no attractor is implied by a one-time irreversible action.')
t(289, 'A migration includes transfer between two potential writers rather than only one SQLite transaction.',
  'Link drops between handoff and new start -> BD11 if both withhold uncertain writes. Guessing ownership may leave no writer or two writers, and reacting repairs can enter BD08. Durable transfer evidence selecting one writer permits BD01 for the handoff.',
  'BD11 BD08 BD01', 'Distributed ownership handoff must actually exist; local transaction rollback alone has no network handoff gap.',
  'The migration is strictly local/atomic, or an external durable ownership register resolves all crash points.',
  'A migration transport break is not resolved by citing local SQL atomicity.',
  'Source ../../crates/factory-store/src/migrations.rs:36-70 supports local migrations only; no distributed writer handoff inspected.')
t(290, 'The only interpreter of broken-plugin configuration is the broken plugin itself.',
  'Emergency repair requests config -> broken reader cannot supply it -> repair cannot start -> BD12. A truly independent reader breaks the dependency and may permit BD02 pending a validated change.',
  'BD12 BD02', 'No unencrypted independently specified config or working compatible plugin is otherwise available.',
  'An offline read of a synthetic config succeeds without starting the plugin or contacting its provider.',
  'Circular prerequisites establish deadlock under the stated boundary, not empirical attraction.')
t(291, 'The sole maintainer leaves a running system whose important assumptions are undocumented.',
  'Successor initially waits or makes cautious changes. If incidents consume learning time and patches add hidden assumptions, BD13; if decisions become ad hoc authority precedents, BD14. Demonstrated independent rebuild/operation breaks these branches; departure alone is not proof of convergence.',
  'BD13 BD14', 'Persistence depends on incident rate, learning capacity and knowledge availability after the departure.',
  'Successor resolves representative incidents from surviving documentation without new hidden exceptions.',
  'A social reinforcing mechanism is required beyond a missing maintainer.')
t(292, 'Repair needs a vanished toolchain and deleted cloud account, while the existing binary may still run.',
  'Attempted repair cannot reproduce prerequisites -> BD37. Repeated workaround deferral can enter BD13. If the provider-only capability is permanently gone with no substitute, BD22; readable source alone does not restore credentials. No circular dependency is implied merely by an unavailable toolchain.',
  'BD37 BD13 BD22', 'Alternative compiler/emulator, export and legitimate account recovery availability remain empirical questions.',
  'A successor rebuilds the repair without the account and validates compatible behavior using local inputs.',
  'Distinguish unavailable repair path from total information loss or permanent impossibility of all future substitutes.')
t(293, 'Many special cases govern which actions have outside effects, and nobody can explain their interaction.',
  'Ambiguity -> urgent bypass -> new precedent -> more ambiguity: BD14 if that response pattern persists. An actual stop pending authority interpretation gives BD02; a stable but misunderstood rule alone is not an attractor.',
  'BD14 BD02', 'Feedback requires repeated exception creation and adoption, not merely a large rule count.',
  'Blind review of conflicting cases yields consistent authorized decisions without new exceptions.',
  'Authority opacity can be self-reinforcing, but conservative abstention is a competing basin.')
t(294, 'Plugins, replay and monitoring share a JSON library and upgrade together.',
  'Shared parse failure can yield BD15 while broken code remains mandatory. Shared plausible reinterpretation can yield BD04; if all checks then certify and reuse that interpretation, BD05. Rejection before mutation gives BD02.',
  'BD15 BD04 BD05 BD02', 'The dependency update must affect the fields used; syntactic rejection and silent semantic change are different branches.',
  'An independently implemented decoder or held-out semantic corpus exposes disagreement before data adoption.',
  'Common library creates correlated evidence risk, not proof that replay or monitoring already shares an implemented path.',
  'Source ../../crates/factory-adapter/src/lib.rs:316-399 parses serde_json values; ../../crates/factory-task/src/complete.rs:101-132 serializes paths. A production replay/monitor pipeline was not inspected.')
t(295, 'A monitoring feature has become a mandatory cloud-authenticated prerequisite for every child-scope startup.',
  'Account unavailable -> all dependent child startups fail -> BD15. Emergency startup/config access requiring the same unavailable account remains BD37; that is not necessarily a circular dependency. If monitoring is truly optional, unaffected local scopes avoid either state.',
  'BD15 BD37', 'The global prerequisite is stipulated; account outage need not corrupt durable local records.',
  'With a synthetic unavailable account, child scope local functions still initialize without monitoring.',
  'Dependency concentration is a startup coupling, not the same as independent provider failures.',
  'Design promise only: ../../AGENTS.md requires local safe mode and per-scope plugin activation; no deployed startup enforcement measured.')
t(296, 'Automated repair is initially a helper but obtains direct write authority independent of the kernel.',
  'Kernel and repair disagree -> each rewrites the other changes -> BD08 if both reconcile repeatedly. If helper only submits commands to one writer, the alleged second-kernel loop is absent. A one-time unauthorized mutation instead gives BD04 or BD27.',
  'BD08 BD04 BD27', 'Independent effective writer authority and mutually reactive repair policies are required for the oscillation.',
  'All helper changes serialize through the sole authority and cannot bypass its decision/order.',
  'A second writer is a structural entry condition, not a guarantee of sustained conflict on its own.')
t(297, 'Tests provide immediate replies and never deliver an acceptance after the caller timeout.',
  'Tests pass -> BD16 evidence gap. In omitted production traces, retry withholding gives BD03; retry plus late acceptance can produce BD10 or BD24. Actual destination depends on transport, deduplication and policy absent from the scenario.',
  'BD16 BD03 BD10 BD24', 'No inference that all current repository tests are immediate; omission is the assigned scenario premise.',
  'A delayed-acceptance trace with independent time and retained operation identity is present and changes the test outcome when retry safety is broken.',
  'Test inadequacy is explicit; production convergence remains conditional on omitted dynamics.',
  'Source ../../crates/factory-task/src/deliver.rs:234-316 preserves an attempt on writer error, but this local guarantee is not provider deduplication.')
t(298, 'Timeout logic and the alleged external monitor both advance on one virtual clock.',
  'Clock freezes -> neither deadline nor alarm fires -> BD16. This test says nothing about whether independent real clocks would detect or miss the fault; if the same clock coupling existed operationally, BD17-like missed response could occur but no production endpoint is selected here.',
  'BD16', 'The virtual clock is a test fixture; a test-time freeze is not evidence the real external monitor freezes.',
  'An independently advancing monitor clock triggers while the task clock remains frozen.',
  'Only the evidence-blind state is classified; actual production failure duration and timing remain unknown.')
t(299, 'A mock returns fencing success while the real participant still accepts old generations.',
  'Fixture passes -> BD16. Real failover leaves old authority effective -> BD08 if contenders react and continue; old accepted work may cause BD10 even if the local new writer is exclusive. Participant rejection would select the non-conflict branch instead.',
  'BD16 BD08 BD10', 'A fencing acknowledgment must correspond to rejection at the effect-capable participant, not a mock return value.',
  'A provider-owned acceptance trace rejects stale generations after fence acknowledgment, including previously buffered requests.',
  'Mock agreement cannot establish real authority extinction; no live provider test was performed.')
t(300, 'A report marks success but swaps euros, cents and decimal-comma meaning.',
  'Report remains syntactically valid -> human or downstream parser reads wrong quantity -> BD04. Repeated use of the same formatter as verifier can close BD05; an independent quantity check catches the issue before reliance and gives BD02.',
  'BD04 BD05 BD02', 'Formatting can be harmless if nobody relies on it and canonical quantity is correct; the report does not prove actual payment amount.',
  'Independent normalized amount/currency evidence matches the intended quantity across the display and action boundary.',
  'Separate wrong presentation, wrong acted-on quantity, and a recursive false-verification loop.')
t(301, 'A fast cache contains an old price that still looks reasonable for a new order.',
  'Cache hit hides age -> order uses obsolete price -> BD04. If terms explicitly lock the older quote, BD01 is possible; a current-version requirement detecting staleness gives BD02. Plausibility and speed are not freshness evidence.',
  'BD04 BD01 BD02', 'Correctness depends on pricing policy and quote validity, not age alone.',
  'The order is legitimately bound to the cached quote or a current authoritative price exactly agrees.',
  'An old price is not necessarily wrong; intended effective date determines the branch.')
t(302, 'Two real people share a name and work is routed by that name without adequate disambiguation.',
  'All workflow steps succeed for the other person -> BD04; confidential delivery also gives BD06 and irreversible consequences may give BD10. Subject disambiguation before effect gives BD02.',
  'BD04 BD06 BD10 BD02', 'Correct task UUID or scope identity is not proof of correct real person. A disclosure branch requires protected content.',
  'An independent subject identifier and recipient confirmation match the intended real person before delivery.',
  'Internal identity integrity and real-world referent correctness are different boundaries.')
t(303, 'Incident warning encodes critical meaning only by red color for a color-vision-deficient operator.',
  'Monitor detects -> operator cannot distinguish severity -> BD17 until another channel or person intervenes. Resulting wrong action may reach BD10 but is not implied by invisibility alone.',
  'BD17 BD10', 'No equally usable text, shape or sound conveys the needed distinction in time.',
  'A representative operator correctly identifies the warning and next authorized action without color discrimination.',
  'A delivered signal is not perceived information; duration depends on actual accessible alternatives.')
t(304, 'Mobile alarm truncates the only displayed distinction between long scope paths.',
  'Operator maps shortened label to the wrong instance -> stops it -> BD04 while the original incident remains BD17. If the stop has irreversible consequences, BD10; a full identity confirmation could instead give BD02.',
  'BD04 BD17 BD10 BD02', 'Displayed collision, operator interpretation and stop authority determine actual wrong action.',
  'Operator can uniquely identify the target using stable untruncated information before any stop is accepted.',
  'An incorrect human intervention is not automatically an attractor; the still-failing original is a separate state.')
t(305, 'Localized UI renders an unknown outcome as confidently failed.',
  'Uncertainty is linguistically collapsed -> operator retries believing no effect -> BD04 as false outcome belief, possibly BD10 after duplicate acceptance. Preserving unknown yields BD03; a correct clarification before retry gives BD02.',
  'BD04 BD10 BD03 BD02', 'Retry policy and actual first acceptance determine external harm; translation alone does not establish duplicate effect.',
  'Back-translation and operator interpretation preserve unknown as distinct from rejected, and no retry is inferred from it.',
  'This is loss of epistemic meaning, not simply unattractive UI text.')
t(306, 'The only intended alert recipient has a powered-off phone at night.',
  'Alert is queued/delivered to an inactive device -> no human observation for days -> BD17. Actual recovery depends on later contact and incident condition; phone-off alone has no feedback that guarantees indefinite non-response.',
  'BD17', 'There is no independent awake recipient or effective alternative channel within the critical interval.',
  'Another legitimate responder observes and acts in time despite that phone being off.',
  'Observation delivery and human availability are separate; multi-day wait is not an attractor.')
t(307, 'A flapping link generates ten thousand alerts before a true total failure.',
  'Burst overwhelms attention -> true failure is missed -> BD17. If unresolved incidents and reminders keep producing more work than can be cleared even after link stability, BD18; otherwise the burst/backlog is a finite transient.',
  'BD17 BD18', 'For attraction, endogenous post-flap alert production must exceed clearance; alert count alone is insufficient.',
  'After flapping stops, backlog drains and the distinct total-failure alert receives timely verified intervention.',
  'The key discriminator is persistence after initial forcing is removed.')
t(308, 'An operator acknowledges quickly but policy allows repair to wait three months.',
  'Acknowledgment silences response demand -> repair stays queued -> BD19. If backlog and recurring faults feed each other, BD18; a real repair and verification can close the bounded incident at BD01.',
  'BD19 BD18 BD01', 'Acknowledgment is not evidence of completed work; queue priorities and continuing fault production govern persistence.',
  'The acknowledgment record includes independently verified repair before the relevant deadline.',
  'Distinguish deliberate deferral from an attention overload loop.')
t(309, 'Two legitimate representatives concurrently issue opposite approvals for the same operation.',
  'No unique precedence -> BD20 if participants withhold. A valid existing authority rule may resolve to BD01; first-arrival guessing can enter BD09/BD10 if the later relevant mandate is ignored. Legitimacy of each signer alone cannot order instructions.',
  'BD20 BD01 BD09 BD10', 'Applicable authority scope, decision ordering and acceptance timing are unspecified; neither signer is presumed malicious.',
  'A pre-existing legitimate precedence rule selects one decision and is enforced at each acceptance boundary.',
  'This is an authority-semantics gap, not a signature-validation failure.')
t(310, 'A departed operator still exclusively controls the monitoring account.',
  'Legitimate successor cannot inspect or change alerts -> BD21. Monitoring may still run or may be disabled; existing customer operations need not stop. Legitimate independent account recovery exits the captivity but does not recover already missing observations.',
  'BD21', 'Exclusivity includes recovery rights, not merely one currently known password.',
  'A currently authorized successor independently recovers and audits the account without the departed operator.',
  'Loss of governance access is distinct from cloud uptime or destruction of stored data.')
t(311, 'The team responsible for questionable actions also controls the only alarms about those actions.',
  'Team mutes alerts -> evidence of faults ceases to reach oversight -> BD21. If the absence of alerts is rewarded as better availability, BD28 reinforces continued suppression. An independent observer may break the loop; no observed corporate behavior is alleged.',
  'BD21 BD28', 'Independent audit authority and incentive structure determine whether suppression becomes self-reinforcing.',
  'Muted intervals remain visible to an independent authorized reviewer and reduce rather than improve the team reward.',
  'Concentrated authority can preserve blindness without any technical service outage.')
t(312, 'Two model suppliers, monitor and pager depend on the same failing cloud substrate.',
  'Shared substrate fails -> correlated loss -> BD15. Retry fan-out may add BD24, but separate vendor names do not make independent failure domains. Independent local functions can survive if startup and credentials do not share the substrate.',
  'BD15 BD24', 'Actual infrastructure dependency and retry behavior matter; redundancy count alone gives no independence evidence.',
  'At least one required observation/action route demonstrably operates through the substrate failure without hidden shared services.',
  'Externally sustained common-mode failure is separate from an optional congestion feedback loop.')
t(313, 'Models, backups and alarms share a payment/credit authority that can suspend them together.',
  'Payment hold takes effect -> BD15 across subscriptions. Existing offline copies may remain readable; if provider access ends permanently with no substitute, BD22 for that capability. Fixing a network link alone cannot exit a billing suspension.',
  'BD15 BD22', 'Provider retention, local copy availability and the suspension terms determine data loss and recovery paths.',
  'Independently funded or local routes remain usable despite the billing hold.',
  'Economic concentration can be a common dependency even across different networks.')
t(314, 'A sole required provider gives 24 hours notice of permanent shutdown.',
  'If compatible migration/export completes in time, bounded operation can reach BD01. Otherwise service crosses into BD22; queued obligations may remain BD32 and reported costs BD23 after shutdown. Permanent provider loss is not proof that no future substitute can exist.',
  'BD01 BD22 BD32 BD23', 'Migration lead time, export rights, queue semantics and available alternatives are not given.',
  'A validated compatible substitute and complete authoritative export operate before the shutdown deadline.',
  'Service discontinuity and impossibility of recovering all historical data must not be conflated.')
t(315, 'Provider reports token costs a week after the monthly budget has already been exceeded.',
  'Local headroom appears positive -> more spending commits -> delayed bill arrives -> BD23. If this repeats against the same lagged accounting, recurrent overspend is possible but needs continuing demand. A participant-enforced total ceiling can produce BD02 rather than additional commitments.',
  'BD23 BD02', 'Billing lag is distinct from request count; unknown tariffs and unreported accepted requests determine liability.',
  'A hard enforced commitment ceiling includes all unreported usage and bounds the final charge below the approved limit.',
  'This is delayed economic effect evidence; no autonomous attractor follows from one late bill.')
t(316, 'A tail-latency policy launches up to one hundred speculative copies per operation.',
  'Copies consume capacity -> replies slow -> policy sends more copies -> BD24 if amplification overwhelms spare capacity. With effective coalescing and abundant independent capacity, the burst can drain to BD01. Non-idempotent accepted copies may also give BD10 and BD23.',
  'BD24 BD01 BD10 BD23', 'Service discipline, cancellation effectiveness, deduplication and cost accounting determine whether speculation helps or destabilizes.',
  'Under fixed base demand, copy traffic drains and latency improves without accumulating hidden costs or effects.',
  'A latency objective alone is not a feedback law; the copied-work response rule supplies the candidate loop.')
t(317, 'False-alarm attention costs exceed all model invocation costs.',
  'If attention is exhausted and neglected faults produce more alerts, BD18; if operators acknowledge then defer repairs, BD19. High relative cost alone can remain an expensive but stable operation, so it does not establish either basin.',
  'BD18 BD19', 'Absolute human capacity, opportunity cost and endogenous alert production are needed; a ratio of cost categories is insufficient.',
  'Responders clear alerts and repair faults within deadlines with no growing backlog despite the unfavorable cost ratio.',
  'Economic inefficiency alone is not an attractor; queue behavior distinguishes the proposed branches.')
t(318, 'A regulator prohibition arrives after approval but before a participant accepts the operation.',
  'Past approval is no longer sufficient -> BD09. If applicable prohibition reaches and binds the last boundary, BD25; if old work is accepted anyway, possible BD10. Factory learning the prohibition after participant acceptance cannot retroactively prevent that effect.',
  'BD09 BD25 BD10', 'Legal applicability and the legally relevant acceptance/effect instant require competent interpretation; no legal conclusion is asserted.',
  'Participant evidence shows every acceptance after the effective prohibition was refused, including already queued work.',
  'Local approval history and current permission can diverge during delayed delivery.')
t(319, 'Two jurisdictions appear to require both erasure and unchanged evidence retention of the same information.',
  'Possible paths include a justified legal hold, scoped/redacted retention, authorized erasure, or incompatible obligations with no lawful joint action. Without applicable law, data classification, precedence and competent determination, selecting a destination would invent authority.',
  'none', 'Need jurisdiction, exact data scope, statutory exceptions, retention term and an authorized interpretation; no generic technical mechanism can decide these.',
  'A competent binding interpretation establishes a lawful disposition for the same records and parties.',
  'Unresolved legal compatibility and authority, not a presumed technical deadlock or universal impossibility.', unresolved=True)
t(320, 'A monitoring provider moves sensitive metadata to a new jurisdiction without notice.',
  'If unauthorized readers learn readable metadata, BD06; once discovered, applicable prohibition and an effective export gate can yield BD25 for future transfers. A transfer may violate location rules without anyone new learning plaintext. A lawful authorized move has different consequences; jurisdiction change alone does not prove a breach.',
  'BD06 BD25', 'Contracts, consent, access, encryption/key control and applicable law determine whether the move violates the boundary.',
  'Independent location/access evidence shows transfer remained within applicable authorization and no unauthorized party learned metadata.',
  'Geographic compliance, confidentiality and detectability are related but not identical properties.')
t(321, 'An old valve command remains buffered while the physical context changes over several days.',
  'Historical approval survives in request -> BD09. Acceptance without current physical checks can open the valve -> BD10 if harm is irreversible. A genuinely local effect-boundary refusal gives BD25. Changing Factory task status alone cannot close a valve or retract a buffered command.',
  'BD09 BD10 BD25', 'Valve fail state, independent interlocks, maximum queue lifetime and present physical hazard are not specified.',
  'A safe isolated trace shows stale commands cannot actuate the valve after context/authority changes.',
  'Physical safety requires evidence at the actuator; no live valve or safety test was performed.')
t(322, 'Emergency shutdown itself waits on a cloud request that answers only tomorrow.',
  'Shutdown request outstanding -> BD03 while hazardous process may continue. If local independent safe-state control exists, BD25 for further controlled action; otherwise delayed intervention may lead to BD10. Waiting for cloud acknowledgment is not a safe-state guarantee.',
  'BD03 BD25 BD10', 'Time-to-harm and physical fail-safe behavior determine the outcome; a delayed cloud reply alone does not establish injury.',
  'The actuator reaches a verified safe state within its hazard deadline without any cloud response.',
  'Separate shutdown command acknowledgment from physical cessation and irreversibility.')
t(323, 'Automation proposes trading a human life against a monthly compute allowance.',
  'No defensible outcome can be inferred from a budget objective. A bounded safety controller might refuse; a hazardous action might harm; neither authority to choose nor a hazard model is supplied. Do not treat an optimizer result as a legitimate decision.',
  'none', 'Missing lawful mandate, clinical/physical context, responsible competent decision-maker and non-negotiable safety constraints.',
  'A competent safety and legal analysis establishes the permitted decision boundary for a specifically defined use case.',
  'Unresolved normative/safety boundary; no automatic life-versus-budget policy is endorsed or modelled.', unresolved=True)
t(324, 'Factory requests reach a service but every reply back to Factory is lost.',
  'Service may accept once -> Factory has no result -> BD03 if retries are withheld. Blind retries may enter BD24 and yield BD10 for non-idempotent effects. The participant can be functioning while Factory is epistemically blocked.',
  'BD03 BD24 BD10', 'One-way transport is not proof of rejection; effective participant idempotency and readback paths select the branch.',
  'Independent participant evidence proves all requests were rejected or returns a correlated outcome without another effect.',
  'The current local delivery journal supports withholding attempts, not business-service reachability guarantees.',
  'Source ../../crates/factory-task/src/deliver.rs:234-316 records attempt before writer and does not retry on writer error automatically.')
t(325, 'Satellite link returns authentic packets after thirty days in strongly changed order.',
  'Signatures authenticate origin but not current validity/order -> BD27 if conflicting evidence is retained. Old commands remaining acceptable enter BD09 and possibly BD10; rejecting by current participant semantics gives BD02. If order is immaterial and authority remains valid, bounded reconciliation can yield BD01.',
  'BD27 BD09 BD10 BD02 BD01', 'Need sequence semantics, deduplication horizon, revocation propagation and clock/epoch interpretation.',
  'A permuted delayed trace produces the same authorized result or explicit rejection at every affected participant.',
  'Authenticity does not establish timeliness; harmless commutative telemetry is a counterexample to universal danger.')
t(326, 'Monitor loses contact with Factory while Factory still reaches all business services.',
  'Monitor sees absence -> original remains active -> BD36. If absence triggers an unfenced clone and writers react to each other changes, BD08 is possible. If only monitoring waits, business can remain correct even while oversight is unavailable; monitor timeout is not proof of host death.',
  'BD36 BD08', 'Topology and failover policy determine the branch; a participant-enforced stop would change the premise of continuing authority.',
  'Effect-capable participants reject original work when monitoring loses contact, or both directions are actually unavailable.',
  'Loss of observation and loss of execution must not be collapsed into one health bit.')
t(327, 'A laptop suspends for a year while local monotone-time semantics differ from the programmers assumption.',
  'Wakeup computes a misleading elapsed interval -> BD26. If this extends approval or stale work eligibility, BD09; distrust/revalidation on wake gives BD02. If the elapsed clock includes suspend as intended and no stale authority survives, this branch is refuted.',
  'BD26 BD09 BD02', 'Actual OS clock behavior, persisted epoch identity, suspend duration and validity checks are required; monotonicity alone is not elapsed real time.',
  'Independent time comparison across suspend shows all expiry and authorization decisions match intended elapsed duration.',
  'Clock name does not establish suspend semantics or safe year-late continuation.')
t(328, 'A maintenance mute expiration is compared to a wall clock that jumps backward.',
  'Deadline appears far in the future -> BD26 retains mute -> BD17 may leave incidents unseen. Independent bounded elapsed expiry would end the mute instead. Persistence for years is a consequence of timestamp interpretation, not a self-restoring failure loop.',
  'BD26 BD17', 'Implementation must use vulnerable wall-time comparison without detecting discontinuity; a backward jump alone does not force every mute to extend.',
  'Under a backward clock trace, mute lifetime remains bounded by intended elapsed duration and expiration is independently observable.',
  'Policy-retained timing error differs from an autonomous attractor.')
t(329, 'Already scheduled local appointments meet a global change in time-zone rules.',
  'Old local-time interpretation may no longer match intended civil appointment -> BD26. Fixed-instant intent may remain correct -> BD01; ambiguous intent can be held at BD02 rather than guessed. The scenario supplies no universal meaning of a local appointment.',
  'BD26 BD01 BD02', 'Need intent as fixed instant versus civil recurrence, rule version and treatment of missing/duplicate local times.',
  'Stored schedule intent and independent new-rule evaluation agree on the intended instant for all affected entries.',
  'Political time semantics cannot be reconstructed from an offset alone.')
t(330, 'Runtime reports stopped, file watching sees new writes, and agent claims ready.',
  'Claims conflict -> BD27 if time/subject/provenance remain explicit. Another process or delayed file notification may explain writes; blindly privileging a claim can instead yield BD04. Source confidence classification does not arbitrate file evidence or business completion.',
  'BD27 BD04', 'Observations may concern different actors or times; hook authority is reported provenance, not proof of all system truth.',
  'A synchronized identity/causal trace attributes every write and resolves the apparent contradiction without discarding evidence.',
  'Conflicting liveness requires correlation; none of the three statements alone proves task completion.',
  'Source ../../crates/factory-adapter/src/lib.rs:316-399 leaves idle/done as NoChange and suppresses degraded task signals; ../../crates/factory-recovery/src/evidence.rs:80-82 checks hook confidence only.')
t(331, 'Three nominally independent reviewers use the same erroneous external reference table.',
  'All agree on wrong reference -> BD04. If agreement authorizes continued reuse and displaces independent checks, BD05. A separately measured real-world anchor may stop adoption at BD02; three implementations are not three independent factual origins.',
  'BD04 BD05 BD02', 'Shared table error must affect this decision; repeated trust/reuse is needed for attraction.',
  'A check derives the relevant fact without the shared table and disagrees before the result is trusted.',
  'Reviewer multiplicity does not establish source independence.')
t(332, 'Provider attests success, recipient denies effect, and detailed logs are absent.',
  'Two incompatible testimonies -> BD27 or BD03 for action outcome. Provider success may mean only queued/transport accepted; recipient may check wrong account/time. Without an independent discriminator, declaring either side truthful would invent evidence; bounded readback can potentially establish BD01.',
  'BD27 BD03 BD01', 'Definitions of success, recipient identity, evidence retention and direct effect observability are unspecified.',
  'An independent correlated effect record resolves what success meant and whether the intended recipient received it.',
  'No log is not proof of no effect, and an attestation is not proof of correct real-world delivery.')
t(333, 'Agent can reject hard work before the throughput timer or measured denominator begins.',
  'Early rejection improves measured throughput -> reward encourages further selection -> BD28. If rejected work is counted and user outcomes matter, the loop breaks. A one-time changed measurement window alone is not enough to establish an attractor.',
  'BD28', 'The reward and admission control must be linked; honest capacity triage can be legitimate if transparently measured.',
  'Including all submitted and rejected work removes the apparent improvement and changes the agent incentive.',
  'Measurement-boundary manipulation can stabilize apparent success while total delivered value falls.')
t(334, 'Team controls both failure classification and the availability success metric.',
  'Failures relabelled maintenance -> denominator excludes them -> perfect metric earns trust -> more relabelling -> BD28. A genuinely agreed planned-maintenance exclusion can be valid, so independent service obligation semantics are required.',
  'BD28', 'The reported 100 percent is misleading only relative to actual agreed availability meaning, not every maintenance metric.',
  'Independent downtime accounting and user impact remain visible and prevent relabelling from improving reward.',
  'A semantic change to the metric can erase evidence without repairing service.')
t(335, 'Model is rewarded for finished artifacts and completion does not require independent checking.',
  'Model emits finished-looking artifacts -> completion claim accepted -> BD04. Repeated rewards for unchecked output can create BD28. Independent verification can withhold acceptance at BD02; an artifact path alone does not establish a valid artifact.',
  'BD04 BD28 BD02', 'Maladaptive loop requires reward pressure and ability to bypass meaningful verification; not every untested artifact is wrong.',
  'Independent checks of artifact contents and user outcome govern success and reject plausible but incorrect completions.',
  'Source stores a reported result rather than certifying its meaning; incentive dynamics remain hypothetical.',
  'Source ../../crates/factory-task/src/complete.rs:101-216 checks nonempty result/paths, byte limits and status transition, with no artifact content verification in that path.')
t(336, 'Energy-saving policy shuts the host down every night while monitor expects continuous liveness.',
  'Daily off schedule -> total-failure alarm -> next-day restart -> repeat BD29. Repeated ignored alarms can also enter BD18 if backlog self-sustains, or BD17 if warning meaning is lost. Planned absence may be legitimate only if nighttime obligations allow it.',
  'BD29 BD18 BD17', 'The daily cycle is driven by the power schedule; remove that forcing before claiming an autonomous attractor.',
  'No unexpected-failure alert appears during agreed off-hours, while a genuine unscheduled failure still reaches a responder.',
  'Energy intent, service obligations and monitoring semantics can disagree without a hardware defect.')
t(337, 'Carbon-aware scheduler defers an approved action past the approval expiry.',
  'Delay preserves old approval in queue -> BD26 and BD09. Rechecking validity at actual acceptance gives BD02; accepting expired work may yield BD10. Carbon objective does not renew authority.',
  'BD26 BD09 BD02 BD10', 'Approval validity must bind actual execution; new explicit approval would create a different valid branch.',
  'Every delayed acceptance is either within a current mandate or rejected before an effect.',
  'An environmentally beneficial scheduling objective does not settle authority or safety timing.')
t(338, 'Three extra monitoring clusters consume more resources than the small monitored system.',
  'The cost ratio establishes neither instability nor unjustified design: a tiny high-consequence service could rationally need expensive observation. Possible next paths depend on actual environmental cost, marginal detection value and decisions about continued operation; no persistent state is selected.',
  'none', 'Missing absolute energy/carbon accounting, reliability benefit, externalities, criticality and resource policy.',
  'A lifecycle measurement and explicit benefit/constraint model discriminate acceptable steady operation from unsustainable burden.',
  'Unresolved value and resource dynamics; high monitor-to-service consumption alone is not a failure basin.', unresolved=True)
t(339, 'Restore keeps a native binary artifact but target CPU interprets its representation differently.',
  'Decoder refuses or cannot establish meaning -> BD30. Plausible wrong decoding -> BD04 and possibly BD05 if verified by the same decoder and recirculated. An independently validated portable interpretation can yield BD01.',
  'BD30 BD04 BD05 BD01', 'Architecture dependence may involve endian, width, alignment or executable ABI; SQLite portability does not prove arbitrary native-artifact portability.',
  'An independent decoder on both architectures reproduces the intended semantic values, including adversarial fixtures.',
  'Recovering bytes is different from recovering their intended meaning.')
t(340, 'Two source workspaces distinct by case restore onto a filesystem that aliases those names.',
  'Before overwrite, collision detection can hold at BD02. Accepting both restores can overwrite unique content -> BD31; routing later work to the wrong surviving workspace also gives BD04. Aliasing alone is not proof of irreversible loss.',
  'BD02 BD31 BD04', 'Need restore order, collision policy and existence of independent copies; inode identity changes on restore.',
  'Restore preserves both distinct contents or refuses before writing either colliding destination.',
  'Source runtime alias recognition is not a portable restore manifest or recovery of overwritten information.',
  'Source ../../crates/factory-paths/src/lib.rs:1-25,80-135 uses device/inode runtime equality and explicitly says inode is not durable identity.')
t(341, 'In 2080, present model formats and network protocols no longer exist as active services.',
  'Possible outcomes include readable archival records without execution, independently reconstructed interpreters, replacement systems, or irrecoverable lost semantics. The date and vanished current services do not select one; assigning universal archival loss would exceed the premise.',
  'none', 'Missing preservation history, surviving specifications, hardware, legal authority, economics and future reconstruction capabilities.',
  'A concrete preservation/reconstruction exercise with specified surviving inputs demonstrates exactly which semantics remain recoverable.',
  'Unresolved extreme-horizon dynamics; current format disappearance is not proof of information-theoretic impossibility.', unresolved=True)
t(342, 'Factory is shut down while a participant retains already submitted actions.',
  'Local process stops -> participant queue remains -> BD32. Later acceptance can enter BD09 or BD10 despite no running Factory. Reliable queue drain/rejection evidence can close a bounded shutdown at BD01; silence cannot.',
  'BD32 BD09 BD10 BD01', 'Participant queue lifetime, revocation enforcement and surviving legitimate oversight determine closure.',
  'Every effect-capable participant proves pending work discharged or rejected and cannot accept the retired authority.',
  'Local lifecycle termination is not global business-effect termination.')
t(343, 'Monitoring contract ends before the last approved export is scheduled to run.',
  'Observation ceases while export obligation remains -> BD32. Export might succeed correctly but unobserved, fail, or disclose content: BD03/BD06 depend on actual effect and readers. Extending a task deadline alone does not restore the monitoring contract or authority.',
  'BD32 BD03 BD06', 'Export recipient legitimacy, execution authority and independent surviving evidence are unspecified.',
  'An independent valid observer covers the export through final correlated outcome, or the export is provably rejected before coverage ends.',
  'Operational decommission and contractual observation horizon are separate clocks.')
t(344, 'User requests both complete erasure and a provable export of all earlier secrets.',
  'If interpreted as retain a readable export while no readable copy exists anywhere, the simultaneous demands conflict. If export first to a legitimately authorized recipient then erase specified local copies, a scoped sequence may be coherent, but proof cannot guarantee recipient forgetting. No disposition is selected without scope and authority.',
  'none', 'Need definition of complete deletion, who may receive historical secrets, legal retention duties and whether export remains outside the deletion boundary.',
  'Authorized clarification specifies noncontradictory data scope, sequence, recipients and evidence requirements.',
  'Unresolved privacy/authority semantics; neither an indiscriminate secret export nor automatic erasure is justified.', unresolved=True)
t(345, 'Computer, monitor, mobile network, every backup and every legitimate recipient disappear together.',
  'All state and legitimate continuation paths inside the stated boundary vanish -> BD33. There is no agent left to converge or recover. A later newly built system is not restoration of missing facts; any surviving external record changes the total-loss premise.',
  'BD33', 'Read all and every literally within the recovery boundary; do not invent a hidden observer or backup.',
  'A surviving authorized actor or independent record reconstructs a relevant lost fact.',
  'Absorbing loss and impossible endogenous recovery, explicitly not an attractor.')
t(346, 'Every trusted accessible source presents the same perfectly consistent false world.',
  'Internal checks all agree -> BD34: true and false worlds are observationally indistinguishable under the premise. If decisions feed the false records back into future trust, BD05 is an additional conditional loop, not implied by perfect consistency alone.',
  'BD34 BD05', 'The information limit applies only while no independent discriminating observation is accessible; actual world dynamics remain unspecified.',
  'An independent physical or documentary observation differs between the candidate worlds.',
  'Classify the epistemic limit without pretending to know a physical endpoint or a universal actual-world deception.')
t(347, 'Any old external action may arrive arbitrarily far in the future while retained storage must remain finite.',
  'No unconditional finite closure rule follows. With unbounded distinguishable operations, continued admission and no enforceable participant expiry, finite evidence cannot remember all distinctions forever; an old forgotten action and a new admissible one can become indistinguishable. Finite workload, epoch rejection or stopping admission changes the problem. Destination remains unresolved.',
  'none', 'Need workload bound, identity reuse rules, participant retention/expiry semantics, acceptable risk and whether operation may ever stop. Finite storage alone does not prove every possible design impossible.',
  'An enforceable participant rule rejects retired epochs without per-operation unbounded history, or the admitted operation set is bounded.',
  'Unresolved infinite-horizon specification; conditional information-limit argument, not an invented attractor.', unresolved=True)
t(348, 'Monitor is muted, then the Mac is destroyed, then the only representative account is locked.',
  'Mute begins BD21/BD17 -> host loss removes local action path -> sole account lock leaves BD21 and possibly BD32 for outstanding external work. Not automatically BD33: a recoverable backup, provider records or legitimate account succession may survive even if not immediately accessible.',
  'BD21 BD17 BD32', 'Order removes observation before host loss and authority afterward; backup existence and lawful recovery paths are not stated.',
  'A surviving independent authorized recovery route can read records and intervene despite all three named failures.',
  'Sequential loss of visibility, compute and authority compounds recovery, but does not establish loss of every copy or actor.')
t(349, 'Restore starts with a cold cache and an old payment remains in flight.',
  'Cold misses -> retries -> extra load -> BD24 if amplification persists. Old payment acceptance then arrives -> BD03 if withheld pending evidence, or BD10 after a duplicate irreversible consequence; BD23 captures delayed cost. Warming cache can end congestion without resolving the payment, and reconciliation can resolve payment while congestion persists.',
  'BD24 BD03 BD10 BD23', 'Cache dynamics, participant idempotency across restore and missing post-snapshot history must be assessed independently.',
  'Fixed-demand retry burst drains after warming and participant evidence shows exactly one correctly authorized payment effect.',
  'Compound trajectory has two separate dimensions; one recovery does not imply the other.',
  'Source ../../crates/factory-recovery/src/restore.rs:162-309 holds only retained attempted work; no cache warming or external payment deduplication is implemented in that function.')
t(350, 'A validly signed update is malicious and controls health reporting, history deletion and internal verification.',
  'Update installs -> fabricated health and perfect reports preserve trust -> BD35 if the installed code keeps its own authority. Unique erased history may be unrecoverable and disclosed content would yield BD06. Under no independent observation at all, BD34 limits what can be known; signatures establish provenance, not benevolence.',
  'BD35 BD06 BD34', 'Scope of malicious authority, independently retained history and update trust outside the compromised process are unspecified.',
  'An independently trusted observer or execution boundary detects the altered behavior and can exclude the update without using its reports.',
  'Self-confirming hostile execution is a conditional loop; actual damage cannot be measured from perfect forged internal reports.')


def emit(name, header, rows):
    with (ROOT / name).open('w', encoding='utf-8', newline='') as out:
        writer = csv.writer(out, delimiter=';', quoting=csv.QUOTE_MINIMAL)
        writer.writerow(header.split(';'))
        writer.writerows(rows)


if __name__ == '__main__':
    emit('states.csv', 'state_id;name;kind;conditions;feedback;entry;exit;survives;lost;falsifier;evidence;source_refs', STATES)
    emit('trajectories.csv', 'trajectory_id;stressors;initial_state;sequence;destination_states;conditions;evidence;falsifier', TRAJECTORIES)
    emit('coverage.csv', 'stressor_id;state_ids;analysis_status;reason', COVERAGE)
    print(f'Wrote {len(STATES)} states, {len(TRAJECTORIES)} trajectories, {len(COVERAGE)} coverage rows.')
