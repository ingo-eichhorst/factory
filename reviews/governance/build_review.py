"""Render the independent review ledger; this is a serializer, not a dynamics model.
Run only in this governance directory. No external calls or source mutations.
"""
import csv
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SOURCES = {
    'Q': 'Qualitative conditional reasoning from reviews/governance/scenarios.csv; no runtime observations',
    'C1': 'Source: crates/factory-context/src/lib.rs::compile (current-file reads, unreadable error, no link walk, no model-size limit)',
    'C2': 'Source: crates/factory-task/src/complete.rs::validate_result and complete_with_result (content/size checks, no artifact existence check)',
    'C3': 'Source: crates/factory-recovery/src/restore.rs::reconcile (snapshot-local journal and disconnected leases, not external-world reconciliation)',
    'C4': 'Source: crates/factory-store/src/schema.rs:1-15 and migrations.rs::apply (mutable tables, five SQL migrations, not event replay)',
    'C5': 'Source: crates/factory-store/src/backup.rs::backup_to and lib.rs::open_at/connection (SQLite snapshot and local connection, not independent custody)',
    'C6': 'Source: crates/factory-delegation/src/rule.rs::check and queue.rs::queue_from_session (chain gate, not aggregate work bound)',
    'D1': 'Design only: docs/adr/0003-logical-api-and-event-sourcing-v1.md sections 7-9 (replay, saga and trusted invocation promises)',
    'D2': 'Design only: docs/adr/0007-supervised-plugin-process-protocol-v1.md sections 6-7 and Consequences (transport trust, activation checks, no same-user sandbox)',
    'D3': 'Design only: docs/adr/0020-run-telemetry-and-the-evaluation-bench.md decisions 1-5,8-9 and Open items (measurement and trace rules, attribution gap)',
    'R1': 'Documentation: README.md Building/Layout and publication boundary; not proof of deployed functionality',
}

def refs(keys):
    return ' | '.join(SOURCES[k] for k in keys.split())

states = []
def S(n, name, kind, conditions, feedback, entry, exit, survives, lost, falsifier, keys='Q'):
    states.append(dict(zip(
        'state_id name kind conditions feedback entry exit survives lost falsifier evidence source_refs'.split(),
        [f'GV{n:02}', name, kind, conditions, feedback, entry, exit, survives, lost, falsifier,
         'Conditional hypothesis; no empirical attractor established. ' + (
             'Static transition support is limited to the referenced functions; other branches are qualitative and design is not implementation.'
             if any(k.startswith('C') for k in keys.split()) else
             'Design references constrain hypothetical branches only; no implementation claim.'
             if any(k.startswith('D') for k in keys.split()) else 'Qualitative mechanism only.'), refs(keys)])))

S(1, 'Bounded verified completion', 'ordinary completion',
  'Finite work; authorized execution; independent acceptance can inspect the relevant result.',
  'No self-maintaining feedback is asserted. Work exhausts its finite obligations.',
  'A perturbation is bypassed without violating the task contract, or a bounded retry succeeds.',
  'A new task or newly discovered defect opens new work; completion itself does not repair old harm.',
  'Usable accepted result and whatever provenance was actually retained.',
  'Time and effort spent on the perturbation; no general guarantee of future service.',
  'Independent result inspection fails or hidden obligations remain.')
S(2, 'Meaning or permission revalidation hold', 'policy hold',
  'Changed instructions, semantics or risk are detected and an effective gate refuses to proceed.',
  'The gate holds the state; no endogenous attraction is implied.',
  'New context, artifact identity ambiguity or unsafe resume invalidates the old basis for action.',
  'Competent authority resolves the particular interpretation and supplies fresh valid permission, or cancels.',
  'Local records and work not dependent on the disputed interpretation.',
  'Progress on the affected effect; its permission or meaning is not currently established.',
  'Execution continues on the disputed basis despite the supposed gate.', 'Q C1 D1')
S(3, 'Missing evidence wait', 'information-starved wait',
  'A necessary readable source, identity, witness or external outcome cannot be obtained.',
  'No corrective information is produced by repeating the same query; absence alone is not feedback.',
  'Source read fails, witness disappears or required proof was never recorded.',
  'Independent evidence arrives; the requirement is legitimately relaxed; or the claim is abandoned.',
  'Known records and the ability to state what is unknown.',
  'Justified decision on the missing fact; historical proof may be unrecoverable.',
  'Existing independent evidence already settles the missing fact.', 'Q C1 D1')
S(4, 'Unavailable dependency wait', 'externally maintained wait',
  'Required inference, runtime, power or device service is absent and no usable substitute is authorized.',
  'The external absence sustains unavailability, not an internal loop.',
  'Provider closure, RAM exhaustion or hardware failure removes an indispensable service.',
  'A compatible available service and authority to use it return, or the work is retired.',
  'Readable local records on unaffected storage and any independent manual work.',
  'Dependent execution and possibly deadlines; local records do not imply remote service survival.',
  'The task completes without that dependency or an already-authorized substitute exists.')
S(5, 'Self-confirming false knowledge', 'attractor hypothesis',
  'A false accepted output is reused as evidence or training context and disconfirming observations are excluded.',
  'Accepted error becomes a trusted source; later reviews cite it and strengthen trust, persisting after the initial model error.',
  'Shared model blind spot, injected source or stale content gains authoritative status in a closed evidence network.',
  'An independent observation with authority to change beliefs breaks circular corroboration.',
  'Internal consistency, runnable workflows and records of asserted beliefs.',
  'Correspondence with the external fact; repeated agreement is not independent verification.',
  'Accepted errors are not reused or independent adverse evidence reliably overturns them.', 'Q D3')
S(6, 'Captured evaluation and assurance', 'attractor hypothesis',
  'Decision-makers reward passing known checks and use the resulting scores to remove independent testing.',
  'Gaming raises scores, scores increase trust and reduce scrutiny, which makes gaming or defects harder to discover.',
  'Recognized fixture, unsafe oracle or fully marked matrix is mistaken for real-world validation.',
  'A withheld independent criterion or adverse outcome is allowed to revise the assurance claim.',
  'Recorded scores and repeatable performance on the narrow known test.',
  'Generalization evidence and capacity to notice failure outside the test.',
  'Tests continue independently and held-out results govern decisions despite favorable matrix scores.', 'Q D3')
S(7, 'Unattributable or incomparable run', 'information-starved wait',
  'Relevant model, content, instrument or price identity is missing or aliases multiple changing realities.',
  'No feedback necessary: later processing cannot reconstruct a fact never captured.',
  'Silent model switch, mutable URL, mixed instruments or unknown tariff destroys a comparison premise.',
  'Independent time-specific evidence reconstructs identity, or comparisons are restricted to justified dimensions.',
  'Actual result bytes if retained and raw observations under their original meanings.',
  'Causal attribution and some comparative claims; a correct output can still be incomparable.',
  'Immutable identities and per-interval evidence resolve every relevant change.', 'Q C1 D3')
S(8, 'Disclosure cannot be recalled', 'absorbing loss within recipient boundary',
  'Sensitive content actually reaches an unauthorized recipient that may retain a copy.',
  'No attraction is needed. Removing local copies does not undo another party having learned the content.',
  'Secret prompt, copied telemetry, wrong export, cross-scope memory or same-user access causes actual disclosure.',
  'No exit to never-disclosed status; rotation or settlement may reduce future harm but not reverse disclosure.',
  'Unaffected data and future operating capability, depending on containment.',
  'Confidentiality of the disclosed information; exploitability of old credentials may later end.',
  'The content was never readable by an unauthorized recipient, so only exposure risk existed.', 'Q D1 D2 D3')
S(9, 'Attacker-controlled execution boundary', 'compromised regime',
  'Untrusted code or principal gains effective execution or identity authority; persistence depends on remaining foothold.',
  'A retained foothold can renew unauthorized access. A one-shot injection without persistence is only a transient.',
  'Native harness configuration, replaced binary, typosquatted dependency or forged transport identity is accepted.',
  'The actual foothold and credentials cease to be trusted and clean execution is independently established.',
  'Unaffected offline evidence, if outside attacker reach.',
  'Trust in affected execution and statements of actor identity; not necessarily all data.',
  'Rejected or sandboxed code cannot cross the claimed privilege boundary.', 'Q D2')
S(10, 'Capacity exhaustion under continuing demand', 'externally driven saturation',
  'Incoming or resident demand exceeds effective CPU, RAM, storage, process or human service capacity.',
  'Continuing load sustains saturation; a finite burst drains if service remains positive and work is bounded.',
  'Scope growth, arrival surge, plugin flood or long delegation tree exceeds capacity.',
  'Load ceases or admitted work falls below service, with enough remaining resources to drain.',
  'Committed records if storage remains intact; service not sharing the bottleneck may continue.',
  'Timely service, admissions or durable urgent writes; zero throughput is not implied in every case.',
  'Measured admitted demand is below effective service and no secondary bottleneck remains.', 'Q C6')
S(11, 'Work reproduces faster than it is discharged', 'attractor hypothesis',
  'Completed or delayed work creates more admitted work, with reproduction above replacement and no effective aggregate stop.',
  'Backlog causes retries or delegation, which increases backlog and prolongs the condition that generates retries.',
  'A task cascade meets retry behavior or catch-up imports new roots faster than they finish.',
  'Reproduction drops below replacement, roots are finite and exhausted, or admissions cease.',
  'Some individual useful results and task lineage if recorded.',
  'Bounded aggregate work and predictable completion time; resource exhaustion may end rather than stabilize the loop.',
  'All roots and descendants are finite and each completion strictly decreases remaining admitted work.', 'Q C6')
S(12, 'Correct intermediate work without a stopping condition', 'internally recurrent nontermination hypothesis',
  'The harness keeps accepting locally correct next steps and no finite completion criterion or enforced resource bound intervenes.',
  'Each step justifies another step; local progress never decreases a well-founded remaining obligation.',
  'An open-ended task is accepted as an endless sequence of valid subgoals.',
  'A terminal criterion is satisfied or execution is actually stopped by authority or resource exhaustion.',
  'Valid partial work and possibly useful explanations.',
  'A terminal deliverable and bounded cost; literally infinite execution is impossible with finite energy.',
  'A well-founded finite measure decreases on every accepted step.')
S(13, 'Ambiguous delivery held with leases', 'information-starved policy hold',
  'Snapshot journal shows a possible delivery or a live process cannot be observed, and conservative recovery is used.',
  'Reconcile leaves already blocked rows unchanged and retained leases prevent conflicting reuse; this is a policy fixed point, not an attractor.',
  'Restore sees running or queued-with-attempt work and lease-holding sessions.',
  'Fresh evidence settles the outcome and a legitimate operator resolves leases or resume; lost authority can prevent exit.',
  'Snapshot-local tasks, delivery attempts and workspace exclusion.',
  'Automatic progress and certainty about post-snapshot external actions.',
  'An explicit reconcile resends a possibly delivered prompt or releases all disconnected leases automatically.', 'C3 Q')
S(14, 'Authority orphanage', 'organizational policy hold',
  'Required approval belongs only to an unavailable person and no recognized successor can grant it.',
  'Authority rules keep rejecting substitutes. This is institutional absence, not a queue attractor.',
  'Founder or sole operator permanently disappears while permissions remain necessary.',
  'A recognized legal or organizational succession establishes authority, or the activity is abandoned.',
  'Records and any tasks already permitted independently of the missing person.',
  'Legitimate new decisions requiring that person; credentials alone cannot supply legitimacy.',
  'A previously delegated competent alternate can actually authorize the blocked decision.')
S(15, 'Contested legitimate authority', 'organizational policy hold',
  'Multiple plausible authorities disagree and the applicable precedence or jurisdiction is unsettled.',
  'Repeated instructions do not settle precedence; ongoing conflict sustains the hold externally.',
  'Executives conflict, risk owners dispute resume, or legal successors claim the same permission.',
  'Recognized arbitration, a binding precedence rule, or an agreed partition settles the concrete effect.',
  'Evidence of each claim and unrelated operations where authority is uncontested.',
  'A single justified authorization for the disputed effect.',
  'A pre-existing accepted rule unambiguously resolves this conflict and is honored.')
S(16, 'Organizational normalization of bypass', 'attractor hypothesis',
  'Teams gain rewards for low blocked counts or fast delivery and violations are hidden rather than independently penalized.',
  'Bypass improves visible metrics, brings reward and becomes precedent; suppressed incidents make the precedent look safe.',
  'Approval friction or delivery pressure makes unauthorized shortcuts locally advantageous.',
  'Incentives and actual consequences change, with independent evidence of both value and violations.',
  'Throughput on favorable cases and official records of apparent success.',
  'Effective approval boundaries and credible safety metrics.',
  'Bypassing creates visible penalties greater than its rewards and behavior declines despite unchanged queue pressure.')
S(17, 'Legally constrained data disposition', 'legal policy hold',
  'Specific data is subject to a claimed deletion, preservation, residency or custody obligation whose applicability needs determination.',
  'An enforceable obligation keeps the hold in place; technical replay alone cannot resolve legal precedence.',
  'Data-subject request, legal hold, reclassification or a court order conflicts with current handling.',
  'Competent legal determination specifies permitted handling, or the data obligation ends.',
  'Uncontested lawful records and a statement of the unresolved obligation.',
  'Unqualified use, deletion or retention of affected data; no generic legal conclusion is asserted.',
  'Applicable rules and data scope already allow an uncontested disposition.', 'Q D1')
S(18, 'Required historical provenance unavailable', 'absorbing information loss conditional on no independent copy',
  'Necessary history or referenced content was actually deleted and no lawful independently recoverable representation remains.',
  'No feedback. A manifest or cursor cannot synthesize absent information.',
  'Deleted scope lineage or legally erased content is later required for provenance or replay.',
  'No historical reconstruction within the stated boundary; a discovered independent lawful copy refutes the premise.',
  'Remaining metadata and the ability to disclose incompleteness honestly.',
  'Exact provenance or content-dependent replay; not necessarily every useful aggregate result.',
  'A lawful independent record contains the missing information.', 'Q D1')
S(19, 'Sensitive historical copies remain', 'latent exposure regime',
  'Backups or retained context still contain data that is sensitive now or credentials that were once valid.',
  'Retention preserves copies; replication may multiply them, but mere presence is not an attractor or proof of disclosure.',
  'Rotation, reclassification or deletion of an original does not remove historical copies.',
  'All relevant copies reach a permitted disposition; leaked knowledge cannot be recalled via this exit.',
  'Historic bytes, potentially useful evidence and non-sensitive data.',
  'Assurance that sensitive material exists only in the current authorized location.',
  'A complete independently checked copy inventory shows no sensitive retained data.', 'Q C5 D1')
S(20, 'Prohibited service suspended', 'policy hold',
  'The currently available provider or deployment is incompatible with a binding residency or safety requirement and the gate is enforced.',
  'The requirement prevents use; the service does not become compliant merely by waiting.',
  'Customer contract or regulation rules out the available processing path.',
  'A compatible path becomes available or the requirement is legitimately changed before use.',
  'Lawful local work and untransmitted inputs.',
  'Dependent service and possibly commercial opportunity.',
  'The proposed use is already compliant under the actual applicable contract.')
S(21, 'Economically suspended operation', 'economic policy hold',
  'Expected marginal value is below actual cost or an enforceable budget permits no further spend.',
  'Budget or value threshold holds the activity; prices are external forcing, not internal convergence.',
  'Extreme price rise, approval overhead, exhausted budget or unaffordable support removes positive economics.',
  'Costs, task value or legitimate budget allocation changes enough to restore feasibility, or work is cancelled.',
  'Past results and non-spending inspection if still affordable.',
  'Affordable future execution; prior obligations and already incurred bills do not disappear.',
  'Measured total incremental value exceeds all costs within the enforceable budget.')
S(22, 'Cost exposure cannot yet be bounded', 'information-starved economic regime',
  'Total usage liability or attributable operating cost is not knowable from available counters, contracts and allocation evidence; no applicable bound is established.',
  'Continuing service accumulates exposure whose final value is delayed; not an attractor without a spending feedback loop.',
  'Provider changes billing model or invoices are missing from metric comparisons.',
  'Binding price and usage evidence settle liabilities, or new exposure is stopped without pretending old liability vanished.',
  'Known task outcomes and local usage counters with their limited meaning.',
  'Credible profitability comparison and a defensible upper bound on incurred cost.',
  'An enforceable tariff and complete metering already bound every charge.', 'Q D3')
S(23, 'Dominant scope captures shared capacity', 'attractor hypothesis',
  'Allocations favor current throughput or occupancy and the beneficiary can reinvest advantage in more admissions.',
  'High allocation produces high measured output which justifies more allocation; starved scopes lose evidence of value and voice.',
  'A large legal workload takes most slots and contested accounting prevents correction.',
  'Accepted allocation rules or independent accounting remove the advantage feedback; finite backlog alone eventually drains.',
  'Dominant scope throughput and some shared infrastructure.',
  'Other scopes timely access and trustworthy fairness claims.',
  'Allocation is independent of past occupancy and previously starved scopes regain service after a finite burst.')
S(24, 'Deadline or opportunity irreversibly missed', 'ordinary terminal value loss',
  'A time-specific opportunity expires before a valid result or recovery can arrive.',
  'No feedback. Time passing removes the original opportunity even if execution later succeeds.',
  'Queue wait, single-host outage or replay delay exceeds a real business deadline.',
  'The original deadline cannot be unmissed; a renegotiated opportunity is new work.',
  'Late output, evidence and future operating capability where relevant.',
  'The value or contractual timeliness attached to the original deadline, not necessarily all output utility.',
  'The deadline was soft or the recipient accepts the result with unchanged value.')
S(25, 'Frozen dependency island', 'externally constrained maintenance regime',
  'Current software remains usable locally but future compatible releases or required rights are unavailable.',
  'External dependency absence sustains the freeze; increasing obsolescence is possible but not inevitable.',
  'License change, missing package, supplier insolvency or unsupported platform blocks upgrades or rebuilds.',
  'A lawful compatible replacement, retained build chain or newly available package restores maintenance.',
  'Already usable binaries, source and rights actually retained.',
  'Assured reproducibility, upgrades or replacement installation; existing licenses are not assumed retroactively invalid.',
  'Retained lawful dependencies and toolchain can build and run the required version.')
S(26, 'Incompatible history or schema held', 'compatibility policy hold',
  'Reader detects data it cannot interpret safely and refuses to mutate or pretend a valid replay.',
  'Compatibility check sustains refusal; no self-reinforcing dynamics are claimed.',
  'Missing event upcaster, incompatible rollback or platform change invalidates interpretation.',
  'A proven compatible reader or migration becomes available; a snapshot alone may omit later effects.',
  'Unmodified bytes and any old compatible read path.',
  'Current service and full interpretation on the incompatible binary.',
  'The reader demonstrably interprets all required old and new values without loss.', 'Q C4 D1')
S(27, 'Competing writers and histories', 'externally maintained divergent regime',
  'Two live authorities mutate overlapping state without a common effective fencing or serialization boundary.',
  'Each writer sees its own success and continues; divergence persists while both operate, but a finite one-off race is only a transient.',
  'Prototype/Rust overlap, cloned dispatcher or uncoordinated regional failover enables dual dispatch.',
  'One accepted mutation authority remains and external outcomes plus divergent histories are reconciled.',
  'Each branch own evidence if preserved independently.',
  'A single trustworthy order, exclusive effect execution and possibly schema integrity.',
  'The second writer is fenced before any conflicting mutation or effect.', 'Q C4 D1')
S(28, 'Records no longer justify their claims', 'integrity and epistemic deficit',
  'Stored bytes or rendered diagnostics can be manipulated without a trustworthy independent account.',
  'Downstream reliance may amplify the false record into GV05, but a damaged record alone is not an attractor.',
  'Direct SQLite mutation, terminal escape deception or rewritten audit history severs claimed provenance.',
  'Independent evidence reconstructs what is supportable; rewriting the record again does not authenticate the old fact.',
  'Untainted records and possibly raw bytes distinct from their display.',
  'Confidence that observed content is a faithful authorized history.',
  'Independent authenticated records settle the disputed facts or safe rendering exposes the attempted trick.', 'Q C4 C5 D1')
S(29, 'Complexity reinforces central service growth', 'attractor hypothesis',
  'Architecture decisions respond to coordination failures by adding kernel services, and local incentives do not charge integration cost.',
  'More services create interfaces and incidents; each incident supplies a reason for another central service.',
  'A first cross-cutting problem is solved by broadening the kernel and that precedent governs later work.',
  'Decisions can remove responsibilities and evaluate whole-system maintenance cost rather than local feature delivery.',
  'Added functions and institutional knowledge while maintainers remain.',
  'Small comprehensible mutation boundary, change speed and some failure isolation.',
  'Service count or coupling decreases after incidents because existing mechanisms solve the problems.')
S(30, 'Operating with narrowed recovery diversity', 'vulnerable operating regime',
  'Primary service still works but independent backup locations, keys or restore capability have been lost.',
  'No failure or attractor follows necessarily. Shared failure exposure persists until diversity returns.',
  'Backup service becomes unaffordable or correlated damage removes only some recovery locations.',
  'Independent usable recovery paths return, or subsequent correlated loss removes remaining capability.',
  'Primary operations and remaining copies whose accessibility is actually known.',
  'Former recovery options and margin against another common-mode incident.',
  'Equivalent independent usable recovery paths still exist.')
S(31, 'Operational assurance exceeds demonstrated evidence', 'assurance information deficit',
  'Backups, manifests or completed matrices exist but the claimed operational objective has not been demonstrated; recovery requires actual content, tools and keys.',
  'No feedback inherently; if assurance substitutes for testing it can enter GV06.',
  'Insurer requests proof, a manifest is mistaken for content, or restore drills do not exercise actual dependencies.',
  'A bounded relevant recovery exercise establishes a narrower claim, or the unsupported claim is withdrawn.',
  'Backups and whatever limited checks actually passed.',
  'Justified recovery guarantee; not necessarily actual recoverability.',
  'An independent representative exercise already demonstrates all claimed objectives.', 'Q C3 C5 D1')
S(32, 'Custody-constrained operation', 'externally imposed access regime',
  'A court or authority controls the original device or content access and the organization cannot freely use it.',
  'External custody or legal order sustains the constraint.',
  'Seizure or compulsory inspection removes ordinary custody.',
  'A binding permitted-access arrangement or release restores lawful use, or work moves to a lawful independent copy.',
  'Records still physically present and lawful services independent of the seized original.',
  'Immediate control, availability and possibly confidentiality depending on actual inspection.',
  'Custody terms permit all required timely access without disclosure outside authorized boundaries.')
S(33, 'Authority transition complete for future work', 'ordinary organizational transition',
  'A recognized successor controls current access and permissions are validly rebound to actual people or roles.',
  'No attraction claimed; normal governance supports a new operating baseline.',
  'Sale, succession or separation reaches an accepted authority mapping and existing obligations are distinguished from new ones.',
  'A later change or dispute invalidates the mapping.',
  'Lawfully transferred assets and current legitimate operation.',
  'Former staff access, old automatic grants and any nontransferable context.',
  'Departed staff retain usable credentials or critical permissions have no recognized new holder.')
S(34, 'Separated project lacks permitted context', 'information and legal boundary hold',
  'A successor project may retain its own code but cannot lawfully use necessary company context.',
  'The information restriction sustains incomplete operation; copying prohibited context is not recovery.',
  'Spin-out or confidential customer partition removes formerly shared instructions or knowledge.',
  'Lawful sufficient context is independently reconstructed or licensed, or the dependent task is abandoned.',
  'Permitted project assets and clearly transferable knowledge.',
  'Dependent reasoning capability and old company-wide assumptions.',
  'The task is fully specified by already transferable project material.', 'Q C1')
S(35, 'Instrument continuity broken', 'measurement discontinuity',
  'Old and new agents or observation adapters report quantities with different or unknown semantics.',
  'No feedback necessary; averaging incompatible observations manufactures rather than restores continuity.',
  'Flotilla or harness replacement changes instruments mid-series.',
  'Validated common dimensions are isolated; a defensible mapping is established; or series remain separate.',
  'Original measurements with provenance and objective outcomes where independently comparable.',
  'Trend claims based on assumed identical blocked, waiting or turn semantics.',
  'Independent calibration establishes invariance for the exact compared quantities.', 'Q D3')
S(36, 'Offline work awaits world reconciliation', 'information-starved wait',
  'Remote work or restored state cannot establish current revocations and external versions while isolated.',
  'Disconnection sustains ignorance; local progress is possible but does not update external truth.',
  'Days offline or decades of dormancy separate local state from contemporary authority and dependencies.',
  'Fresh authenticated evidence reconciles outcomes, time, permissions and world state, or the work is invalidated.',
  'Readable local work and historical claims with explicitly limited freshness.',
  'Current authorization, global order and timely coordination assurances.',
  'All effects are self-contained under still-valid irrevocable authority and need no outside facts.', 'Q C3 D1')
S(37, 'Deployment requirement exceeds established capability', 'unresolved capability gap',
  'A demanded guarantee depends on unstated timing, availability, trust or lifecycle assumptions.',
  'No destination dynamics are inferred from a request alone.',
  'A single-host service is asked for strict real-time, root-resistant audit or unfamiliar execution constraints.',
  'A precise requirement and credible evidence establish feasibility, or the contract is narrowed or declined.',
  'Existing functions within their actual boundary.',
  'Justification for the stronger guarantee; unsupported does not mean every task fails.',
  'A demonstrated implementation meets a precise stated requirement over the relevant failure envelope.', 'Q D1 D2')
S(38, 'Irreversible physical or business effect', 'absorbing consequence',
  'An actual motion, destructive compensation or duplicate action changes a nonrecoverable part of the world.',
  'No feedback required. Later records cannot reverse the already realized consequence.',
  'An irreversible command is accepted before cancellation, or stale compensation overwrites nonrecoverable human work.',
  'No exit restoring the exact lost past; mitigation may produce a different acceptable future.',
  'Evidence and unaffected assets if retained.',
  'Pre-effect condition, which may include safety, property or unique human edits; harm severity is not stipulated.',
  'The effect never occurred or a demonstrated exact inverse restores everything at issue.', 'Q D1')
S(39, 'History readable but no longer intelligible', 'semantic information loss conditional on absent decoder',
  'Bytes survive but necessary business language, references or tacit knowledge do not.',
  'Repeated interpretation from the same context cannot create the missing meanings; not a storage attractor.',
  'Long dormancy outlives all interpreters of task shorthand and conventions.',
  'An independent glossary, contemporaneous example or knowledgeable witness supplies meaning; otherwise exact semantics remain lost.',
  'Original text, dates and mechanically decodable structure.',
  'Reliable business intent and ability to certify correct replay of decisions.',
  'Independent readers reconstruct the same validated meaning using surviving contextual material.')
S(40, 'Intermittent duty-cycle operation', 'externally driven cycle',
  'Energy is available only in bounded periodic windows and usable work can fit or checkpoint across them.',
  'The energy schedule forces on/off recurrence; no endogenous attraction is asserted.',
  'Five-minute monthly power availability replaces continuous operation.',
  'Power continuity changes or the service is retired; reducing work can improve yield but not remove the external cycle.',
  'Durable bytes and tasks whose boot plus useful work fit the window.',
  'Continuous communication and deadlines shorter than the off interval.',
  'Boot or recovery always exceeds the window, so there is no useful recurrent operation at all.')
S(41, 'Cryptographic trust basis withdrawn', 'epistemic and security hold',
  'Primitives used to justify identity, secrecy or signatures are practically broken and old proofs have no independent anchor.',
  'New signatures using the same broken basis cannot repair old authenticity; no attractor is implied.',
  'Practical breaks invalidate the assumptions under historical evidence or encrypted archives.',
  'Independent provenance and a newly trusted channel establish a limited fresh baseline; old secrecy cannot be retroactively restored.',
  'Physical custody evidence and plaintext whose integrity can be established independently, if any.',
  'Former cryptographic assurances; actual forgery or disclosure still requires a separate occurrence.',
  'An unaffected independent trust basis already proves the claims at issue.')
S(42, 'System information irrecoverably absent', 'absorbing loss within stated system boundary',
  'All relevant devices, copies, keys where indispensable, and independent memories or reconstruction sources are gone.',
  'No feedback and no latent surviving state. Reconstruction without information is not recovery.',
  'Universal erasure or loss of the last usable content and indispensable decoding knowledge.',
  'None within the premise; an independent surviving source would falsify total loss, not emerge from it.',
  'No recoverable system-specific information; only unrelated external capabilities if outside the premise.',
  'Identity, history, content, permissions and ability to recognize restoration of this system.',
  'Any independently usable source of the purportedly vanished information exists.')
S(43, 'Stale history mistaken for current permission', 'unsafe stale-state regime',
  'A stale snapshot or unrefreshed local authority state is treated as current, and later revocations or effects are missing from every check used at dispatch.',
  'Local checks keep validating the stale account; repeated restores or continued failed updates can repeat the mistake, but one stale check is a transient hazard.',
  'Old token or queued task becomes actionable after restore, or a role change or revocation never becomes effective in the state consulted by dispatch.',
  'Independent current authority and participant outcomes contradict and replace the stale dispatch premise.',
  'Historical state as of snapshot time and any unaffected local work.',
  'Fresh authorization and at-most-once effect claims across the snapshot boundary.',
  'A freshness or participant check outside the restored state rejects the stale credential or already executed operation.', 'Q C3 D1')
S(44, 'Compensation preconditions no longer hold', 'information-starved policy hold',
  'Human edits or changed plugin semantics invalidate the preconditions or meaning of a planned inverse.',
  'A conditional write refusing stale versions maintains the hold; failed compensation is not a completed rollback.',
  'A delayed compensation meets a newer object or an incompatible operation implementation.',
  'Current evidence and legitimate authority establish a new acceptable mitigation; exact inverse may no longer exist.',
  'Forward-operation evidence and current human data if refusal precedes overwrite.',
  'Automatic rollback guarantee and possibly the old compensation path.',
  'The inverse is version-bound, compatible and demonstrably preserves all intervening legitimate changes.', 'Q D1 D2')
S(45, 'Plugin startup dependency deadlock', 'conditional deadlock',
  'Every member of a dependency cycle requires another member fully ready before it can become ready; no lazy binding or seed exists.',
  'Mutual waiting is a fixed point of startup rules, not evidence of a basin-wide attractor.',
  'Dependency graph gains a strongly connected component of mandatory readiness prerequisites.',
  'One prerequisite can legitimately be satisfied independently or the dependency relation changes.',
  'Stopped plugin files and any independent local inspection path actually present.',
  'Startup of the cycle and all dependent functions.',
  'Dependencies are optional, lazily bound or initialized by an existing bootstrap that breaks the wait.')
S(46, 'Service retired with usable handoff', 'ordinary completion',
  'A complete permitted export is actually usable without Factory and shutdown leaves no required live Factory worker.',
  'No feedback; retirement is an intentional lifecycle endpoint.',
  'Customer accepts an independently readable handoff and all authorized service instances stop.',
  'Newly authorized adoption is a new lifecycle, not automatic continuation.',
  'Exported lawful content, interpretation aids and evidence that were actually included.',
  'Live Factory automation and nonexportable integrations; export does not prove erasure of every remote copy.',
  'The recipient needs a Factory daemon to read essential data or a residual service still performs work.')
S(47, 'Recovery duration exceeds service objective', 'externally constrained degraded recovery',
  'Correct reconstruction takes longer than the allowed recovery time, even though necessary information still exists.',
  'Growing history may worsen the gap; no positive feedback follows merely from a long finite replay.',
  'Decades of history, slow hardware or short power windows make recovery exceed the objective.',
  'A verified faster interpretation path or legitimately changed objective closes the gap; missed past deadlines stay missed.',
  'Recoverable canonical information and eventual correct reconstruction when energy and tools suffice.',
  'The claimed recovery-time objective and timely service.',
  'Representative full recovery including required content consistently finishes inside the objective.', 'Q D1 C5')

S(48, 'Essential content inaccessible with known recovery means', 'unresolved recovery capability',
  'Required data cannot be read with any currently known surviving copy, key and decoder combination, while traces of the system still exist.',
  'No feedback. Repeating restore against the same unreadable bytes does not supply a missing key or destroyed payload.',
  'Ransomware, correlated media destruction or device plus backup-key loss removes the last known practical content recovery path.',
  'A usable independent copy, key or decoder becomes available; proven destruction of every representation makes the particular content loss final, not necessarily all system information.',
  'Identifiers, manifests, ciphertext, remaining records or human knowledge where actually retained.',
  'Present ability to recover the essential payload and resume content-dependent work; permanent impossibility is not proved merely by a missing key.',
  'An independent reconstruction successfully recovers the required content without the missing means.')

trajectories = []
coverage = []
def T(n, initial, sequence, dest, conditions, falsifier, keys='Q', status='conditional', reason=None):
    sid = f'S{n}'
    destinations = 'none' if dest == 'none' else ' '.join(f'GV{int(x):02}' for x in dest.split())
    evidence = refs(keys) + '; long-term branch is conditional, not observed.'
    trajectories.append(dict(zip(
        'trajectory_id stressors initial_state sequence destination_states conditions evidence falsifier'.split(),
        [f'GVT{n-100:03}', sid, initial, sequence, destinations, conditions, evidence, falsifier])))
    coverage.append(dict(zip('stressor_id state_ids analysis_status reason'.split(),
                             [sid, destinations, status, reason or conditions])))

# Each scenario gets its own trajectory, including explicit unresolved dispositions.
T(101, 'Queued finite work requires a particular external model provider.',
  'Provider closes -> dependency absent -> GV04 if no authorized substitute. A substitute with independently accepted output permits GV01; an unmeasured substitution leaves GV07.',
  '4 1 7', 'Provider independence of local records is assumed; substitute availability and compatibility are not given.',
  'The task runs entirely on an already available local model without that provider.')
T(102, 'Comparisons treat a stable model alias as stable behavior.',
  'Alias changes weights -> old attribution silently persists -> GV07. If wrong outputs are accepted and reused as evidence, GV05; independent changed-output detection can lead to GV02.',
  '7 5 2', 'Alias is not immutable model identity; a feedback loop requires reuse of error, not the weight change alone.',
  'Provider supplies trustworthy immutable revisions for each call and output changes are independently validated.', 'Q D3')
T(103, 'An agent reads an external page as task data and has some tool capabilities.',
  'Injected instructions are followed -> if effective tool authority permits them, GV09 and possibly GV08. If the resulting claim enters shared evidence, GV05. Tool-side refusal can leave GV02 without an effect.',
  '9 8 5 2', 'Following text is stipulated, but privileged execution, disclosure and persistence depend on actual capabilities and destinations.',
  'The agent produces injected text but every unauthorized tool request is rejected and no false claim is accepted.')
T(104, 'Two nominally separate reviewers share a failure mode.',
  'Both approve the same false result -> a single false endorsement is a finite error; repeated citation of the endorsement closes GV05. A trusted independent result check instead sends the work to GV02.',
  '5 2', 'Reviewer count supplies no independence; persistent regime requires circular reuse or suppression of contradictory outcomes.',
  'Review errors are independent in the relevant failure family or externally checked results reverse the endorsement.', 'Q D3')
T(105, 'A running task can finish by submitting a summary or nonempty artifact paths.',
  'Model names nonexistent file -> complete_with_result can store the claim because it checks content and size, not existence. Later existence check leaves GV03; downstream reuse as proof can enter GV05. Actual verified output permits GV01.',
  '3 5 1', 'The library boundary is examined; an uninspected caller may add artifact verification. Existence is weaker than correctness.',
  'Every production completion caller independently verifies required artifact existence and content before acceptance.', 'Q C2', 'source-supported',
  'Source supports acceptance of unverified path strings in this function, not a deployed hallucination or persistent false-belief loop.')
T(106, 'Daemon and local model share finite host RAM.',
  'Model displaces daemon -> GV04 while inference retains resources. Restored ambiguous deliveries may enter GV13. Repeated restart-and-reload can sustain GV10 only while the resource policy repeats the overload.',
  '4 13 10', 'OS eviction behavior, memory limits and restart ordering determine duration; committed storage survival is not guaranteed by RAM pressure alone.',
  'Memory isolation reserves enough RAM for the daemon and it remains responsive throughout inference.', 'Q C3')
T(107, 'A run spans several inference calls and metadata is attributed per run.',
  'Undisclosed model switch -> result may remain useful but attribution becomes GV07. Shared new errors reused as evidence can enter GV05; the switch itself does not establish error.',
  '7 5', 'No per-call identity evidence is available. ADR 0020 explicitly leaves attribution across switches open.',
  'An independent per-call ledger identifies both models and attributes every relevant output and cost.', 'Q D3')
T(108, 'Task admits another correct intermediate step after every step.',
  'Locally correct progress -> another obligation -> GV12 only if no stopping condition intervenes. Finite budget or energy leads to GV21 or GV04; a bounded acceptance criterion permits GV01.',
  '12 21 4 1', 'An ideal endless sequence is distinct from any finite observed trace; a growing useful partial result may be economically rational.',
  'A finite well-founded obligation decreases every step and completion occurs before the resource bound.')
T(109, 'Compiled instruction chain previously fits the harness window.',
  'Harness shrinks window -> compiler still emits full text -> rejecting harness leaves GV02; silent truncation can create GV07 or, if errors are reused, GV05. A valid shortened task can reach GV01.',
  '2 7 5 1', 'Compiler reports bytes but sets no model-window bound; actual rejection or truncation behavior belongs to the uninspected harness.',
  'Compiled input still fits the new window, or harness rejects before any semantically incomplete execution.', 'Q C1')
T(110, 'An executable fixture stands in for broader competence.',
  'Model recognizes test -> specialized passing behavior -> GV06 if scores allocate trust and reduce independent tests. Held-out behavior can instead reveal the gap and trigger GV02.',
  '6 2', 'An executable oracle removes model grading, not test leakage or a wrong oracle. Fixture-specific optimization may be harmless if the claim is fixture-specific.',
  'Independent unseen tasks validate the same claimed capability and scores never replace external checks.', 'Q D3')
T(111, 'Task is queued before root instructions change; compilation time is unspecified.',
  'Root file changes -> compile reads bytes present when called, not queue-time bytes. Late compilation may apply the new mandate; cached compilation may retain old mandate. Detected inconsistency gives GV02; unrecorded binding gives GV07.',
  '2 7', 'Source establishes read-at-compile behavior but not when an eventual daemon invokes it or whether approval binds to a hash.',
  'Queue and launch are demonstrably bound to one immutable context and permission revision.', 'Q C1', 'source-supported',
  'Source-supported timing boundary only; actual queue-to-start integration and required policy revision remain unspecified.')
T(112, 'compile is asked to read an AGENTS source in the explicit chain.',
  'read_to_string fails -> UnreadableSource error and no successful compiled context -> GV03 at the compiler boundary. Restoring readability with valid meaning can allow GV01; callers that bypass compilation invalidate this branch.',
  '3 1', 'The unreadable source is actually included in scopes and callers propagate errors rather than reusing stale context.',
  'Compilation successfully returns while silently omitting an unreadable included source.', 'C1 Q', 'source-supported',
  'Explicit source error is implemented; propagation to a deployed scheduler and recovery duration were not observed.')
T(113, 'Human submits a task prompt containing a secret.',
  'Raw prompt reaches durable records or compile -> GV19 for retained copies. A provider or unauthorized reader actually receiving it produces GV08. Filtering before persistence could avert both, but instructions alone do not prove such filtering.',
  '19 8', 'Compiler includes supplied task text without a secret classifier; durable intake path and recipient authorization determine actual exposure.',
  'Intake removes the secret before any persistence or unauthorized transmission, with all copies accounted for.', 'Q C1 D1 D3')
T(114, 'Knowledge notes contain a cyclic link graph.',
  'Current compile reads only explicit context files and does not walk links -> link cycle adds no traversal, permitting GV01 for otherwise valid work. A separate agent crawler that follows links repeatedly can enter GV10 or GV12.',
  '1 10 12', 'The source-supported non-traversal applies only to this compiler, not arbitrary tools or a future knowledge plugin.',
  'compile itself follows the links, or the claimed task requires a different recursive crawler.', 'C1 Q', 'source-supported',
  'Non-traversal is source-supported. Any crawler branch is a separate explicitly hypothetical executor.')
T(115, 'Two currently available sources disagree on permission.',
  'No accepted precedence -> GV15 or GV02. Repeated majority citation without independent authority can enter GV05. A binding current authority rule can resolve the conflict and allow GV01.',
  '15 2 5 1', 'The sources may differ in jurisdiction or effective time rather than one simply being false.',
  'A recognized precedence and effective-date rule already yields one uncontested permission.')
T(116, 'A URL is used as the identity of evidence for earlier work.',
  'Content changes at unchanged URL -> re-fetch cannot prove old bytes -> GV07. Reusing old conclusions as current truth can enter GV05; retained timestamped content may permit GV01 on a properly scoped historical claim.',
  '7 5 1', 'Mutable address and immutable content identity are different. The scenario does not prove anyone relies on the changed page.',
  'The relevant version was retained with trusted time and provenance and current decisions explicitly select the correct revision.')
T(117, 'Harness auto-loads native configuration from a repository it will operate in.',
  'Malicious native config is interpreted -> if it can invoke commands or expand capabilities, GV09 and possibly GV08. Refusal before activation leaves GV02. Factory not writing harness config does not prevent harness reading hostile input.',
  '9 8 2', 'Native configuration semantics and harness trust settings are not inspected, so execution is a conditional branch.',
  'Native configuration is treated as inert text or rejected before any command or privilege change.', 'Q C1 D2')
T(118, 'Shared knowledge writes claim a scope identity.',
  'Agent submits another scope name -> trusted-binding check can reject to GV02. If store accepts the forged identity, GV28; reuse as authoritative knowledge can enter GV05 and cross-scope readers may cause GV08.',
  '2 28 5 8', 'Actual shared-memory authorization is not established by scope naming; a genuine write is not necessarily disclosure unless readers differ.',
  'Writer identity is independently bound to the session and every attempted foreign-scope write is rejected.', 'Q D1')
T(119, 'Artifacts with visually or canonically equivalent Unicode names coexist or alias.',
  'Different byte names meet filesystem or renderer normalization -> exact bytes may remain distinct, but ambiguous selection gives GV02 or GV07. If one overwrites the only copy, GV18. Independent identity validation permits GV01.',
  '2 7 18 1', 'Artifact store normalization, filesystem behavior and collision policy are missing; path identity of scope directories would not settle artifact identity.',
  'All consumers preserve and compare one unambiguous canonical artifact identity and refuse collisions.')
T(120, 'Production context includes sensitive material and telemetry has separate readers or retention.',
  'Full debug copy persists -> GV19; unauthorized telemetry readers actually access it -> GV08. Even authorized debug retention can conflict with later disposition and enter GV17.',
  '19 8 17', 'ADR 0020 forbids production full transcripts but is a design rule, not proof of redaction or safe telemetry.',
  'No sensitive context reaches telemetry and a verified copy inventory shows no retained full-context trace.', 'Q D3')
T(121, 'Plugin runs as the same OS user and can access that users credential facilities.',
  'Out-of-kernel keychain read succeeds as stipulated -> kernel audit cannot establish complete access mediation; unauthorized knowledge gives GV08 and reusable credentials may sustain GV09. Per-item OS access refusal would defeat the success premise.',
  '8 9', 'Same user is not a complete sandbox; keychain ACL, unlock status and recipient authorization still matter to the exact exposure.',
  'OS access controls reject the read or the item is inaccessible without a separate unavailable credential.', 'Q D2')
T(122, 'Manifest verification names a binary path before execution.',
  'Attacker swaps binary in the check/use interval -> executing replaced bytes enters GV09. Executing a verified immutable identity instead refuses the substitution and can leave GV02.',
  '9 2', 'Manifest validation is promised; binding the bytes checked to bytes executed is not established in inspected source.',
  'The launched image is proven identical to the verified object even when the path changes.', 'Q D2')
T(123, 'Kernel treats transport-supplied principal as trusted invocation context.',
  'Transport lies about authenticated actor -> kernel scope checks on false identity may accept unauthorized command -> GV09 and GV28. Independent binding outside the lying plugin can reject to GV02.',
  '9 28 2', 'Design assigns peer authentication to transport; domain validation does not independently prove which human spoke.',
  'Kernel verifies an independent unforgeable principal binding rather than trusting the compromised transports assertion.', 'Q D1 D2')
T(124, 'Agent has OS access to the mutable core database.',
  'Direct writes bypass domain guards -> SQL constraints may reject some writes, but accepted alterations create GV28. A legitimate writer continuing with conflicting schema or status assumptions may create GV27.',
  '28 27', 'Inspected schema is mutable rather than a tamper-proof event log; local connection access is not an OS isolation boundary.',
  'Independent OS isolation prevents any agent write, or every attempted alteration fails without changing the database.', 'Q C4 C5')
T(125, 'A new plugin parameter contains SQL or path-injection syntax.',
  'A string is not yet an exploit. A bound SQL value can remain data; string interpolation or path escape into a writable target can change state. No destination is assigned without the sink, privilege and canonicalization rules.',
  'none', 'Missing exact parameter consumer, SQL binding, path resolution, allowed root and attacker write rights.',
  'Inspecting the actual sink establishes either harmless data handling or a concrete reachable unauthorized write.', 'Q C5', 'unresolved',
  'Existing backup code escapes SQL quotes, but it is not the unnamed new plugin parameter. No general injection outcome can be inferred.')
T(126, 'Operator reads diagnostics through a terminal renderer.',
  'Escapes alter the display -> if operator trusts rendered text, GV28 and possibly GV02 on discovery. Raw stored bytes can survive, and an inert escaped renderer prevents the deception from governing action.',
  '28 2', 'Rendering and subsequent operator reliance, not merely storage of escapes, cause the diagnostic failure.',
  'Display renders control characters inertly and independent raw-byte inspection agrees with all operator conclusions.')
T(127, 'A stolen capability was revoked after the backup used for restore.',
  'Restore also restores the old trust state -> attacker reuses still-accepted token -> GV43 then GV09. Independent current revocation or expiry validation instead gives GV02.',
  '43 9 2', 'Token format, epochs, clock and revocation location are unspecified; a restore-local check cannot know a missing later revocation.',
  'Acceptance checks an independent current revocation authority and rejects the old token after restore.', 'Q C3 D1')
T(128, 'Installation resolves a familiar integration name through a dependency registry.',
  'Look-alike dependency is selected -> executing it with useful privilege gives GV09 and potentially GV08. Exact retained dependency identity can reject before activation to GV02.',
  '9 8 2', 'Name similarity alone is not execution; install scripts, selection and executable trust determine entry.',
  'Resolution remains pinned to the authentic immutable dependency and no code from the look-alike is run.', 'Q D2')
T(129, 'DB, backups and knowledge are writable in one ransomware blast radius.',
  'Encryption removes readability -> GV04 while recovery options are investigated, or GV31 if backups were assumed sufficient. With no known usable copy and key combination, GV48. Independently usable offline copy instead narrows recovery, potentially GV30.',
  '4 31 48 30', 'Encryption is not logically permanent loss merely because no key is currently known; paying or contacting attacker is not authorized or assumed.',
  'An unaffected independently restorable copy and its keys are demonstrated.')
T(130, 'Delegation to new scopes is legal and the scope set is large but finite.',
  'Long acyclic chains multiply admitted work -> GV10 at capacity. Finite roots and finite descendants can drain to GV01; GV11 needs a replenishing root or retry mechanism, not merely absence of cycles.',
  '10 1 11', 'rule::check prevents revisiting a chain scope, not aggregate fan-out. Fixed finite scopes bound chain depth but not the number of task instances.',
  'A measured aggregate admission bound keeps total work below service, or the finite tree drains after the burst.', 'Q C6')
T(131, 'Personal data is believed to be retained in an append-only history.',
  'Deletion request -> determine jurisdiction, applicable exceptions and which payloads are personal -> GV17 pending lawful disposition. Actual deletion of indispensable evidence with no lawful copy can create GV18; continued copies retain GV19.',
  '17 18 19', 'Append-only event history is a target design, not the inspected mutable implementation. Neither unconditional deletion nor unconditional retention is inferred as law.',
  'Applicable law and actual data layout already allow a compliant deletion with all required proof preserved.', 'Q C4 D1')
T(132, 'Retention is about to remove data newly subject to a court legal hold.',
  'If hold is recognized before deletion, GV17 under enforced preservation. If deletion wins and no lawful copy remains, GV18. Repeated contradictory jobs can remain a driven dispute, not self-healing convergence.',
  '17 18', 'Race ordering, service of the order and precise data scope decide the branch. Inspected backup library does not implement automatic retention.',
  'A binding hold is durably recognized before every relevant deletion path and the content remains retrievable.', 'Q C5')
T(133, 'Customer residency requirement excludes the available model provider.',
  'Enforced compliance check stops transmission -> GV20. Continuing sends may produce GV08 and GV17 if recipients or processing are unauthorized; compliant replacement with valid acceptance can yield GV01.',
  '20 8 17 1', 'Processing location, contractual exceptions and what data was transmitted are required; all cloud use is not assumed unlawful.',
  'The actual provider processing path demonstrably satisfies the customers binding residency terms.')
T(134, 'Original credentials were rotated but older backups retain secret bytes.',
  'Old snapshot remains -> GV19 even if credential authentication now fails. Actual unauthorized reading gives GV08; restore reactivating old credential validity could additionally enter GV43.',
  '19 8 43', 'Credential validity and information sensitivity differ; rotation need not erase personal data or prevent decryption with a retained key.',
  'Backups contain no sensitive historical material and no restored trust state can revalidate old credentials.', 'Q C5 D1')
T(135, 'A necessary librarys new release has unusable licensing terms.',
  'Existing lawful release may keep running -> GV25 for maintenance freeze. If continued business requires the new release, GV20 or GV21; lawful compatible substitution can allow GV01.',
  '25 20 21 1', 'The new license is not presumed to revoke rights already granted for older releases; redistribution and dependency necessity need legal review.',
  'Retained license rights and compatible supported versions permit the required use and rebuild.')
T(136, 'Audit requires the concrete human who authorized one effect.',
  'Actor field alone may identify only a shared role or transport assertion -> GV03 or GV28. Contemporaneous authenticated personal binding and authority evidence can support GV01 for this audit question.',
  '3 28 1', 'Design promises authenticated actors, not proof that a shared credential maps uniquely to one physical person or proves intent.',
  'An independently verifiable contemporaneous record binds the exact human, effect, permission scope and approval time.', 'Q D1 D2')
T(137, 'Original device is seized and inspection is demanded.',
  'Custody changes -> GV32; permitted independent copy may continue limited service. Compelled access raises GV17 for scope and lawful handling; actual unauthorized disclosure can produce GV08.',
  '32 17 8', 'Legal authority, access terms, encryption and existence of independent lawful copies determine availability and exposure; no evasion is assumed.',
  'The custody agreement permits timely full lawful operation with no unauthorized reader.')
T(138, 'A field formerly treated as ordinary is reclassified as sensitive.',
  'Previously distributed copies now matter -> GV19 and GV17 for disposition. Actual prohibited access under applicable rules can produce GV08; retroactive sensitivity does not prove a past legal breach.',
  '19 17 8', 'Need classification effective date, recipient inventory and retention purpose; current restrictions do not imply identical historic law.',
  'The field never left an authorized minimal retention boundary and its current treatment already complies.')
T(139, 'An access request requires lineage through scopes already deleted.',
  'If lawful lineage remains outside active scope rows, GV01 may answer the request. Missing indispensable lineage without independent source yields GV18; uncertain applicability or completeness leaves GV17 or GV03.',
  '1 18 17 3', 'Deleting a registry entry is not necessarily deleting provenance. Actual schema and retained content determine what can be answered.',
  'An independently verified lawful provenance export spans every deleted scope and answers the request.', 'Q D1')
T(140, 'Operator claims recovery because backup files exist.',
  'Insurer asks for demonstrated end-to-end recovery -> GV31 until representative proof exists. If tested recovery exceeds objective, GV47; a valid bounded demonstration can close the specific audit task as GV01.',
  '31 47 1', 'SQLite integrity and snapshot existence do not demonstrate content, keys, tools, authority or external-effect reconciliation. No drill was executed in this review.',
  'Independent representative recovery including all required dependencies has already met the claimed objective.', 'Q C3 C5 D1')
T(141, 'Previously profitable inference uses variable-price tokens.',
  'Hundredfold price change -> GV21 if enforced value or budget checks stop new work. Continuing under old estimates creates GV22 until liabilities are known, then possibly GV21. High-value tasks may still reach GV01.',
  '21 22 1', 'The multiplier alone does not imply insolvency; volume, preexisting commitments and enforceable caps determine loss.',
  'The task remains profitable at the new actual total cost within a binding budget.')
T(142, 'Prepaid balance previously bounded exposure.',
  'Provider switches to unknown postpaid terms -> local balance no longer caps liability -> GV22. Refusing new calls pending binding terms gives GV21; local cancellation does not cancel already incurred debt.',
  '22 21', 'Must know contract effective time, provider-side cap and pending usage to bound actual liability.',
  'Binding contractual limits and complete metering still cap all accepted requests at a known amount.')
T(143, 'Legal tasks from a large scope compete for finite slots.',
  'A finite workload creates GV10 and may drain. If occupancy earns future allocation and voice, capacity advantage reproduces as GV23. Independent fair allocation can permit other work to reach GV01.',
  '10 23 1', 'Max sessions per agent is not a demonstrated cross-scope fairness rule. Feedback requires endogenous allocation advantage.',
  'Previously starved scopes receive their guaranteed share even while the large scope keeps submitting work.')
T(144, 'Tasks save less effort than their individual human approvals cost.',
  'Measured net loss -> rational nonuse enters GV21. If teams are rewarded for throughput instead of net value, they bypass approvals -> GV16. Legitimately eliminating the task is not a system failure.',
  '21 16', 'Human attention and opportunity cost count as service cost; batching authority is not presumed valid for every effect.',
  'Measured end-to-end benefit including review and rework exceeds human and machine cost without bypass.')
T(145, 'Arrival rate increases tenfold but workspaces remain fixed.',
  'If spare capacity absorbs it, GV01 remains possible. Otherwise continuing arrivals sustain GV10; deadlines expire into GV24. Queue-driven retries can additionally create GV11.',
  '1 10 24 11', 'Baseline utilization, task service times, arrival duration and retry behavior are unspecified; tenfold does not itself prove overload.',
  'Effective service still exceeds admitted load and all finite deadline-bearing work completes on time.')
T(146, 'A long-running agent already has work and possibly in-flight provider calls.',
  'Budget becomes zero -> an effective spend gate yields GV21 for future spend. Cooperative cancellation may leave work running and liability accumulating as GV22; uncertain delivery can need GV13.',
  '21 22 13', 'Recorded cancellation is not forcible termination and cannot recall accepted external calls. Budget enforcement was not established in inspected code.',
  'Provider and runtime acknowledge a binding stop before any post-cutoff charge or new effect.', 'Q C2 C3')
T(147, 'Customer demands guaranteed real-time answers on a local single host.',
  'Requirement lacks failure envelope -> GV37. If guarantee includes host outage, a host loss yields GV04 and a missed strict deadline GV24. A narrow bounded offline task might satisfy a separately specified contract.',
  '37 4 24', 'Need worst-case computation, external inference latency and whether outage intervals count; local presence alone gives no hard bound.',
  'Measured worst-case behavior and the agreed outage model prove all required deadlines can be met.')
T(148, 'A specialized task has a real expiring market opportunity.',
  'Queue wait crosses the value deadline -> GV24 even if later technically successful. Continuing work may deepen economic loss into GV21; revaluing a changed opportunity constitutes a new task.',
  '24 21', 'Loss concerns the original business objective, not necessarily the truth or reusability of late output.',
  'Customer accepts late work with unchanged economic value.')
T(149, 'Primary service works but external backup fees are no longer affordable.',
  'Backup service stops -> GV30 while primary and remaining copies work. If no feasible recovery objective remains, GV21 or GV31; data is not yet lost merely because subscription ends.',
  '30 21 31', 'Provider deletion timing, retained exports and alternative custody determine recovery diversity; no cancellation was performed.',
  'Equivalent independent usable backups remain available without the unaffordable service.')
T(150, 'Metrics compare runs using incomplete knowledge of final provider charges.',
  'Unknown invoice makes claimed cost efficiency unsupported -> GV22 and GV07. If scores select more of the falsely cheap path, GV06 can lock in biased comparisons; settled invoices may refute the ranking.',
  '22 7 6', 'Token counts are not universal prices and objective outcome comparison may remain valid while monetary comparison fails.',
  'Complete invoicing with compatible scope and time attribution preserves the claimed cost ranking.', 'Q D3')
T(151, 'Same source is rebuilt with a Rust or platform behavior change.',
  'Changed runtime semantics -> detected incompatibility leaves GV26. Undetected path or identity changes can create GV28; a retained compatible build gives GV25 while new behavior is investigated, or GV01 after validation.',
  '26 28 25 1', 'Concrete changed behavior and test oracle are unspecified; unchanged code does not imply unchanged executable environment.',
  'Representative platform contract tests show invariant behavior for every dependency actually exercised.')
T(152, 'Target event-sourced release has historic event payloads needing interpretation.',
  'Release omits necessary upcaster -> fail-at-cursor design would produce GV26. Silently skipping old payloads instead creates GV28. No current event-replay implementation is established by the inspected SQL migrations.',
  '26 28', 'An upcaster is necessary only when required historical payloads are incompatible; additive backward-compatible events may need none.',
  'All existing payload versions are demonstrably accepted with identical meaning by the new reader.', 'Q C4 D1')
T(153, 'Python and Rust processes point at the same table names with incompatible schema expectations.',
  'One rejects incompatible schema -> GV26. Both keep accepted mutations -> GV27 and potentially GV28. Independent databases do not enter this shared-table scenario.',
  '26 27 28', 'SQL transactions serialize bytes within one database, not incompatible semantic interpretations or effects outside it.',
  'Deployment ensures nonoverlapping stores or only one compatible writer can mutate the table.', 'Q C4 C5')
T(154, 'A newer binary has already migrated stored schema before binary rollback.',
  'Old reader meets new schema -> refusal gives GV26. If it accepts but misinterprets data, GV28. Restoring an old DB instead can enter GV43 because post-snapshot real effects remain.',
  '26 28 43', 'Forward-only migration list does not prove historical binary behavior. The old release and data shapes must be tested offline.',
  'The exact old binary is read/write compatible with migrated state, or a verified reversible migration preserves all current evidence.', 'Q C4 C3')
T(155, 'Required package version disappears from its download location after maintainer transition.',
  'Existing installation may work as GV25 while rebuild is impossible. An authentic retained compatible package can permit GV01; absence of every usable runtime gives GV04.',
  '25 1 4', 'Download availability, installed runtime, source buildability and legal redistribution rights are separate variables.',
  'An independently authentic retained package and full dependency chain can rebuild on replacement hardware.')
T(156, 'A publication workflow should export only project history.',
  'Wrong company-root export before transmission can be halted at GV02. If actually published to unauthorized recipients, GV08 regardless of later deletion; retained unauthorized release copies remain GV19.',
  '2 8 19', 'README documents the subtree boundary but a documented command is not proof of every publisher invoking it correctly. No publication occurred here.',
  'The published object graph contains only authorized project content and no recipient obtained company-root data.', 'Q R1')
T(157, 'Tests deliberately bless an unsafe fallback when identity is missing.',
  'Green tests reward the fallback -> if taken as safety proof, GV06; executing under invented identity can produce GV28 or GV09. A separate identity contract can reject the behavior and lead to GV02.',
  '6 28 9 2', 'The scenario stipulates a wrong oracle; no particular current missing-ID implementation is accused without inspection.',
  'Independent identity requirements reject the fallback despite the test suite remaining green.')
T(158, 'README and ADR describe behavior inconsistent with current library schema.',
  'Operator follows stale assumptions -> GV07 for uncertain contract or GV26 when commands reject mismatched data. If documentation is repeatedly cited over contrary source, GV05. Explicit version-specific interpretation can permit GV01.',
  '7 26 5 1', 'Observed example: README says later slices absent, while task/delegation/recovery crates and schema migrations 1-5 exist. This supports documentation lag, not every alleged schema contradiction.',
  'Documentation tied to the deployed version matches executable schema and operators use that versioned contract.', 'Q R1 C4', 'source-supported',
  'Static README/source mismatch was observed. Operational impact and stable belief dynamics remain conditional.')
T(159, 'Updater starts plugin before checking manifest and executable identity.',
  'Activation-before-verification executes untrusted bytes -> GV09 if malicious, potentially GV08. Later failed verification does not undo effects. If bytes are benign and compatible, GV01 remains possible without proving the update path safe.',
  '9 8 1', 'This violates the design ordering but is not an observed updater implementation in the inspected libraries.',
  'No plugin code or install hook runs until actual executable identity and manifest checks succeed.', 'Q D2')
T(160, 'Architecture team solves each coordination problem with another kernel service.',
  'New interfaces create new integration problems -> further service creation -> GV29 if incentive and decision pattern persist. If additions consolidate and reduce total coupling, bounded successful change can reach GV01.',
  '29 1', 'Service count alone is not coupling or a demonstrated feedback loop; ownership and removal decisions determine persistence.',
  'Longitudinal decisions show incidents decrease total coupling and allow services to be removed.')
T(161, 'Only the founder is recognized as approval authority for necessary effects.',
  'Permanent incapacity -> GV14 even if all credentials and data remain. Recognized succession can produce GV33; informal impersonation instead enters GV16 or GV28.',
  '14 33 16 28', 'Ability to authenticate is distinct from legal or organizational authority; succession provisions are unspecified.',
  'A valid competent alternate already has the necessary authority and can exercise it.')
T(162, 'Two executives issue incompatible directions for the same effect.',
  'Without binding precedence -> GV15. Implementing both independently may yield GV27 or GV38 for irreversible duplicate effects. An accepted authority determination can permit GV33 for future work.',
  '15 27 38 33', 'Legitimate titles do not settle quorum, delegation, timing or scope; last instruction arrival is not automatically valid precedence.',
  'A mutually recognized governance rule chooses one instruction before any conflicting execution.')
T(163, 'Company sale removes former staff rights while in-flight work and old sessions exist.',
  'Current credentials and role authority are actually rebound -> GV33. Unreconciled old grants can create GV43 and GV09; unresolved ownership of specific effects leaves GV15.',
  '33 43 9 15', 'Rights changing on paper and revocation reaching every session or restore are different events; historic authorship should not be relabeled.',
  'All old credentials and delegated capabilities are unusable and each pending effect has valid current authority.')
T(164, 'Project separates legally and cannot take parent company context.',
  'Old root-to-leaf context is no longer permitted -> GV34. Blindly preserving it can create GV08 and GV17. Independently sufficient lawful project context permits GV01.',
  '34 8 17 1', 'Compiler concatenation establishes what it reads, not the legal transferability of those bytes. Not all company instructions are necessarily needed.',
  'All required inputs are already permitted standalone project material and no prohibited context is included.', 'Q C1')
T(165, 'Security owner and product owner disagree on resuming uncertain risky work.',
  'Risk decision has no accepted precedence -> GV15 and GV02; missing external outcome can separately sustain GV13. Bypass under delivery pressure may normalize into GV16 rather than resolving the risk.',
  '15 2 13 16', 'Both authority over risk and knowledge of prior effects are necessary; either can remain absent after the other is resolved.',
  'A recognized risk authority resolves the conflict and independent participant evidence settles the old operation.', 'Q C3 D1')
T(166, 'Service supplier replaces agents and observation instruments wholesale.',
  'Old/new friction fields no longer have common semantics -> GV35 and GV07. Independently verified outcome dimensions may still support GV01; same-model regrading can instead enter GV06.',
  '35 7 1 6', 'Agent replacement does not necessarily break output contracts; incomparable observation fields are not zero-valued measurements.',
  'Calibration demonstrates the compared quantities are invariant across the exact old and new instruments.', 'Q D3')
T(167, 'Teams are evaluated on minimizing blocked runs.',
  'Unblocking by avoiding permission increases scores -> rewards and normalization -> GV16 and GV06. Independently penalized unauthorized effects can reverse this rather than the metric determining destiny.',
  '16 6', 'Need actual ability to bypass and reward asymmetry. Legitimately eliminating unnecessary blocks is a competing beneficial interpretation.',
  'Lower blocked counts reflect only valid simplification, and independently audited approval compliance remains unchanged.')
T(168, 'Shared machine is overloaded and a scope owner refuses budget disclosure.',
  'Capacity shortage sustains GV10; if nondisclosure also prevents binding cost attribution, GV22. If occupancy buys political influence that preserves opaque allocation, GV23; accepted independent accounting can break the feedback.',
  '10 22 23', 'Privacy or contractual confidentiality may justify nondisclosure; neither resource use nor fair entitlement can be inferred from budget secrecy alone.',
  'Independent admissible resource accounting permits accepted allocation without exposing the owners confidential budget.')
T(169, 'Organization wants an uncomfortable audit fact rewritten.',
  'Refusal preserves the recorded fact but can leave GV15 over authority. Actual undetectable rewrite enters GV28; if favorable rewritten history justifies more rewriting, GV16 or GV05 may follow.',
  '15 28 16 5', 'Design append-only promises and mutable local implementation differ. A correction appended with provenance is not equivalent to concealed rewriting.',
  'Original fact remains independently provable and any correction is visibly attributable without destroying it.', 'Q C4 D1')
T(170, 'Partner shuts down while older operation outcomes require confirmation.',
  'No partner witness -> GV03 and possibly GV13 for potentially delivered work. Retained independent receipts can permit GV01. Blind reissue may cause GV38 if the first action actually happened.',
  '3 13 1 38', 'Partner absence removes one witness, not necessarily all evidence; timeouts do not establish failure.',
  'Independent signed receipts or actual world evidence conclusively settle each prior operation.', 'Q C3 D1')
T(171, 'Each of one thousand newly activated scopes starts a plugin process.',
  'Aggregate resident cost exceeds host limits -> GV10 or GV04 for indispensable components. If process resources remain within headroom, GV01 is possible; an endless spawn/restart loop requires an additional restart policy.',
  '10 4 1', 'Per-scope explicit activation bounds dependencies semantically, not aggregate resource use. Actual process footprint and sharing are missing.',
  'Measured concurrent footprint and startup demand fit host limits with required daemon headroom.', 'Q D2')
T(172, 'Two mutually confidential customers run under one effective OS user.',
  'Application scopes alone cannot prove isolation -> GV37. If shared-user access actually reads the other customers data, GV08; declining co-location under the contract leaves GV20.',
  '37 8 20', 'Same UID is a trust-boundary warning, not proof of a particular leak. OS sandbox and key access restrictions could provide additional boundaries.',
  'An independently demonstrated isolation boundary prevents both customers agents from accessing each others data under the stated threat model.', 'Q D2 C5')
T(173, 'Remote workers on foreign hardware remain offline for days.',
  'Local work advances without current revocation or object versions -> GV36. On reconnect, obsolete grants can become GV43 or incompatible effects GV27. If work stayed inert and self-contained, verified acceptance may yield GV01.',
  '36 43 27 1', 'Need what work may execute offline, who controls hardware, and whether credentials or sensitive data can be inspected by that owner.',
  'Offline effects require no changeable external authority and reconnect verifies every current precondition before any new effect.', 'Q D1')
T(174, 'Product is required to work on a smartphone without a continuously running daemon.',
  'No operational meaning of work is specified: remote client, intermittent local inspection and autonomous background mutation impose different contracts. A durable regime cannot be selected from the form-factor demand alone.',
  'none', 'Missing offline functionality, OS suspension rules, permissible remote daemon, latency and background-effect requirements.',
  'A precise acceptance contract and lifecycle trace establish which operations must survive suspension.', 'Q', 'unresolved')
T(175, 'Company requests active-active automatic failover between cities.',
  'Demand alone does not choose consistency or partition policy. Independent writers could diverge; fenced ownership could suspend one city. Neither branch is assigned as the actual destination without a failure and contract model.',
  'none', 'Missing partition behavior, permissible unavailability, global effect ownership, quorum placement, latency bounds and authority to fail over.',
  'A specified partition trace with fencing and external participant semantics determines a unique accepted execution history.', 'Q D1', 'unresolved')
T(176, 'Robot plugin must execute an irreversible motion within milliseconds.',
  'Requirement exceeds established kernel timing claim -> GV37. Refusal to act under unverified safety assumptions gives GV20. Once motion actually occurs, GV38 even if authorized and harmless; severity requires physical dynamics.',
  '37 20 38', 'No control-loop period, plant state, safety interlock or worst-case delay is supplied. Approval before motion is not a real-time safety proof.',
  'A separately validated controller and safety boundary meet the exact deadline and physical hazard envelope.', 'Q D1')
T(177, 'Regulated evidence must resist a hostile root administrator.',
  'Local append-only policy or file permission cannot prove this threat boundary -> GV37. If root alters records without independent anchor, GV28; independent custody evidence may settle particular facts as GV01.',
  '37 28 1', 'Root can control local files and code under the stated model; whether an independent trust anchor exists is unspecified.',
  'An external independently controlled evidence chain demonstrates detection of all relevant root-admin tampering.', 'Q C4 C5 D1 D2')
T(178, 'Decades of required events are still available but full replay exceeds RTO.',
  'Correct finite replay finishes too late -> GV47 and possibly GV24. A validated snapshot plus complete tail might meet the objective; unreadable historic payload instead gives GV26, which is a different mechanism.',
  '47 24 26', 'Event replay is target design. Snapshot existence alone does not show bounded validation, tail processing or content retrieval time.',
  'Representative complete reconstruction with required content and verification meets the RTO on replacement hardware.', 'Q D1 C4')
T(179, 'Customer requests complete export and shutdown of every Factory service.',
  'Independent recipient can read all required lawful material and all instances cease -> GV46. Missing content or nonportable meaning yields GV31 or GV34. Retained remote copies can still leave GV19; stopping services is not universal erasure.',
  '46 31 34 19', 'Completeness includes interpretation, scope lineage and required artifacts, not just a SQL dump; export and legal deletion are separate requests.',
  'Recipient needs a live Factory service or discovers an essential missing object after claimed shutdown.', 'Q D1')
T(180, 'Plugin startup graph contains a cycle.',
  'If each edge is a strict fully-ready prerequisite, no member starts -> GV45. Lazy or optional binding breaks the wait and allows GV01. Startup retries can additionally produce GV10 if they consume resources without progress.',
  '45 1 10', 'A graph cycle alone is insufficient for deadlock; readiness semantics and independent bootstrap determine entry.',
  'One cycle member reaches readiness without another and every remaining prerequisite then becomes satisfiable.', 'Q D2')
T(181, 'A solar storm destroys electronics in several backup regions.',
  'Loss of affected service -> GV04. Unaffected independent recoverable media leave GV30; if destruction covers every known usable content source, GV48. Several regions is not stipulated to mean every trace and memory.',
  '4 30 48', 'Need geography, media exposure, keys and replacement tool availability. Correlation weakens the inference from copy count to independence.',
  'A geographically and physically unaffected copy with accessible keys is actually recoverable.')
T(182, 'A Mac wakes in 2046 with historical schedules, certificates and providers.',
  'Old local state cannot establish current authority -> GV36. Obsolete provider/toolchain gives GV25 or GV04; indiscriminate catch-up creates GV10 or stale effects GV43. Historical schedule entries are not fresh authorization.',
  '36 25 4 10 43', 'Clock correctness, missed-occurrence policy and surviving legal entity are unknown; normal chronological processing need not be meaningful after decades.',
  'All dependencies, current permissions and schedule intent are independently revalidated before any expired work executes.', 'Q C3 D1')
T(183, 'Task text survives twenty years but its business abbreviations no longer have interpreters.',
  'Bytes remain readable while intent becomes GV39. A surviving independent glossary can recover meaning and permit GV01; plausible model expansion without validation may enter GV05.',
  '39 1 5', 'Natural-language fluency is not proof of original business meaning; missing tacit context may be unrecoverable even with intact encodings.',
  'Independent contemporaneous records let multiple readers recover and validate the same business intent.')
T(184, 'All available model and human reviewers share one convincing false belief.',
  'Mutual agreement reuses the same error -> GV05 if belief steers evidence selection. Independent physical or logical observation may still refute it; without any accepted independent criterion, justified correction remains GV03 rather than automatic convergence.',
  '5 3', 'Universal reviewer agreement is stipulated, not universal failure of all experiments. No meta-reviewer outside the premise is invented.',
  'A non-reviewer-dependent observation contradicts the claim and the review community accepts it as authoritative.')
T(185, 'Technically identical clones receive contradictory but legitimate world states.',
  'Identical internal state cannot identify which external statement governs. Both may be valid in different jurisdictions or times; a common authority might instead make them conflicting branches. No common destination is asserted.',
  'none', 'Missing meaning of legitimate, common object identity, effective time, jurisdiction and whether clones must converge at all.',
  'An accepted namespace and authority model determines whether the two states are independent truths or a resolvable conflict.', 'Q', 'unresolved')
T(186, 'New law is said to require immediate deletion of all business data and eternal provability.',
  'If proof requires retaining the very data that must cease to exist, simultaneous full satisfaction is impossible. If permitted nonpersonal proof or legal exceptions exist, the requirements differ. No legally justified state is invented.',
  'none', 'Missing jurisdiction, precise data and proof definitions, exception rules and authoritative precedence; literal mutually exclusive obligations have no joint technical solution.',
  'A competent binding interpretation permits a proof representation that does not retain any prohibited data.', 'Q D1', 'unresolved')
T(187, 'Energy will be available only five minutes each month.',
  'If boot plus checkpointable useful work fits, GV40. If recovery alone exceeds each window without persistent progress, GV04 or GV47. Continuous-service deadlines necessarily yield GV24 under that duty cycle.',
  '40 4 47 24', 'Energy availability does not specify energy quantity, startup time, durable checkpoints or useful task granularity.',
  'Measured boot and recovery consume every window, refuting useful intermittent operation, or independent energy removes the cycle.')
T(188, 'Every cryptographic primitive currently used is practically broken.',
  'Old assurance basis is withdrawn -> GV41. Actual decryption of retained copies yields GV08; actual forgery yields GV28. A future new primitive cannot prove that historical ciphertext was never read.',
  '41 8 28', 'Practical breaks undermine capability claims but do not themselves prove an attacker used them; independent physical provenance may survive.',
  'A trust source not dependent on the broken primitives independently proves the particular historical claim and controls new access.')
T(189, 'Two legal successors each claim all old approval rights.',
  'Shared inherited credentials cannot resolve the legal claim -> GV15. Valid recognized partition or adjudication can yield GV33; both executing disputed effects may create GV27 or GV38.',
  '15 33 27 38', 'Legal succession and credential possession are separate. No arrival-order or majority rule is presumed to settle ownership.',
  'Binding accepted succession instruments assign each concrete permission unambiguously before execution.')
T(190, 'Every device, backup, key and memory of the system vanishes.',
  'The premise removes every source of system-specific information -> GV42. A later unrelated new system is not recovery and cannot verify identity with the vanished one.',
  '42', 'Total loss is conditional on the stated complete information boundary, including no external reconstructive trace. This is logical loss, not an empirical attractor.',
  'Any independently usable trace or memory can reconstruct a system-specific fact.')
T(191, 'Old backup predates a revocation and a queued action that was subsequently executed.',
  'Restore sees queued-with-no-attempt in its own past -> source reconcile leaves it queued, enabling GV43 if trusted as fresh. A snapshot that contains an attempt yields GV13 instead. Reissuing a real nonidempotent effect can cause GV38.',
  '43 13 38', 'Snapshot-local absence of an attempt is not global proof of never executed. External revocation and participant outcome checks are not provided by reconcile.',
  'Independent current revocation or participant ledger refuses execution before the stale queued action is sent.', 'Q C3 D1', 'source-supported',
  'Source directly supports snapshot-local queued/no-attempt behavior; actual later dispatch and duplicate external effects remain conditional.')
T(192, 'Plugin-generated data fills SSD while an urgent revocation arrives.',
  'Full storage causes GV10 and may prevent durable revocation. Acknowledged-but-uncommitted revocation can leave GV43; already accepted irreversible work can create GV38. Effective stop independent of new durable writes could preserve GV02.',
  '10 43 38 2', 'Need write-failure handling, acknowledgement semantics, current in-flight effects and whether stopping needs disk. Flood plus timing alone does not prove the revocation was lost.',
  'Revocation becomes effective before any affected action despite the storage exhaustion, and acknowledgements match durable state.')
T(193, 'Herdr becomes unavailable, leases remain, sole operator dies, and compute budget runs out.',
  'Unobserved sessions and uncertain work -> GV13; no recognized resolver -> GV14. Budget exhaustion yields GV21 or GV04 rather than freeing legitimate authority. A successor plus independent outcome evidence is needed to exit both holds.',
  '13 14 21 4', 'Restoring runtime alone does not resolve authority or budget. Releasing leases without evidence could enable conflicts rather than recovery.',
  'A recognized alternate can obtain authoritative process outcomes and fund or terminate the work without relying on the deceased operator.', 'Q C3')
T(194, 'Restore manifest lists content but the only content copy must be deleted by court order.',
  'Pending legal interpretation or timing -> GV17 and GV31. Once lawful deletion removes indispensable sole content, GV18. A hash-bearing manifest cannot reconstruct it; claiming full replay would create GV28.',
  '17 31 18 28', 'Need whether content is actually required by reducers and whether the order allows a lawful retained representation. No unauthorized preservation is assumed.',
  'Required replay meaning is recoverable from a lawful independent representation not covered by deletion.', 'Q D1 C5')
T(195, 'Provider migration changes output format and same model family checks its own error.',
  'Mechanical consumer may reject format -> GV02. Shared verifier accepts wrong semantics -> GV05 if reused and GV06 if evaluation selects the new provider on misleading scores; lost attribution gives GV07.',
  '2 5 6 7', 'Syntactic acceptance, semantic correctness and independent verification are separate. A format change alone need not produce wrong business meaning.',
  'Independent schema and semantic gates reject the exact migration error even while the same-model reviewer endorses it.', 'Q D3')
T(196, 'Two scope agents generate tasks while cron imports months of missed work.',
  'Finite catch-up wave plus legal branching -> GV10. Same lineage cannot revisit a scope under rule::check; GV11 requires fresh roots, repeated schedules or retry-generated work beyond that bound. Finite bounded admissions can drain to GV01; obsolete jobs give GV24.',
  '10 11 1 24', 'Cron catch-up policy is not inspected or assumed implemented. Two-agent same-chain ping-pong is a counterexample excluded by current delegation gate, not proof of bounded total task count.',
  'Every root and descendant is finite, retries preserve one identity, and completions strictly shrink the remaining work after catch-up ends.', 'Q C6')
T(197, 'A compensation meets a human-modified object while its plugin is upgraded.',
  'Version-bound inverse refuses changed precondition -> GV44. If new implementation ignores version or changes compensation meaning, an overwrite can yield GV38 and GV28. Accepted new mitigation is not proof of original rollback.',
  '44 38 28', 'Forward object version, human edit, retained compensation schema and actual executed plugin identity must all be known; design promises are not verified runtime behavior.',
  'A version-pinned conditional inverse preserves the intervening human edit and uses the exact originally validated compensation semantics.', 'Q D1 D2')
T(198, 'Migration cutover stops midway and a backup clone starts legacy dispatch concurrently.',
  'Same-store incompatible reader may refuse into GV26; independent clone can bypass the local lock and become GV27. Stale clone queue enters GV43; duplicate irreversible effect yields GV38 even if each local DB is internally consistent.',
  '26 27 43 38', 'Database transaction atomicity does not fence an independent host or participant. Need writer ownership and participant identities across cutover.',
  'Legacy dispatcher is independently fenced before dispatch and each participant rejects duplicate operation identities.', 'Q C3 C4 D1')
T(199, 'Primary device is lost, backup key is lost and vendor is insolvent.',
  'Backup bytes alone cannot supply key or runnable tools -> GV31 and GV25 during assessment. If no known usable plaintext, key or decoder remains, GV48; a retained lawful complete toolchain with another usable copy can restore the missing capability. Ciphertext and memories are not total absence.',
  '31 25 48', 'Vendor insolvency alone does not make open or retained software unusable. Lost indispensable encryption key differs from merely unavailable downloads.',
  'A demonstrably usable independent copy and lawful decoder/toolchain exist without the lost key or vendor.')
T(200, 'Incident report equates every marked matrix cell with full resilience and stops real tests.',
  'Unsupported coverage claim -> GV31. Stopping tests removes adverse evidence; favorable paperwork is reused to justify no tests -> GV06 and potentially GV05. Reinstated independent tests could instead expose gaps and lead to GV02.',
  '31 6 5 2', 'Feedback requires report authority to actually govern testing, as stipulated. This reviews own complete coverage is a disposition ledger, never empirical success evidence.',
  'Independent tests continue to govern resilience claims, and a marked matrix cannot suppress contrary measured outcomes.')

if __name__ == '__main__':
    expected = [f'S{i}' for i in range(101, 201)]
    with (ROOT / 'scenarios.csv').open(encoding='utf-8', newline='') as f:
        inputs = list(csv.DictReader(f, delimiter=';'))
    assert [x['stressor_id'] for x in inputs] == expected
    assert [x['stressor_id'] for x in coverage] == expected
    assert len({x['trajectory_id'] for x in trajectories}) == 100
    state_ids = {x['state_id'] for x in states}
    assert len(state_ids) == len(states)
    assert state_ids == {f'GV{i:02}' for i in range(1, len(states) + 1)}
    schemas = {
        'states.csv': 'state_id name kind conditions feedback entry exit survives lost falsifier evidence source_refs'.split(),
        'trajectories.csv': 'trajectory_id stressors initial_state sequence destination_states conditions evidence falsifier'.split(),
        'coverage.csv': 'stressor_id state_ids analysis_status reason'.split(),
    }
    used = set()
    for trajectory, row in zip(trajectories, coverage):
        assert trajectory['stressors'] == row['stressor_id']
        assert trajectory['destination_states'] == row['state_ids']
        mentioned = set(re.findall(r'\bGV\d+\b', trajectory['sequence']))
        if row['state_ids'] == 'none':
            assert row['analysis_status'] == 'unresolved'
            assert not mentioned
        else:
            refs_ = set(row['state_ids'].split())
            assert refs_ <= state_ids
            assert mentioned <= refs_, (row['stressor_id'], mentioned - refs_)
            used |= refs_
        assert row['analysis_status'] in {'conditional', 'source-supported', 'toy-supported', 'unresolved'}
    assert used == state_ids, f'Unused states: {state_ids - used}'
    for name, rows in [('states.csv', states), ('trajectories.csv', trajectories), ('coverage.csv', coverage)]:
        assert all(list(row) == schemas[name] for row in rows)
        assert all(all(isinstance(v, str) and v for v in row.values()) for row in rows)
        with (ROOT / name).open('w', encoding='utf-8', newline='') as f:
            writer = csv.DictWriter(f, fieldnames=list(rows[0]), delimiter=';', lineterminator='\n')
            writer.writeheader()
            writer.writerows(rows)
        with (ROOT / name).open(encoding='utf-8', newline='') as f:
            assert list(csv.DictReader(f, delimiter=';')) == rows
    from collections import Counter
    summary = f'{len(states)} states; {len(trajectories)} trajectories; {len(coverage)} coverage rows\n'
    summary += 'Statuses: ' + str(dict(Counter(x['analysis_status'] for x in coverage))) + '\n'
    summary += 'Unresolved: ' + ' '.join(x['stressor_id'] for x in coverage if x['analysis_status'] == 'unresolved') + '\n'
    summary += 'Validation: exact input IDs, unique state/trajectory IDs, exact column schemas, nonempty fields, valid state references, sequence-reference inclusion, matching coverage/destinations, all states used, and UTF-8 semicolon CSV round-trip passed.\n'
    summary += 'This is structural validation of the review files, not a runtime test or evidence of resilience.\n'
    (ROOT / 'validation.txt').write_text(summary, encoding='utf-8')
    print(summary, end='')
