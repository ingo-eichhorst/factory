# ADR 0012: Slice-2 prerequisites — SQLite driver, migrations, locking, backup, and lease-holding states

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory

## Context

Slice 2 of the implementation backlog lists four unresolved decisions: migration
tooling, lock strategy, backup retention and restore verification, and which
session states count as a live workspace lease. All four block the first table,
so this ADR resolves them together, in the manner of ADR 0009.

It implements `.specs/design.md` §4 (one operational database at the company
root, transactions, one mutator, idempotent mutations) and §2.3 (session states
and the one-live-lease-per-workspace rule). It amends no baseline section.

Crate figures were measured from the crates.io API on 2026-09-08 rather than
recalled.

## Decision 1: `rusqlite` with the `bundled` feature, not `sqlx`

| Crate | Version | Last release | Downloads (90d) |
|---|---|---|---:|
| `sqlx` | 0.9.0 | 2026-05-21 | 35.5M |
| `rusqlite` | 0.40.2 | 2026-08-08 | 32.8M |

Both are healthy; popularity does not decide this. Three properties do.

**Factory's database work is synchronous.** SQLite is a library call against a
local file, not a network service. `sqlx` is an async API, so using it here means
introducing an async runtime to await work that a blocking thread pool performs
anyway. ADR 0008 chose Rust partly for a small dependency surface; a runtime
that buys nothing contradicts that.

**Transaction mode must be explicit.** The design requires `BEGIN IMMEDIATE`
for mutations, so that a writer takes its lock at the start of the transaction
rather than discovering a conflict at first write and failing after it has
already read. `rusqlite` exposes this directly as
`Connection::transaction_with_behavior(TransactionBehavior::Immediate)`.

**`bundled` removes a hidden platform dependency.** It compiles SQLite into the
binary instead of linking whatever macOS ships. Without it, `VACUUM INTO`
(decision 4) and the exact locking behaviour depend on the host's system
library, and a machine upgrade could change the database's behaviour without a
line of Factory changing. Pinning the engine version is worth the compile time.

## Decision 2: `rusqlite_migration`, and a versioned schema

`rusqlite_migration` 2.6.0 (2026-05-28, 2.1M downloads in 90 days) over
`refinery` 0.9.2. Both are maintained; `rusqlite_migration` is chosen because it
is built on `rusqlite` rather than abstracting over several backends Factory
will never have, and because it tracks schema state in SQLite's own
`PRAGMA user_version` rather than in a table of its own.

That last point is not cosmetic. Slice 10 requires `factory doctor` to report
"database integrity and schema version" in a read-only pass. `user_version` is a
single integer readable from any SQLite client, including `sqlite3` on the
command line during an incident, with no Factory code and no knowledge of a
bookkeeping table's layout.

Migrations are forward-only and append-only: a released migration is never
edited. This matters more here than in an ordinary application because of
ADR 0003. The append-only event store is canonical and must survive every
migration; read tables, indexes, and snapshots are CQRS projections and may be
dropped and rebuilt by replay. So the two halves of the schema get different
rules:

- a migration touching the **event store** may only add, never rewrite history;
- a migration touching a **projection** may drop and rebuild it, because replay
  reconstructs it — and replay must never invoke plugins or repeat external
  side effects.

## Decision 3: locking — WAL, `busy_timeout`, and `BEGIN IMMEDIATE`

Every connection sets, on open:

```sql
PRAGMA journal_mode = WAL;      -- readers do not block the writer
PRAGMA foreign_keys = ON;       -- off by default in SQLite, per connection
PRAGMA busy_timeout = 5000;     -- wait rather than fail instantly on contention
PRAGMA synchronous = FULL;      -- durability over speed; this database is small
```

`synchronous = FULL` rather than WAL's usual `NORMAL`: with `NORMAL`, a power
loss can lose the most recent committed transactions. Slice 7 commits a task as
`queued` *before* a prompt is entered into a terminal, precisely so that a crash
cannot lose the record of work that may already have had effect. `NORMAL` would
undermine the guarantee the commit ordering exists to provide. This database
handles a handful of transactions per minute, so the cost is irrelevant.

WAL is safe here because the database is on local APFS. It is not safe on a
network filesystem; if `.factory/` is ever placed on one, this decision must be
revisited, and `factory doctor` should report the filesystem type.

**Slice 2's criterion "a second supervisor/mutator is rejected or serialized" is
met by serialization, not rejection.** `BEGIN IMMEDIATE` plus `busy_timeout`
serializes concurrent mutators correctly and is the same answer whether Factory
runs as a long-lived daemon or as a process per invocation.

That is worth stating plainly, because the daemon-versus-process question is
still open and deliberately parked until Slice 5. It does not reach into
Slice 2. A single-supervisor *rejection* — an exclusive `flock` on
`.factory/factory.lock`, held for the process lifetime — is only meaningful once
something long-lived exists to hold it, so it belongs with the daemon decision.
The backlog's claim that slices 1–4 are identical under either answer survives
inspection here.

### Implementation note: create the database file before SQLite opens it

Slice 2 requires restrictive permissions on the database. It is not enough to
`chmod` after opening, because under WAL, SQLite creates `-wal` and `-shm`
sidecars and derives their mode from the database file's mode *at the moment it
creates them*. Measured on 2026-09-08:

```text
create file 0600, then open      ->  factory.sqlite      rw-------
                                     factory.sqlite-shm  rw-------
                                     factory.sqlite-wal  rw-------

open, then chmod 600              ->  factory.sqlite      rw-------
                                     factory.sqlite-shm  rw-r--r--   <-- leaked
                                     factory.sqlite-wal  rw-------
```

The `-shm` file inherits the umask default and stays world-readable, beside a
database that looks correctly locked down. So `Store::open_at` creates the file
with mode `0600` *before* `Connection::open`, and the permission test asserts
the mode of all three files rather than only the database.

## Decision 4: backup by `VACUUM INTO`, verified by restore

```sql
VACUUM INTO '<destination>';
```

A single statement producing a consistent, defragmented snapshot of a live
database without stopping writers, available since SQLite 3.27 and therefore
guaranteed by decision 1's `bundled` engine. It is preferred over copying the
file (which is wrong while WAL is active) and over the incremental Online Backup
API (which solves a problem — very large databases, restart-on-write — that
Factory does not have).

**Retention:** the operator's business, not Factory's. Version 1 provides the
command and writes the snapshot into `.factory/backups/`, satisfying design §4's
rule that Factory writes only inside `.factory/`. It does not schedule backups
and does not delete old ones; a Factory that silently deletes its own backups is
a worse failure than one that fills a disk visibly.

**A backup is not verified until it has been restored.** The verification is a
drill, not a checksum:

1. `PRAGMA integrity_check` on the snapshot returns `ok`;
2. its `user_version` equals the source's;
3. row counts for scopes, sessions, leases, and tasks match the source;
4. the snapshot opens read-write in a scratch directory and accepts a write.

Step 4 exists because a snapshot that opens read-only and passes an integrity
check can still be unusable as a working database. A backup procedure whose
verification never opens the result for writing is a procedure that has not been
tested.

## Decision 5: which session states hold a workspace lease

Design §2.3 gives the states:

```text
stopped | starting | running | disconnected | failed
```

A lease is held by **`starting`, `running`, and `disconnected`**, and released
by `stopped` and `failed`. Each follows from a Slice 6 criterion rather than
taste:

- **`starting` holds.** Slice 6 requires `starting` to be recorded *before*
  launch. If the lease were taken only on `running`, two concurrent starts would
  both pass the check and race for one workspace — which is the exact defect the
  lease exists to prevent.
- **`running` holds.** The trivial case.
- **`disconnected` holds.** This is the one that could plausibly go the other
  way, and the conservative answer is right. `disconnected` means Factory has
  lost sight of the session, not that the process is gone. Releasing the lease
  would let a second harness start in a directory where the first may still be
  writing files. Design §5 already takes this stance for delivery — after an
  ambiguous failure it declines to resend rather than risk a repeat — and the
  same reasoning applies to workspaces. A stale lease is a nuisance an operator
  clears; two live harnesses in one directory is corruption.
- **`failed` releases.** Slice 6 requires a failed start to leave no leaked
  lease.
- **`stopped` releases.** By definition.

Because `disconnected` holds its lease, Slice 9's reconciliation needs an
explicit operator path to clear a lease whose session cannot be recovered. That
is a *stale-lease recovery* action, listed in Slice 6's open risks, and this
decision makes it mandatory rather than optional: without it, a `disconnected`
session that never returns would hold its workspace permanently.

The lease uniqueness constraint indexes the canonical path, but every runtime
aliasing check compares `(st_dev, st_ino)` per ADR 0009 §3a. A case-variant path
must not be able to take a second lease on one directory.

## Consequences

- Slice 2 can begin; the remaining work is implementation, not decision.
- The workspace gains `rusqlite` (with `bundled`) and `rusqlite_migration`.
  Compile time rises because SQLite is built from source; this is accepted.
- `factory doctor` (Slice 10) reads `PRAGMA user_version` and
  `PRAGMA integrity_check`, both of which exist because of decisions 2 and 4.
- Slice 6 inherits a defined lease-holding set, and Slice 9 inherits a mandatory
  stale-lease recovery action.
- The daemon-versus-process decision remains outside slices 1–4, now checked
  rather than assumed. It was settled shortly afterwards by ADR 0014 — a
  long-running daemon owns all mutations — which makes the deferred
  single-supervisor `flock` real: the daemon holds it for its lifetime, while
  ordinary CLI mutations keep serializing through SQLite as decided here.

## Open item created by this ADR

Backup retention is deliberately unspecified. If `factory doctor` is ever asked
to warn about disk usage under `.factory/backups/`, that is a new decision about
what Factory is entitled to notice, and it should be taken explicitly rather than
folded into a diagnostic as an afterthought.
