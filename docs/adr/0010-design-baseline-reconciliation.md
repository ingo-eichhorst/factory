# ADR 0010: Reconciling the design baseline with the project ADRs

- Status: Accepted
- Date: 2026-09-07
- Owners: Business Factory

## Context

Two document sets describe Factory, and neither references the other. The
company design baseline `.specs/design.md` defines five primitives, a Factory
supervisor, and harness adapters. The project ADRs 0001, 0002, and 0007 define
a microkernel, a daemon, thread and task agents, and supervised plugin
subprocesses. ADRs 0001–0007 contain no reference to `.specs/design.md` at all,
so the two evolved independently and the divergences went unnoticed.

The implementation backlog names none of daemon, plugin, transport, or thread.
An implementer starting Slice 5 today would have to guess whether a harness
adapter is an in-process trait or a supervised subprocess.

This ADR states which apparent conflicts are only differences of altitude, and
resolves the one that is real.

## Decision

### 1. Three apparent conflicts are layering, not disagreement

| Design baseline | Project ADRs | Resolution |
|---|---|---|
| "Factory supervisor" (§3) | "daemon" (ADR 0002) | The same component. Compare the responsibility lists in design §3 and ADR 0002 §2: registry, task state, leases, context, reconciliation. `daemon` is the implementation term; `supervisor` is the domain term. |
| Harness adapters (§3) | Supervised plugin subprocesses (ADR 0001, 0007) | The ADRs specify *how* the adapter boundary of design §3 is hosted. The adapter contract `start/send/observe/interrupt/stop` is carried over the framed JSON-RPC protocol; it is not replaced by it. |
| `permanent` / `temporary` agent lifetime (§2.2) | Thread agent / task agent (ADR 0001 §1) | The same distinction. A thread agent is a `permanent` agent; a task agent is a `temporary` agent. |

Where the terms differ, **the design baseline supplies the domain vocabulary and
the project ADRs supply the implementation vocabulary.** Neither is wrong; a
document that uses one should say which it means when ambiguity is possible.

### 2. Threads and messages are not a version-1 primitive

This is the one real divergence. ADR 0001 gives a thread agent "durable messages
in one or more threads" and uses the word `thread` twelve times.
`.specs/design.md` never uses it, states in §2.4 that "a task is the only durable
unit of work exchanged between humans and agents or between agents", and defers
"separate message queues" in §8.

**The design baseline governs version 1: the task remains the only durable unit
of work exchanged.** Threads and messages are target-state, not version-1 scope.

Three pieces of evidence support this rather than the reverse:

- Design §8 defers separate message queues explicitly, and §9 ADR-001 keeps
  "exactly five version-1 domain primitives".
- Design §12.7 already covers inbound communication without a message primitive:
  intake "creates a task run with recorded provenance", classified as later work.
- The behaviour already exists in production without one. The assistant scope
  runs two permanent agents against an iMessage channel; the conversation lives
  in iMessage and in the Pi session, not in a Factory message store. A permanent
  agent can serve a communication channel without Factory owning the thread.

A thread primitive may still be right later. It is deferred, not rejected, and
when it arrives it must compose from the five primitives per design §8.

### 3. The ADRs must state their altitude

ADRs 0001, 0002, and 0007 describe the Factory product, including capabilities
beyond version 1, and none of them says so. Any statement in a project ADR that
exceeds the version-1 scope of `.specs/design.md` is target-state unless the
implementation backlog carries a slice for it.

Going forward, a project ADR that constrains version 1 must reference the design
baseline section it implements or amends. Had ADRs 0001–0007 done so, this
reconciliation would not have been necessary.

## Consequences

- `projects/factory/AGENTS.md` marks the thread/message boundary as target-state.
  This matters more than the ADR text: `AGENTS.md` is loaded as instructions at
  every session start, so an unmarked target-state boundary reads as a
  requirement to build one now.
- The backlog gains no thread or message slice.
- Slice 5's harness adapter is built to the design §3 contract. Whether it is
  hosted in-process or as a supervised subprocess per ADR 0007 is a hosting
  decision that Slice 5 must state explicitly rather than leave implied — it is
  recorded here as an open item, not resolved.
- No accepted ADR is rewritten. ADRs 0001, 0002, and 0007 keep their decisions;
  this ADR records their scope relative to version 1.

## Open item created by this ADR — closed 2026-09-08 by ADR 0014

The backlog is silent on the daemon and on plugin hosting. Slices 5–10 describe
adapters and a CLI without saying whether a long-running daemon owns the
mutations, as ADR 0002 requires, or whether each CLI invocation is its own
process, as the current Python prototype is. This is the next reconciliation
step and it should be settled before Slice 5, not before Slice 1: slices 1–4
(configuration validation, SQLite store, scope registry, context compiler) are
identical under either answer.

**Resolved: a long-running daemon owns all mutations (ADR 0014).** This confirms
ADR 0002 rather than amending it, so the divergence this ADR recorded is closed
in favour of the project ADRs.

The claim that slices 1–4 are identical either way was afterwards *checked*
rather than left as an argument — ADR 0012 found that WAL with `busy_timeout`
and `BEGIN IMMEDIATE` serializes mutators identically under both models — and
those three slices were then implemented and committed while the question was
still open, without any of them having to guess.
