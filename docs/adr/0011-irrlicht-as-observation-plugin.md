# ADR 0011: Irrlicht as the session observation source

- Status: Accepted
- Date: 2026-09-07
- Owners: Business Factory

## Context

Factory needs to see centrally what each agent is doing. The adapter contract in
`.specs/design.md` §3 requires `observe()`, and slice 5 must decide where that
observation comes from. Two sources were available: a back-channel out of Herdr,
which owns the PTYs, or Irrlicht, the menu-bar and daemon product in
`projects/irrlicht/`.

Per ADR 0010, this ADR implements design baseline §3 — it gives the adapter's
`observe()` a concrete source — and supplies part of the raw material §12.5
would later project over. It amends no baseline section.

Evidence gathered on the production machine on 2026-09-07:

- Irrlicht is running (`Irrlicht.app` and `irrlichd`).
- It carries inbound adapters for eleven harnesses, including both of Factory's:
  `pi` and `claudecode`.
- The Pi adapter reads structured transcripts under
  `~/.pi/agent/sessions/--<cwd>--/*.jsonl`, not terminal output, and derives a
  three-state model: `working`, `waiting`, `ready`. Process liveness comes from
  `kqueue NOTE_EXIT` and PID-to-working-directory matching.
- It exposes a versioned loopback stream, `GET /api/v1/sessions/stream`, plus a
  relay with token authentication and mDNS discovery for other machines. The
  payload carries session state and `contextUtilization`.
- The correlation keys already exist on both sides. Irrlicht groups sessions by
  working directory (`--Users-factory-business-factory-projects-factory--`),
  which is Factory's canonical workspace path and lease key. And the session
  UUID pinned in `assistant/.factory/config.yaml` as `--session-id
  019fe882-…` is the same UUID in the transcript filename Irrlicht reads.

## Decision

### 1. Irrlicht is the session observation source; Herdr remains the terminal runtime

Factory does not derive agent state by reading terminal output. Herdr keeps
owning PTYs, panes, and attachment exactly as design §3 describes.

The rejected alternative is worth recording: obtaining state from Herdr means
parsing the screen. Design §5 already holds that "terminal scrollback and
native-harness conversation history are conveniences, not canonical Business
Factory state", and ADR 0008 names harness readiness signals the project's
largest unknown. Irrlicht solves that from the harness's own structured output,
for eleven harnesses. Rebuilding it against the PTY would redo that work in the
less reliable direction.

### 2. It is a scope-activated plugin, not a dependency

Per ADR 0001, runtime systems belong behind versioned plugin contracts. The
observation plugin subscribes to the Irrlicht stream and translates events into
Factory observations over the ADR 0007 host protocol. It is enabled per scope
like any other plugin, and no Irrlicht type appears in the Factory domain model.

### 3. Factory must degrade without it

If Irrlicht is absent, stopped, or a version Factory does not understand, the
adapter falls back to the manual-confirmation mode slice 5 and slice 10 already
prescribe. Observation is never on the path that makes a task durable: design §5
requires the task to be committed before any terminal input is attempted, and
that ordering does not change.

### 4. The direction of truth never inverts

| Question | Owner |
|---|---|
| What is this session doing right now — working, waiting, ready, how much context is left | Irrlicht |
| What work exists, who owns it, what was decided, what the result was | Factory |

Irrlicht state is derived and disposable, consistent with design §2.3 ("a session
is disposable") and with the MVP criterion that runtime state survives
independently of terminal scrollback. It is therefore **read as an observation
and never replayed as a domain event.** Writing Irrlicht-derived state into the
append-only store as if it were canonical would violate ADR 0003's rule that
replay must be deterministic and must not depend on plugins.

### 5. Correlation uses identifiers that already exist

The plugin joins an Irrlicht session to a Factory session on the canonical
workspace path and, where present, the harness session UUID. No new identifier
is introduced, and Factory does not ask Irrlicht to store a Factory ID.

## Consequences

- Slice 5's largest open risk shrinks. The question was whether Pi emits reliable
  readiness, completion, and permission signals; a working parser for exactly
  that already exists and is in production use on this machine.
- Factory gains a dependency on a second local product. This is acceptable
  because it is first-party, because ADR 0001 forces it behind a plugin
  contract, and because point 3 keeps Factory functional without it.
- `contextUtilization` becomes available per session. It is related to the §12.6
  unit-cost hook but is not the same measurement, and this ADR does not make it a
  task-run field.
- The voluntary marker channel is available at no transport cost. Agents already
  emit `irrlicht-eta` and `irrlicht-question` markers into normal output, which
  Irrlicht's parsers read. Factory task and run identifiers could ride the same
  path. This ADR notes the option and does not adopt it.

## Open items

1. **Detection parity for Herdr-launched sessions.** Irrlicht matches processes
   by working directory. Whether a Pi session started inside a Herdr pane is
   detected identically to one started directly is unverified, and slice 5 must
   confirm it against a real pane before the adapter relies on it.
2. **Version skew.** The stream is versioned (`/api/v1`), but there is no agreed
   behaviour for a payload Factory does not understand. The plugin should treat
   an unknown state as "no observation" and fall back to manual confirmation
   rather than guessing.
