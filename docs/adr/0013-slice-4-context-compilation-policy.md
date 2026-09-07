# ADR 0013: Slice-4 prerequisites — no generated compatibility files, and the context error policy

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory

## Context

Slice 4 carries two unresolved decisions: "set context size/error policy and
decide whether version 1 needs any generated compatibility file at all; manual
context injection is sufficient for the first rollout." Both block the crate, so
this ADR settles them.

It implements `.specs/design.md` §2.5 (context compiled root-to-leaf with source
headings) and §4's generated-file safety. It amends no baseline section.

## Decision 1: version 1 generates no compatibility file

Design §4 permits Factory to generate harness compatibility files under
`.factory/generated/`, with an ownership marker, and requires a command to stop
with a conflict if a non-Factory file already occupies a generated target.
Version 1 exercises none of that. `factory context show` prints the compiled
text; a human or an adapter passes it to a harness.

The reason is not that generation is hard. It is that generation is the only
part of Slice 4 that *writes*, and writing is where every risk in this slice
lives — the ownership marker, the conflict check, the adoption path for an
existing file, and the regeneration rule. Deferring it removes that entire
surface rather than shrinking it, and the backlog already records that manual
injection suffices for the first rollout.

Two consequences worth stating, because they are what makes this cheap rather
than merely postponed:

- **Slice 4 becomes a pure function.** Given an ordered list of scope
  directories, an agent definition, and a task prompt, it returns text. It opens
  files for reading and creates nothing, so the design §4 write rule is
  satisfied by construction rather than by a check that must be maintained.
- **The `.factory/generated/` rules in design §4 stay written down and stay
  unimplemented.** They are not deleted, because the moment an adapter needs a
  `CLAUDE.md` shim the conflict rule matters again. This ADR defers the code,
  not the policy.

If a later slice adds generation, the ownership marker and conflict behaviour
must be specified then, against a real harness that needs it.

## Decision 2: a missing context file is an error, not a silent gap

Design §2.5 requires byte-stable compiled context, and Slice 4 requires that
inspection "reports missing readable context files clearly". Those pull in
different directions if a missing `AGENTS.md` is merely noted, because the
compiled text then depends on which files happen to exist, and the same scope
compiles to different bytes on two machines.

So: **compilation fails if a declared context source cannot be read.** It names
the file, the scope that expected it, and the action. It does not substitute an
empty section and continue.

This is the conservative direction, and the failure mode it avoids is the
expensive one. A missing `AGENTS.md` silently skipped produces an agent running
with less instruction than its operator believes it has — mandates about
secrets, external side effects, and approval gates quietly absent, with no
symptom until the agent does something it was supposed to have been told not to
do. A hard failure at start is visible in a second and fixed in a minute.

The registered scope must have an `AGENTS.md`. That is a property Slice 3's
registration checks, so by the time Slice 4 compiles, the file's absence means
something changed underneath — exactly the case that deserves a stop.

`factory context show` reports each source path, whether it was readable, and
its byte count, so the operator can see the composition and not only the result.

## Decision 3: no size limit, but the size is always visible

Version 1 imposes no maximum on compiled context. Any specific limit would be
invented: the real bound is the target harness's context window, Factory does
not know which model a harness is configured with, and a cap tight enough to be
safe for the smallest model would reject legitimate context for the largest.

Instead the size is never hidden. `factory context show` prints the total byte
count and the per-source breakdown, so an operator diagnosing a truncated or
confused agent can see immediately that the compiled context is large and which
file made it so.

This is a deliberate trade: Factory reports and the human judges. Revisit it
when an adapter can report its harness's context window, at which point the
check becomes a real comparison rather than a guessed constant.

## Decision 4: version 1 follows no `[[links]]` when compiling

Design §2.5 says knowledge, memory, and skills may later be *referenced* from
`AGENTS.md` but are not separate runtime primitives yet. With the knowledge note
graph now under `.factory/knowledge/`, the question this raises is whether the
compiler should follow `[[note-name]]` links out of `AGENTS.md` and inline them.

It does not. Context is exactly the `AGENTS.md` chain, the agent definition, and
the task prompt — nothing else.

An unbounded graph walk would make compiled context neither bounded nor
byte-stable, and §2.5 requires both. Every link followed pulls in a note that
links onward, so the compiled size would depend on the shape of the graph on
that day rather than on the scope's configuration. Bounding the walk by depth
would restore termination but not stability: adding one link to an unrelated
note silently changes what every agent in that subtree receives.

This is the open question recorded in design §12.4, and version 1 answers it by
declining. If material supply later needs declared knowledge in context, §12.4's
own smallest next step is the right shape — **declared material paths** in
`AGENTS.md`, compiled with source headings and failing loudly on a missing path.
Declared, enumerable, and stable, rather than reachable.

## Consequences

- Slice 4 is a pure compilation function with no write path; its acceptance
  criterion about a non-Factory file under generated output becomes vacuous for
  version 1 and should be marked as deferred rather than silently dropped.
- Slice 3 must verify that a registered scope has a readable `AGENTS.md`, since
  Slice 4 now depends on it.
- `factory context show` gains per-source byte counts.
- The §12.4 graph-traversal question is answered for version 1 and stays open
  for material supply.
