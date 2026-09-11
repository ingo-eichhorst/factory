# A5 model specification and evidence boundary

**Audit erratum:** This is the historical specification. [A6's independent audit](../a6/a5-claim-dispositions.csv) corrects the incident interpretation of §5: the code resets represented pending backlog, not merely new notifications. Other restrictions concern policy classification, capped pressure, continued post-acceptance review and restricted return tests. The old transition code is preserved for reproduction, not presented as a corrected incident-preservation model.

## 1. What “attractor” means here

An attractor is not merely a state reached after a fault. We look for a set that:

1. is invariant or recurrent under specified transition rules;
2. is reached from states outside that set;
3. draws a declared neighborhood of small perturbations back into it;
4. has a stated basin, sustained inputs and exit conditions.

The script operationalizes this on a **small discrete state grid**. It follows deterministic transitions, recognizes repeated states, normalizes cycle rotations and tries neighboring states. It reports `model-attractor` only where the listed seeds include a transient into the set and every tested neighbor returns. Semantic exceptions such as policy holds and ordinary completion remain separate.

This is not a general mathematical proof of asymptotic or structural stability. Discrete adjacency is chosen, state projections omit many variables, and parameter changes can destroy the result. No empirical distribution supplies probabilities for the seed grid. A basin containing seven selected seeds is not a 7/13 incident probability.

### Classes deliberately not collapsed into attractors

- **Transient:** the path before a recurrent set. A long wait in a finite trace does not establish permanence.
- **Fixed without demonstrated attraction:** a queue can remain at one value while nearby queues go elsewhere.
- **Escape:** growing beyond the observation window. We do not silently clamp it into a fake equilibrium.
- **Externally driven cycle:** alternating writers follow an externally specified periodic schedule.
- **Policy hold:** a rule disallows further automatic action until new authority or evidence arrives.
- **Information-starved hold:** the model deliberately supplies no new response. Changing that environmental assumption changes the outcome.
- **Ordinary completion:** a job finishes; this does not by itself establish a self-maintaining operating regime.
- **Absorbing damage fact:** a second effect happened. The damage is historical; the remaining service dynamics are not modelled.

A policy may mathematically create an absorbing or attracting set. Keeping its classification separate is intentional: its persistence comes from a chosen prohibition, not evidence that the uncontrolled system naturally settles there.

## 2. Pipeline and provenance

`dynamics.py` reads only `id` and `stressor` from the two historical source tables, discarding old residue labels, controls, proposed survivors and remaining limits. Its behavior is defined by `cases.json` and its transition functions. It never imports the A4 generator or candidate labels.

After executing and inspecting this stage, its hashes were recorded against the task:

- `cases.json`: `5195b06c75c492fd31d79b68209c67b31899c649f5ebb7229834b6caf4989c78`
- `dynamics.py`: `a3b2f752e4e3058b0a35cd0a5dfdf751fd783a6e44241e8a1950464cddc88035`

Only then were `regimes.csv` and `candidates.csv` written. `derive.py` joins those downstream interpretations for traceability. It does **not** discover surviving structures or prove the qualitative interpretation.

This sequencing limits one form of circularity, but not all bias: the same analyst selected the models and knew the earlier analysis. There was no independent observer, blinded classification or learned physical model. Earlier quality research is documented in [A4 sources](../a4/sources.md); this pass adds no claim of externally validated attractor theory or observed Factory dynamics.

## 3. Queue and retry model

State: pending work `q`, an integer. A step is an abstract scheduling interval, **not** a measured second.

```text
service(q) = 3 when q < 6, otherwise 1
retries(q) = floor(q / 2) when retries enabled and q >= 4, otherwise 0
incoming = fresh_arrivals + retries(q)

with admission gate:
    incoming = min(incoming, max(0, 5 - q))

q_next = max(0, q + incoming - service(q))
```

Seeds: `q = 0..12`. Neighbor perturbations change `q` by one. Above 100, the trajectory is reported as escape, not a terminal operating state. For the escaping cases the formula itself implies continued growth if parameters remain fixed: retry production outstrips congested service.

The high-load capacity drop and retry law are **stipulated**, not measured. The gate rejects new intake; it does not discard already accepted backlog. Historical effects, leases, memory bytes, tenants and control-lane capacity are not represented. Thus a gate that works in this model is only a proposal until all real work-producing paths are accounted for.

Noteworthy counterfactual: stopping fresh arrivals alone need not stop retry escalation. Stopping retries alone can leave arrivals equal to degraded service, producing a neutral backlog plateau. Pausing both permits draining. These distinctions would disappear in a single label such as “overloaded”.

## 4. Restart and resource-pressure model

State: `(heat, wait, attempts, phase)`, where heat is an ordinal pressure level from 0 to 6 and phase is boot, run or quarantine.

- A boot adds two heat units; boot fails above four or when `bad_binary` is set.
- Immediate restart provides no cooling interval.
- A cooldown step reduces heat by one and decrements the waiting timer.
- Successful running reduces heat toward one.
- With finite attempts enabled, the third failure enters quarantine and pressure cools toward zero.

Seeds vary initial heat from zero through six. Perturbations vary heat only; the tests do not treat replacing the binary as a small perturbation. Heat is capped because this deliberately coarse resource model has a saturation state; it is not a physical thermal simulation or a safety model.

The resulting hot boot trap is caused by the assumed feedback. With a permanently defective binary, cooling generates a cycle rather than running operation. The finite-attempt policy creates a hold, not an automatic repair.

The model stores no actual plugin stderr or diagnostic artifact. A candidate “quarantined integration with inspectable evidence” therefore **adds a required implementation structure**; the simulation has only demonstrated why a no-more-starts regime can matter.

## 5. Alarm-load model

State: `(pending_incidents, mute_remaining)`. `-1` denotes indefinite mute.

- Each active step adds a configured number of alarm items and removes the configured human service capacity.
- Grouping collapses the offered repeated symptom stream to at most one item.
- More than six pending items triggers the configured overload-mute response.
- A muted step suppresses new items. A finite mute counts down, then returns to the same active dynamics.

Seeds vary pending items from zero through six; perturbations vary pending count only. Constant arrival of four and human capacity one creates overload. With a three-step mute the system repeatedly returns to that overload, producing a cycle. Grouping can make the queue serviceable; with zero human capacity it only makes the wait bounded.

The human muting response and service rate are assumptions, not psychological measurements. Safe grouping of distinct incidents is not proven. No sender, external observer, credential, network or actual notification is used. Whether a monitor has an independent failure domain remains a separate empirical question.

## 6. Verification-feedback model

State: `(confidence, artifact_wrong, phase)`. Confidence is a toy integer from zero to four; it is not a probability.

- Echo verification increments confidence without changing the artifact.
- Confidence three or higher marks it accepted.
- An independent blocking verifier detects the wrong artifact and holds it.
- A perfect repair verifier, in a separate counterfactual case, corrects it and begins review again.

Seeds use the same wrong artifact with different initial confidence. Perturbations change confidence, not the evidence source. The confidently wrong state draws reduced-confidence neighbors back because the same confirming procedure remains in place.

The `artifact_wrong` truth bit is known to the test author. Factory does not generally possess it. The block and repair variants deliberately stipulate adequate independent knowledge; they do not demonstrate that such an oracle exists. This is why the independently verifiable-package candidate is marked `unproven-prerequisite` rather than a discovered universal verifier.

## 7. Alternating-writer model

State: `(business_value, next_writer)`. Writers zero and one wish to install their respective values. The external schedule alternates them.

Without fencing, each writes its value. With fencing, only writer zero is accepted by the participant. The scheduling bit alternates in both cases.

This is a useful negative example for automated attractor naming: a two-state cycle in full state may represent harmful business overwrites or just a harmless scheduling phase. Both depend on external periodic forcing. Real concurrent writes, lost updates, authority transfer and actual fencing protocols are not implemented by this model.

## 8. Effect and missing-information model

State records a sender phase, a **test-observer ground-truth** accepted-effect count and a bounded age counter. Scenarios vary retry policy, eventual versus permanently lost response, participant deduplication memory and restore gating.

- The first permitted send can be accepted even when no response is delivered.
- Repeating without retained participant deduplication creates a second accepted effect.
- In the eventual-response case, waiting increments age to three; the next transition confirms the response.
- A restore case starts with one effect already accepted but a locally apparent new intent.
- A restore gate holds before a new send.

Age saturates at three only because later ages are behaviorally equivalent **under these specific rules**. That abstraction does not represent real 90-day retention or arbitrary timeouts. Lost-history cases are parameter counterfactuals, not a simulation of an actual participant's storage engine.

The internal `damage` phase is an **analysis stop marker after observer-known duplicate acceptance**, not a message revealing the damage to the sender. The real sender might still know only “unknown”. The model does not claim to recover that missing information or undo the effect. Likewise, repeated deduplicated calls can leave the same state tuple unchanged while still consuming work at each transition; state appearance alone is insufficient.

## 9. What would falsify or weaken these interpretations?

- Real capacity never decreases in the way the queue model assumes.
- Real retry producers already have an enforceable global budget.
- Startup retries do not create cumulative pressure, or running load creates different feedback.
- Real alarm handling does not follow the assumed suppression dynamics.
- Verification introduces independent evidence rather than merely increasing confidence.
- The participant does not enforce the fence or retain deduplication for the required interval.
- An event called “lost” is in fact guaranteed to arrive within a known bound.
- Important hidden state changes the basin: disk limits, authority, adverse actors, changing demand, deadlines or human availability.

The proposed residues should be split, merged or abandoned when these conditions fail. The software tests check the **toy rules and documentation claims**, not whether the equations describe Factory. No amount of agreement between a generator and its own tests closes that gap.
