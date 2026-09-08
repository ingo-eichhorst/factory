# ADR 0014: A long-running daemon owns all mutations

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory
- Decided by: the human owner, 2026-09-08

## Context

ADR 0010's closing open item was the last structural question in the version-1
plan: does a long-running daemon own all mutations, as ADR 0002 requires, or is
each invocation its own process, as the Python prototype is?

It was deliberately parked before Slice 1 and scheduled before Slice 5, on the
argument that slices 1–4 are identical under either answer. That argument was
later *checked* rather than assumed, while settling ADR 0012's locking strategy:
WAL with `busy_timeout` and `BEGIN IMMEDIATE` serializes concurrent mutators
correctly under either model, so nothing in configuration validation, the SQLite
store, the scope registry, or the context compiler depends on the answer. Slices
1, 2, and 4 were implemented and committed with the question still open, and
none of them had to guess.

Slice 5 is where it stops being avoidable, because an adapter that starts a
harness has to belong to something that outlives a command.

## Decision

**Factory runs as a long-running daemon, and the daemon is the sole owner of
core state mutations.** The CLI is a client.

This confirms ADR 0002 rather than amending it, and closes the divergence
ADR 0010 recorded. It is also what the design baseline already assumed without
saying so: design §7 lists `factory start|stop|status|doctor` as a command
group, and starting and stopping are meaningless against a process that exits
after every command.

## Why this rather than process-per-invocation

Three things Factory must do cannot be done by a process that exits.

**A harness session outlives the command that started it.** Design §2.3 defines
a session as a live harness process in a Herdr pane, with state
`starting | running | disconnected | failed`. Something has to observe the
transition from `starting` to `running` and record it. Under
process-per-invocation, `factory agent start` would exit while the session is
still `starting`, and nothing would ever write `running` — readiness would have
to be rediscovered on the next unrelated command, which is how a system acquires
states that are true in the database and false in the world.

**At-most-once delivery needs a custodian.** Design §5 records a delivery
attempt before writing to the PTY and declines to resend after an ambiguous
failure. That contract needs a process that is present across the ambiguous
window. A CLI invocation that dies mid-delivery leaves the reconciliation to
whoever runs the next command, which may be hours later.

**Cron and reconciliation are continuous, not on-demand.** Slice 11's dispatcher
is minute-level, and Slice 9's reconciliation must react to a Herdr restart.
Under process-per-invocation both become `launchd` jobs poking a database — and
that is a daemon, just one assembled from cron entries, without a supervisor's
ability to hold state or to be asked what it is doing.

The honest cost is that the daemon must be running for Factory to work, which
process-per-invocation would not require. That is what design §7's
`factory start|stop|status` and the safe-mode recovery path of ADR 0002 exist
to manage, and it is a cost the prototype already pays informally through
`launchd`.

## Consequences

- **ADR 0010's open item is closed.** Slices 5–10 describe a daemon and its
  clients, not a family of independent commands.
- **The single-supervisor lock becomes real.** ADR 0012 decided that
  serialization, not rejection, satisfies Slice 2, and deferred an exclusive
  `flock` on `.factory/factory.lock` on the grounds that it is only meaningful
  once something long-lived exists to hold it. Something long-lived now exists:
  the daemon takes that lock for its lifetime, and a second daemon fails to
  start with an actionable error rather than quietly competing. Ordinary CLI
  mutations continue to serialize through SQLite as before.
- **The CLI talks to the daemon over the bundled local-IPC transport** of
  ADR 0002, not to SQLite directly. `factory doctor` (Slice 10) is the exception
  worth naming: it is read-only diagnosis and must still work when the daemon is
  *not* running, since "the daemon is down" is exactly the condition an operator
  runs it to diagnose. It therefore reads the database directly, read-only.
- **Slice 5 gains a prerequisite**: the adapter runs inside the daemon, and its
  `observe()` is a loop the daemon owns rather than a one-shot call. ADR 0011
  already sources observation from Irrlicht, which suits a resident poller.
- **The prototype's migration path is now defined.** `scripts/factory_tasks.py`
  is process-per-invocation; it remains the operational compatibility path until
  the daemon serves the same task tool, and the existing migration plan governs
  its retirement.
- Slices 1–4 are unaffected, and the already-committed crates need no change.
  This is the payoff for having parked the question rather than guessing.

## Open items created by this ADR

- **How the daemon is started and supervised on macOS.** `launchd` is the
  obvious answer and the prototype already uses it, but ADR 0012's restart-drill
  requirement applies here too: a daemon that has never been killed and observed
  to come back is not known to be restartable.
- **What happens to running sessions when the daemon stops.** They are harness
  processes in Herdr panes and do not die with it. Slice 9's reconciliation must
  therefore treat "daemon restarted" as a normal event with sessions to
  re-adopt, not as a cold start.
