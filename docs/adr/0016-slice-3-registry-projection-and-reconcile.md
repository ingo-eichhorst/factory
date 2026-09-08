# ADR 0016: Slice-3 prerequisites — the registry is a projection, and reconcile reports before it repairs

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory

## Context

Slice 3 turns registered scopes into records the rest of Factory can join
against. ADR 0015 changed what it starts from: scopes are now declared in the
instance's `.factory/config.yaml` rather than discovered as files.

That creates a question the old design did not have to answer. There are three
descriptions of where a scope is, and they can disagree:

1. **The instance configuration** — what a human declared.
2. **The SQLite `scopes` table** — what Factory recorded, and what sessions,
   leases, and tasks hold foreign keys into.
3. **The filesystem** — what is actually there.

Design §4 says the database "is canonical for registered paths" and the instance
configuration "is canonical for scope configuration". With a central registry
the path is *part of* the configuration, so those two sentences now overlap.
This ADR resolves the overlap and settles Slice 3's remaining open item, the
operator confirmation policy for a moved scope.

## Decision 1: the configuration declares, the table projects

**The instance configuration is the only source a human edits. The `scopes`
table is a projection of it, joined with what the filesystem says.**

This follows ADR 0003, which already requires that mutable read tables be
rebuildable projections rather than independent authorities. Concretely:

- Deleting the `scopes` table and re-running the projection reproduces it, given
  the same configuration and filesystem. A test asserts this, because a
  projection that cannot be rebuilt is a second source of truth wearing a
  disguise.
- The table adds what the configuration cannot hold: the **canonical absolute
  path** (the configuration stores a relative path as written, per Slice 1), the
  `(st_dev, st_ino)` identity from ADR 0009, and the resolved parent scope.
- Factory never writes a scope back into the configuration file as a side
  effect. `factory scope add` appends an entry because a human asked it to;
  nothing else edits that file. This preserves ADR 0009 rule 6.

The design §4 sentence is refined rather than contradicted: the database remains
canonical for *runtime* facts — which path a lease is held against, which scope
a task targets — while the configuration is canonical for *membership*. A scope
exists because it is declared, not because a row exists.

## Decision 2: reconcile reports; applying is a separate, explicit act

`factory scope reconcile` is read-only and exits non-zero when it finds drift.
`factory scope reconcile --apply` performs the projection.

Slice 10 already establishes this split for `factory doctor` — "it reports and
never repairs: recovery keeps the explicit review, resume, and replacement
actions of slice 9, so there are not two ways to change the same state." The
same reasoning applies a slice earlier. A reconcile that silently repaired would
be the second way to change scope registration, competing with `scope add`.

Four kinds of drift are distinguished, because they have different remedies:

| Drift | Meaning | `--apply` |
|---|---|---|
| **Declared, not projected** | A new entry in the configuration | Applies |
| **Projected, not declared** | An entry a human removed | Reports only |
| **Path changed, same identity** | The directory moved; `(dev, ino)` matches the recorded value | Applies |
| **Path changed, different identity** | Recorded inode is gone or belongs to another directory | Reports only |

The last two are the whole point of the table. A moved directory keeps its
inode, so a rename is recognisable as a move rather than guessed at from the
name. But `(dev, ino)` is *not* durable identity — ADR 0009 is explicit that
delete-and-recreate at the same path yields a new inode — so a mismatch means
"this may be a different directory", not "this is". Applying automatically there
would silently re-point every session, lease, and task that references the scope
at whatever now occupies the path.

**Removing a scope is never automatic.** A scope deleted from the configuration
still has rows referencing it. `--apply` reports it and stops; retiring a scope
is its own command, in a later slice, because it has to decide what happens to
that scope's history.

## Decision 3: registration requires a readable `AGENTS.md`

ADR 0013 made Slice 4 fail hard on a context source it cannot read. That failure
would otherwise surface when an agent starts — the worst moment, since the
operator is then waiting on a session rather than editing configuration. Slice 3
checks it at registration and at reconcile.

The check is *readable*, not *non-empty*. An empty `AGENTS.md` is a legitimate
statement that a scope adds nothing to its ancestors' context, and Slice 4
compiles it deterministically.

## Decision 4: `git` is recorded, not verified

Version 1 stores the `git` reference from ADR 0015 and does nothing with it. It
does not contact a remote, parse the URL, or check that the checkout at `path`
has that origin.

The temptation is to verify, and it should be resisted for now: a check that
runs during registration would make `scope add` require network access and fail
on an offline machine, for a field nothing yet consumes. When something does
consume it, `factory doctor` is where the comparison belongs — it is the
read-only command whose job is exactly to report what disagrees with reality.

## Consequences

- Slice 3 delivers a `factory-registry` crate over `factory-config`,
  `factory-paths`, and `factory-store`, plus the projection and drift report.
- The `scopes` table gains `canonical_path`, `dev`, `ino`, and `parent_id`
  alongside the declared values. `dev` and `ino` are nullable: a scope whose
  path does not currently exist is a reportable state, not an unrepresentable
  one.
- Slice 4 receives canonical paths from this projection, which is the guarantee
  its byte-stability depends on.
- A rebuild test is mandatory, not optional. It is what keeps decision 1 true as
  the table grows columns.

## Open item created by this ADR

Retiring a scope — what happens to the sessions, leases, tasks, and memory of a
scope removed from the configuration — is deliberately unanswered. It needs a
decision about whether history is preserved, archived, or deleted, and that is a
data-retention question rather than a registry one.
