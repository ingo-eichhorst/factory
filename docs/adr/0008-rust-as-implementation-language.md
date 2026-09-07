# ADR 0008: Rust as the Factory implementation language

- Status: Accepted
- Date: 2026-09-07
- Owners: Business Factory

## Context

ADR 0002 states that "Factory is distributed as one Rust executable named
`factory`" and ADR 0001 records that "Rust packaging was resolved by ADR 0002".
The language is therefore already settled in practice, but it was settled as a
by-product of a decision about daemon and client modes. No ADR records why Rust
rather than something else, and neither ADR lists alternatives.

That gap matters more here than for most decisions. The implementation language
is the least reversible choice in the project: it determines the plugin ABI
(ADR 0007 already anticipates "in-process Rust plugins"), the distribution
format, the hiring and delegation surface, and the cost of every later slice.
A future reader — including a future Business Factory agent — who finds only an
unexplained assertion will either treat it as unquestionable or re-litigate it
from scratch. This ADR records the reasoning so neither happens.

The working prototype (`scripts/factory_tasks.py`, 605 lines) is Python, and
the Pi tool extension is TypeScript. Choosing Rust means version 1 starts from
an empty crate rather than from that code.

## Decision

Implement Factory in Rust, as already assumed by ADR 0002.

### Primary driver: the decision owner wants to build this in Rust

This is recorded first because it is true, and because omitting it would be
actively harmful. If this ADR listed only the technical arguments below, a
later reader would over-weight them, and would conclude that the choice must be
reversed the moment one of them weakens. It should not be. Building Factory in
Rust is a deliberate goal of the project, not merely a means to an end.

Business Factory is a system its owner operates and extends personally. For a
system like that, working in a language its owner wants to work in is a real
engineering property: it sustains the attention the project needs over the many
slices ahead. A language chosen purely on a decision matrix, and then resented,
delivers less software than one chosen partly on preference.

### Supporting technical reasons

These would not by themselves have selected Rust over Go, but they confirm the
choice is sound rather than merely preferred.

1. **Single-binary distribution matches the stated architecture.** ADR 0002
   requires one executable serving both daemon and client modes, on a dedicated
   Mac. Rust produces a self-contained binary with no interpreter, virtualenv,
   or runtime to install or keep in sync.
2. **The invariants are the product, and Rust encodes them in types.** Factory's
   value is at-most-once delivery, one live lease per canonical workspace, one
   task per session, and cron idempotency. Sum types with exhaustive matching
   make the session and task state machines illegal-state-resistant at compile
   time, and `Result` makes the error paths — the ones that matter during
   recovery — impossible to ignore silently. This is the strongest technical
   argument for Rust specifically.
3. **Long-running supervision.** The daemon supervises plugin subprocesses over
   framed JSON-RPC (ADR 0007), with restart, health, and shutdown duties, and
   is expected to run for weeks. Predictable memory behaviour and no GC pauses
   suit that role.
4. **The plugin fast path already assumes it.** ADR 0007 anticipates in-process
   Rust plugins alongside supervised subprocesses; a different host language
   would make that path a foreign-function boundary instead.

## Alternatives considered

- **Go.** On the technical criteria alone, the closest call, and arguably the
  better fit for a daemon plus CLI plus subprocess supervisor: simpler
  concurrency, a standard library aimed squarely at this shape of program,
  single-binary output, and materially faster time to a working version 1. It
  loses on the type-level encoding of state machines in reason 2, and on the
  primary driver. This ADR does not claim Go would have been a mistake.
- **Python.** The prototype language, and the fastest path to a working system:
  `sqlite3` and `zoneinfo` in the standard library, no build step, already
  proven against the real task store. Rejected for distribution (no single
  binary without extra packaging machinery) and because optional typing is a
  weak guarantee for a system whose entire value is its invariants.
- **TypeScript / Node.** Matches the existing Pi extension and the harness
  ecosystem the agents already run in. Rejected for the same distribution
  reason, plus a runtime dependency on the operating system.

## Consequences

- Version 1 starts from an empty crate. The Python prototype becomes a
  reference implementation, not a starting point, and the 605 lines of working
  task-store logic are re-expressed rather than reused.
- Initial velocity is lower than Go or Python would give, and the learning
  curve is accepted deliberately rather than incurred by accident.
- Compile times lengthen the edit-run loop, which is felt most in slices 5–9,
  where the work is manual observation of a real harness rather than
  computation.
- **The one risk worth naming.** The project's largest unknown is not the
  domain model but the harness boundary: whether Herdr exposes stable pane
  identifiers and whether Pi emits reliable readiness, completion, and
  permission signals. That is the open risk in slices 5, 9, and 10, and it is
  answered by experimentation, not by design. Doing that discovery in a
  slow-to-iterate language would be the wrong trade.

  Mitigation: keep the Python prototype as the experimentation vehicle for
  adapter signals. Learn what Herdr and Pi actually emit there, write the
  adapter contract down, and implement it in Rust once it is known. This is
  consistent with slice 5, which already prescribes a manual proof before
  automation.
- The Python prototype and the `.pi` extension remain operational compatibility
  paths under the existing rule in `AGENTS.md`. Nothing about this decision
  requires removing them, and they should not be removed until the Rust
  implementation passes the same acceptance criteria.

## Relationship to earlier decisions

This ADR does not change ADR 0002; it records the reasoning behind a choice
ADR 0002 stated as a fact. ADR 0002 remains the authority on distribution and
on daemon and client modes.

Against the company design baseline, this ADR implements the command surface of
`.specs/design.md` §7 as a single executable and amends no baseline section: the
language choice is invisible to the domain model. It satisfies §10's requirement
that no container platform or external runtime be needed. Per ADR 0010, this
reference is stated rather than left implicit.
