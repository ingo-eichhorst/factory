<!-- Generated exclusively by docs/residuality/a5/derive.py. -->
# Provisional structures derived after state analysis

**These are design candidates, not empirically established Factory residues.**
A linked case isolates a mechanism and can enter several regimes from different initial states.
The interpretation below is authored; the dynamics and basin search do not read these labels.

| Candidate | Regimes | Structure / retained function | Condition | Architecture consequence |
|---|---|---|---|---|
| N01 | G01 G02 | Admission-bounded useful-work queue: Some accepted work can finish while excess demand is explicitly refused | One effective intake authority and service capacity not consumed by an uncounted path | Account for accepted and rejected demand without pretending all offered work is served |
| N02 | G03 G04 G01 | Drain-only recovery path: Existing backlog can shrink before normal admission resumes | Both fresh intake and feedback retries can be restricted below actual degraded service | Specify an inspectable draining transition instead of only normal versus stopped |
| N03 | G03 G19 | Cross-layer attempt budget and ownership: Work amplification remains accountable across kernel plugin SDK and participant calls | All retry producers are observable or bounded by an enforceable boundary | Persist business-operation identity separately from the permitted attempt budget |
| N04 | G05 G06 | A bounded startup compartment with cooling opportunity: A viable integration can bootstrap without its own restarts exhausting shared resources | Failure is transient or resource-related and startup limits really include descendants | Separate startup budget from steady-state budget and test cold entry from outside the running basin |
| N05 | G07 G08 | Quarantined integration plus inspectable last evidence: The damaged integration stops consuming restart attempts while its failure remains diagnosable | Diagnostics are captured safely and the diagnostic path does not require that integration | End retries after a bounded policy and require evidence before another activation |
| N06 | G12 | Unacknowledged incident handover record: An unresolved incident remains visible to a later legitimate recipient | Record is durable accessible and not confused with acknowledgment or repair | Treat nobody available as a real holding regime rather than escalating into automatic consent |
| N07 | G09 G10 G11 | Grouped incident delivery lane: Repeated symptoms do not consume one human decision each | Grouping preserves distinct actionable incidents and at least one legitimate recipient is reachable | Budget human alarm load and test which distinctions grouping may erase |
| N08 | G09 G10 G11 | Inspectably bounded suppression and re-entry policy: Temporary silence has an owner an end and a way back into actionable observation | Expiry is enforced independently enough and recurrence is addressed before repeated re-entry | Do not call an expiring mute a fix when it merely creates a flood-mute cycle |
| N09 | G13 G14 | Rejected draft with its contradiction evidence: Useful partial work and the reason not to release it remain available | The contradiction is legitimate and retaining the content is allowed | Keep worker output rejection evidence and permission to use separate |
| N10 | G13 G15 | Verifiable work package with explicit oracle provenance: A later reviewer can inspect what criterion and evidence justified acceptance | A genuinely adequate independent criterion exists outside the toy model | Do not promote modelled perfect repair into an implemented universal verifier |
| N11 | G16 G17 | Participant-enforced single-writer binding: One accepted business history persists despite competing local attempts | The actual participant rejects the excluded authority at its acceptance boundary | Test real fencing rather than interpreting local identity or green monitoring as ownership |
| N12 | G18 G21 | Pending-operation evidence journal: Intent attempts uncertainty and eventual receipt remain tied to the original business operation | Required facts are durable and not reconstructed from current guesses | Permit an explicitly unknown outcome without turning timeout into failure or replay into resend |
| N13 | G19 G20 | Retained participant deduplication evidence: Repeated requests do not necessarily create repeated business effects | Participant retention covers the uncertainty window and business identity truly matches | Treat participant memory as a separate conditional capability from the local pending journal |
| N14 | G20 G22 | Restore-quarantined historical work package: Historical intent remains inspectable without gaining fresh execution authority | Restore is recognized and independent current evidence can be sought | Use a read-and-reconcile boundary before any potentially already executed queued work |

22 interpreted regimes; 14 provisional structures for the investigated mechanisms.
This is NOT a new global answer replacing 17. The unmodelled scenarios can split, merge or invalidate this set.
An irreversible damage fact is not a surviving safety capability. A perfect oracle is an unproven prerequisite.
