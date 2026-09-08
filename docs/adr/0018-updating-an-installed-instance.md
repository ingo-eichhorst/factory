# ADR 0018: Updating an installed instance — when a migration runs, and what it must not do

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory

## Context

ADR 0012 decision 2 settles how a migration is *written*: forward-only,
append-only, schema state in SQLite's own `PRAGMA user_version` rather than in a
bookkeeping table, with different rules for an event store than for a
projection. That is the rule the author of a migration follows.

It is not a statement about what happens when an instance that is already
running is updated to a newer build. Nothing in the ADR record covers that.
Checked rather than recalled: `grep -rniE 'upgrade|downgrade|version skew'
docs/adr/` on 2026-09-08 returns four files, and every hit is about something
else — plugin upgrade rollback (0001), the Irrlicht event stream's protocol
version (0011, 0017), and a macOS system-library upgrade changing SQLite's
behaviour under Factory (0012, which is the argument for the `bundled` feature).

Four things follow from the current code and are undecided.

**A migration already runs on every open, and there is no other door.**
`Store::open_at` is the only way into the database, and it calls
`migrations::apply` unconditionally (`crates/factory-store/src/lib.rs:72`). So
an update does always migrate — but nobody decided that, and it is the *first
ordinary command* after an update that performs the schema change, not an
explicit upgrade step. It also makes one of ADR 0012's own stated consequences
unbuildable as written: `factory doctor` is to report "database integrity and
schema version" in a *read-only pass*, and today that pass would change the
schema before reporting it. The backup drill opens snapshots through the same
door, so inspecting an old snapshot silently migrates it.

**The reverse direction is handled by the library, not by a decision.** Measured
on 2026-09-08 against `rusqlite_migration` 2.6.0, with two migrations known to
the binary and a database already at `user_version = 4`:

```text
before: user_version = 4
to_latest: Err(MigrationDefinition(DatabaseTooFarAhead))
after:  user_version = 4
```

It refuses and leaves the file untouched. That is the right behaviour and no
Factory test asserts it, so a version bump of that crate could change it in
silence. (The probe used two synthetic migrations rather than Factory's own set;
it is evidence about the library, and the test this ADR requires must use the
real `migrations()`.)

**Nothing is backed up before a migration runs.** ADR 0012 decision 4 provides
`VACUUM INTO` and the restore drill, but nothing connects the two. Migration 2
rebuilds `scopes` with a drop/rename; migration 3 does the same to `tasks`. A
migration that fails part-way rolls back inside its own transaction, so that
case is covered. The uncovered case is a migration that *succeeds* and turns out
to be wrong, because append-only forbids a down-migration. The rollback today
is "restore the backup you never took."

**Only the database is covered.** `.factory/config.yaml` carries `version: 1`
and the build carries `SUPPORTED_VERSION`, and a mismatch is rejected with a
help line. There is no configuration migration and no obvious way to add one:
`factory-config` performs no filesystem writes at all — asserted byte for byte,
success and failure paths alike, by `crates/factory-config/tests/no_writes.rs` —
and the live file is hand-maintained prose and comments that AGENTS.md forbids
overwriting.

This ADR amends ADR 0012's consequence that `factory doctor` reads the schema
version in a read-only pass, by providing the door that makes it possible. It
changes none of ADR 0012's decisions.

## Decision 1: a migration runs on open, and a read-only door exists beside it

**Opening the database to *use* it migrates it, with no separate upgrade
command. Opening it to *look* at it does not.**

Implicit migration on open is kept. An explicit `factory upgrade` is the right
answer when many binaries share one database and an operator must schedule the
change; here there is one binary and one database on one machine, and a step a
human can forget produces exactly one new failure mode — a new build running
against an old schema — in exchange for nothing.

What is added is a second door, `Store::open_read_only`, which applies the
pragmas, does not migrate, and reports the schema version it found. Three
callers need it:

- `factory doctor` (Slice 10), which ADR 0012 already promised a read-only pass
  and cannot have one today;
- the backup drill's inspection steps, which today migrate the snapshot they are
  inspecting — an old snapshot is silently rewritten by the act of checking it;
- any future command whose contract is that it changes nothing.

The drill's fourth step — "the snapshot opens read-write and accepts a write" —
is the one place a snapshot may be migrated, and it comes after the
`user_version` comparison, so the comparison still sees the version the snapshot
was taken at.

## Decision 2: an update always migrates, a downgrade never does, and both are tested

`DatabaseTooFarAhead` is Factory's contract, not an accident of the dependency.
`factory-store` carries a test that seeds a database at a `user_version` above
the highest released migration, runs the real `migrations()` set against it, and
asserts two things: the call fails, and the file is unchanged. A future bump of
`rusqlite_migration` that changed this behaviour then breaks a Factory test
instead of quietly downgrading a database.

The operator-facing error names both numbers and the way out: this build
understands schema *N*, the database is at *M*, install the newer build or
restore a snapshot taken before the upgrade. A message that says only "database
too far ahead" tells an operator nothing they can act on.

## Decision 3: a snapshot is taken before any migration that changes an existing database

Before `migrations::apply` runs, and only when the database already has a schema
(`user_version > 0`) and at least one migration is pending, Factory takes a
`VACUUM INTO` snapshot to
`.factory/backups/pre-migration-<from>-to-<to>-<timestamp>.sqlite`.

A fresh database is exempt: there is nothing to lose, and a snapshot of an empty
file is noise in the directory an operator searches during an incident.

**If the snapshot cannot be written, the migration does not run.** This is the
whole point rather than a detail. Append-only migrations mean a released
migration has no inverse, so the snapshot is not a convenience — it is the only
rollback that exists. A migration that cannot be undone must not begin.

This is the first time Factory creates a backup that no one asked for, so the
bound matters: one snapshot per schema change, which is bounded by the number of
releases rather than by time. Factory still deletes nothing (ADR 0012
decision 4).

## Decision 4: a configuration version change is the human's edit, never Factory's

Factory refuses to run against a `version:` it does not support and reports what
must change. It does not rewrite `.factory/config.yaml`.

Three reasons, in increasing order of weight. `factory-config` writes nothing at
all today and a test asserts it byte for byte, so a rewrite would be a new
capability, not a new function. AGENTS.md forbids overwriting human-maintained
configuration without an explicit ownership marker, and the live file is full of
hand-written comments explaining decisions. And no round-trip YAML in this
workspace preserves comments, so a mechanical rewrite would silently delete the
reasoning a human put there — which is worse than refusing.

The error message therefore carries the whole migration: what to change, to
what, and where. A version bump that cannot be explained in an error message is
a bump that should not ship.

If a future configuration change is large enough to be painful by hand, the
escape hatch is a command that writes a *proposal* beside the original —
`config.yaml.v2-proposal`, inside `.factory/` and therefore permitted — which a
human reads, edits, and renames. Factory never edits the file in place.

## Consequences

- `factory-store` gains `open_read_only` and the `DatabaseTooFarAhead` test.
- Slice 10's `factory doctor` gets the read-only pass ADR 0012 promised it. Its
  report should name the schema version it found and whether migrations are
  pending, since after decision 1 a pending migration is a fact about the
  database rather than something doctor is entitled to fix.
- `crates/factory-store/tests/backup_drill.rs` moves its inspection steps to the
  read-only door.
- `.factory/backups/` starts filling on its own. ADR 0012's open item about
  retention is narrowed, not closed: the pre-migration set is bounded and
  predictably named, and what Factory may *notice* about that directory is
  settled in ADR 0019.
- A `version: 2` configuration cannot ship without an error message that
  explains the edit.

## Open item created by this ADR

Decision 3 places the snapshot before the migration but says nothing about the
snapshot of a database that is being migrated for the *second* time in one day
by an operator retrying a failed upgrade. The timestamped name makes collisions
impossible and `backup_to` already refuses an existing destination, so nothing
is unsafe; what is unspecified is whether a retry should reuse the first
snapshot. This is a question about operator ergonomics and should be answered
with an operator watching, not now.
