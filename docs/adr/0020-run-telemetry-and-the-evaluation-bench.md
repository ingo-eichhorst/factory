# ADR 0020: Run telemetry and the evaluation bench

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory
- Amends: ADR 0011 consequence 3 — context utilisation becomes a run field

## Context

Two questions are being asked of the same data. *What does an agent actually do,
what does it find hard, and where does it spend its time?* And: *if a different
agent, harness, or model were put in front of the same work, would it do
better?* The first is instrumentation. The second is an experiment, and it needs
the first to already be in place.

Per ADR 0010, this ADR implements design baseline §12.3 (work instructions),
§12.5 (operating data), and §12.6 (unit cost), and gives §12.1 (final
inspection) a machine-checkable form for one class of task. It amends no
baseline section. It amends ADR 0011's third consequence, which declined to make
`contextUtilization` a run field; decision 5 below makes it one.

### What the baseline already carries

- §12.6 records model, token counts, and duration per run as a version-1 hook.
- §12.3 records the template version a run executed, and states the
  comparability problem directly: "two runs of 'the same' template are not
  comparable and no improvement can be attributed."
- §12.1 and §12.2 give a run a verdict and a rework reference.
- §12.5 projects throughput, cycle time, and scrap over the event log.
- ADR 0013 makes compiled context byte-stable, and a missing source a hard
  failure rather than a silent omission.
- ADR 0017 already records the `herdr --version` observed with each session,
  for exactly the reason this ADR generalises.

That is most of the raw material. The gaps are these five:

1. **No workload.** §12.3 versions the instruction; nothing defines a task set
   that can be re-run. Without one there is nothing to put a second model in
   front of.
2. **No axis of comparison.** §12.6 records *which* model ran; it never makes
   the same task under a different harness or model the unit of measurement.
3. **No verdict a machine can produce.** §12.1 leaves the gate to a human, and
   §12 is explicit that every figure in §12.5 and §12.6 "is uninterpretable
   until a verdict exists." A bench that runs unattended and reports only speed
   and token burn is reporting cost as if it were competence.
4. **Friction is not first class.** §12.5's waiting time is queue-side. What the
   user asked to see — where an agent got stuck, how often it was unblocked by a
   human, how many attempts a step took — happens inside a session and is
   currently recorded nowhere.
5. **No rule for comparing instruments.** Pi's state comes from Herdr and
   claude-code's from Irrlicht (ADR 0017). Those are two different instruments
   with different reliability, and averaging across them produces a number that
   looks like an answer.

## Decision

### 1. A bench verdict comes from the fixture, not from a judge

A bench case is a fixture that carries an executable acceptance gate: a command
the fixture owns, whose exit status is the verdict. `check.sh` in this
repository — "the only gate", running rustfmt, clippy with warnings denied, and
the tests — is the shape, and `fixtures/registration/` is the precedent for a
miniature working world kept as a fixture.

Version 1 admits **no model-graded verdict.** An LLM judge is itself an
unvalidated harness-and-model combination, so scoring the bench with one makes
the instrument a function of the thing being measured. ADR 0017 already states
the principle in the form this ADR adopts as its measurement rule: *a wrong
observation is worse than none, because it looks like an answer.*

This does not move §12.1's gate for production runs. Those stay human. What the
fixture corpus adds is a class of task where the criterion happens to be
executable, which is the only class a bench can run unattended.

### 2. The unit of measurement is a pinned tuple, and an unpinned run is not comparable

A measurement is the outcome of:

| Pinned | Source |
|---|---|
| Fixture case and its revision | The bench corpus |
| Task template version | §12.3 hook |
| Harness and its build | Adapter |
| Model identifier and, for local models, quantisation and runtime | Adapter, or the launcher for a local model |
| Compiled-context hash | ADR 0013 — byte-stable compilation is what makes this hashable |
| Adapter and instrument versions | `herdr --version` (ADR 0017), Irrlicht stream version |

A run missing any of these is recorded and **marked not-comparable**, never
silently included. Dropping it loses an operating-data record that is still
valid for §12.5; averaging it in produces the wrong number quietly.

### 3. Data lands in three tiers, and only one of them is replayed

ADR 0003 requires replay to be deterministic and free of plugin calls; ADR 0011
requires observation never to be replayed as a domain event. Both hold, because
the three kinds of data are stored differently.

| Tier | Contents | Durability |
|---|---|---|
| **Event payload** | Model, harness and version, adapter and version, template version, context hash, token counts, durations, gate exit status, verdict | Captured once when recorded, re-read verbatim on replay, never re-derived |
| **Trace artifact** | Per-step timings, tool and command sequence, the transcript where decision 8 permits it | A file under `.factory/`, referenced from the run event by content hash |
| **Observation samples** | Herdr and Irrlicht polls of session state and context pressure | Derived and disposable; deleting them changes no replayed state |

The trace is an artifact rather than a projection on purpose. It is the only
place the fine-grained "what took long" data exists, and a projection that can be
rebuilt from the event log cannot contain more than the event log does. Runs
already carry artifact references (`run_complete`), so this adds no concept.

### 4. What is comparable across harnesses, and what is not

| Metric | Comparable across harnesses |
|---|---|
| Wall-clock duration, token counts, gate exit status, verdict, attempt count | Yes |
| Blocked duration, turn count, waiting time, idle intervals | **No** — within one adapter only |

Blocked duration under Pi is hook-reported by the harness itself; under
claude-code it is inferred from a transcript. They are not the same measurement
and this ADR writes down no conversion between them. Two models compared under
one harness are comparable on everything; two harnesses are comparable on
wall-clock, tokens, and outcome.

A Pi session started without the Herdr state extension is *observation-degraded*
(ADR 0017 decision 2). Its friction metrics are recorded as **unusable**, not as
zero and not as missing-at-random.

### 5. Friction is recorded as it happens, not reconstructed afterwards

The question "what does this agent find hard" is answered by these signals, each
written when it occurs:

- **Blocked intervals with their reason** — the permission prompt or question
  that stopped the run, and how long it stood. Herdr's `blocked` (ADR 0017
  decision 4) is the source for Pi.
- **Attempts per gate.** How many times the acceptance command was run before it
  passed is the closest thing to a first-pass-yield figure at step granularity.
- **Human interventions.** Every operator prompt or key sent into a running
  session. A run that only finished because a human steered it is not evidence
  that the combination can do the work.
- **Rework references.** §12.2's hook, read as a depth: a case reworked twice is
  a harder case than one reworked once.
- **Redeliveries.** Task deliveries retried after an ambiguous failure, which
  are cost without progress.
- **Context pressure.** Peak utilisation over the run, and its value at the
  terminal event. This is what amends ADR 0011: a run that failed at 96 % context
  failed differently from one that failed at 30 %, and that distinction is not
  recoverable from tokens alone.

Context utilisation is **nullable and per-adapter**. ADR 0011's own measurement
found Irrlicht resolving a transcript path for one of three Pi sessions, and
ADR 0017 makes Irrlicht enrichment for Pi joined on exactly that path. So for Pi
the field will frequently be absent, and absent must read as absent.

### 6. The bench is a task run in a scope it owns; no new primitive

§12 permits no sixth primitive. A bench case is a task run, delivered to a
session of a registered bench scope, whose workspace is a fixture directory the
bench owns and whose result is the gate's exit status. Scopes, agents, sessions,
tasks, and context, unchanged.

Two repository rules constrain the mechanics and are settled here rather than
discovered later:

- **No worktrees.** Creating, merging, or cleaning up Git worktrees requires an
  explicit human request. A bench that wants a clean checkout per case therefore
  does not make one. Each case owns a fixture directory and declares a **reset
  command** that returns it to its starting state; the bench runs that command
  and verifies it, and a case whose reset fails is skipped rather than run dirty.
- **Everything written lands under `.factory/`.** Traces, gate output, and result
  records are Factory writes and obey the allowlist. Work the agent performs
  inside the fixture directory is the agent's output, not a Factory write, and is
  what the reset command reverses.

### 7. model-lab measures the runtime; Factory measures the line

§12.6 already names `projects/model-lab/` as the calibration bench, and this ADR
does not annex its job.

| Question | Bench |
|---|---|
| Tokens per second, quantisation, context window, power draw, does this model load at all | model-lab |
| Does this harness-and-model combination finish this task, at what cost, and where does it get stuck | Factory |

The boundary is that model-lab measures a model in isolation and Factory measures
a combination doing repository work. A model-lab figure is an input to
interpreting a Factory result, never a substitute for one.

### 8. Transcripts are archived only where there is nothing to leak

"Record what the agent does" read naively means archiving every transcript, and a
transcript is the most efficient secret vacuum available: it contains whatever
the agent read.

- **Bench and fixture scopes** contain no real company material by construction,
  and their traces may be archived in full.
- **Production runs** store the event-payload metrics and bounded, size-capped
  excerpts — the failing gate output, the blocking question — and no transcript.

The existing rule stands unchanged: no secret in prompts, task records, notes,
logs, fixtures, or generated context.

### 9. Version 1 carries the fields; the bench itself is post-version-1

§12.9 marks 12.5 as absent from version 1 and 12.6 as a hook, and the backlog
defers 12.5. Nothing here overturns that. Version 1 records the fields of
decisions 2, 3, and 5 on each run and writes the trace artifact. It does not
ship a corpus, a runner, a comparison command, or a report.

The reason to decide this now, before the bench exists, is that **a run that did
not record its inputs cannot be measured retroactively.** Slice 5 produces the
first recorded runs. A run recorded without its context hash, template version,
and instrument versions is an anecdote, and no later work can turn it into a
measurement.

## Consequences

- The Slice 5 record grows by the pinned tuple of decision 2 and the friction
  fields of decision 5, and by a trace artifact with a content hash. This is a
  schema change to design and to write once, not one per station.
- `contextUtilization` becomes a run field, which ADR 0011 explicitly declined.
  It is nullable, per-adapter, and for Pi frequently absent until the Irrlicht
  correlation gap ADR 0017 describes is closed.
- The corpus becomes a maintained asset. Fixture cases rot as tooling moves, and
  a case whose gate fails for everyone measures the fixture rather than the
  agent. Reset verification in decision 6 catches the coarse form of this; drift
  in what a case *demands* is caught only by review.
- Benchmarking gains a real cost. Each measurement is a full agent run, and a
  matrix of four cases across three combinations is twelve runs of billed or
  local inference. The corpus stays small and deliberate for that reason.
- Comparisons are honest by construction and therefore narrower than they look.
  Most useful results will be same-harness, different-model. Cross-harness
  claims are limited to wall-clock, tokens, and outcome.
- The learning loop (§12.8) gets its missing input. It was specified to read
  decisions and verification findings; friction signals and bench results are
  the evidence that would let a proposed instruction change be checked rather
  than argued.

## Open items

1. **The first corpus is unspecified.** Which cases, at what difficulty spread,
   and who ratifies that the spread is real rather than assumed. A corpus where
   every combination passes measures nothing, and neither does one where none do.
2. **No cross-adapter state mapping exists.** Decision 4 forbids comparing
   state-derived metrics across harnesses because no mapping has been written.
   Whether one *can* be written — Herdr's `idle|working|blocked|done` against
   Irrlicht's `ready|working|waiting`, from two different derivations — is open,
   and ADR 0017's own measurement is the reason to doubt it.
3. **Observation sampling rate.** Peak context utilisation is a sampled maximum,
   so it depends on the poll interval, and the interval has a cost. Neither is
   chosen here.
4. **Whether a model-graded verdict is ever admitted.** Decision 1 refuses one
   for version 1. The condition that would change that is measurable: agreement
   with fixture gates on a held-out set of cases, established before the judge
   scores anything that has no gate.
5. **Attribution across a multi-turn run.** Token and duration figures are
   per-run, and a run that spanned a model switch or a harness restart attributes
   everything to whatever the adapter last reported. Slice 9's restart drills are
   where this surfaces.
