# ADR 0019: Restore — what a recovered database may claim, and what a backup actually covers

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory

## Context

Half of this subject is already decided, and this ADR does not reopen it.
ADR 0012 decision 4 settles the backup *mechanism* — `VACUUM INTO` rather than
a file copy or the incremental backup API, a destination that is refused if it
already exists, retention left to the operator, and the four-step drill built on
"a backup is not verified until it has been restored." That is implemented in
`crates/factory-store/src/backup.rs` and exercised by
`crates/factory-store/tests/backup_drill.rs`.

What is not decided anywhere is **what a restored database means**, and that is
what this ADR takes.

A snapshot describes a world that has moved on. Restored, it carries:

- `sessions` rows in `starting`, `running`, or `disconnected` — all
  lease-holding states per ADR 0012 decision 5 — naming panes that may not
  exist any more;
- `workspace_leases` rows with `released_at IS NULL`, held on behalf of those
  sessions;
- `tasks` rows in `running`, pointing through `assigned_session_id` at sessions
  in that same condition.

ADR 0012 decision 5 made `disconnected` hold its lease precisely so that Factory
never assumes a process is gone. Restore is the one moment where that
conservatism has to be turned inward rather than outward: every live-looking row
in the file is now of unknown provenance, because it describes the instant the
snapshot was taken and nothing since.

The second undecided thing is narrower and easier to miss. `VACUUM INTO`
snapshots *the database*. Measured on 2026-09-08, the instance's `.factory/`
holds `config.yaml`, `factory.sqlite`, `secrets.yaml`, `logs/`, and `evals`, and
design §4 adds `generated/`, `knowledge/`, and `memory/`. `git ls-files
.factory` returns exactly one path — `config.yaml` — and `.gitignore` excludes
the database, the logs, the secrets, and the knowledge graph by name. So a
command called `factory backup` protects one of those things and the operator
has no way to know that from its name.

This ADR settles both. It amends no ADR 0012 decision and closes ADR 0012's open
item about what Factory is entitled to notice under `.factory/backups/`.

## Decision 1: restoring is an explicit command, and opening a file is never a restore

`factory restore <snapshot>` puts a snapshot in place and performs the
reconciliation in decision 2, in one transaction, and writes a report. Merely
*opening* a database — including one an operator copied into place by hand —
changes no session and no task.

This follows the same rule ADR 0018 decision 1 sets for migrations: a command
whose job is to look must not rewrite what it is looking at, and the moment
Factory starts inferring "this file looks restored, I should clean it up" it can
be wrong about a live instance. There is no marker a snapshot can carry to
identify itself, because it is taken from a live database that has no way to
know it is being copied. So Factory does not guess.

The consequence is deliberate and must be stated rather than hidden: **an
operator who copies a snapshot into place by hand gets a database full of
stale live-looking rows and no warning.** The reconciliation is therefore
idempotent and separately invocable, so that operator reaches the same state by
running it afterwards. Running it twice changes nothing the second time.

## Decision 2: a restored database claims nothing about running processes

The reconciliation writes exactly this, and nothing else.

**Every lease-holding session becomes `disconnected`, and keeps its lease.**
`starting`, `running`, and `disconnected` all mean, after a restore, the same
thing: Factory cannot see the process. That is what `disconnected` means, so it
is the honest state. The leases stay held for ADR 0012 decision 5's reason,
which restore does not weaken but sharpens — a workspace whose occupant is
unknown must not be handed to a second harness. Failing the sessions instead
would release every lease at once, which is the one outcome the lease exists to
prevent.

This requires adding `Starting → Disconnected` to `factory_session`'s transition
table, which does not have it today. The justification is specific to this case:
a launch that was in progress when the snapshot was taken has three possible
outcomes and Factory can distinguish none of them, so the conservative reading
is the same as for `running`. The edge is not opened for any other caller.

> **Amended during slice 9 — the requirement stands, the mechanism changed.**
> Both sentences above cannot be satisfied by the transition table, because
> `factory_session::valid_targets` is global: an edge added there is open to
> every caller, and "not opened for any other caller" would survive only as a
> comment. Slice 9 therefore adds a dedicated entry point,
> `factory_session::reconcile_to_disconnected`, which performs exactly this
> move and refuses a session whose current state holds no lease. The general
> table is unchanged and `starting_cannot_go_directly_to_disconnected` still
> passes, so the restriction is enforced by the API rather than asserted in
> prose. This ADR's *requirements* — the conservative reading, and the edge
> being unavailable to anyone else — are met as written; only the sentence
> naming the transition table as the place to put it is superseded.

**Tasks are decided by the delivery journal, not by their status alone.**

| Row | Becomes | Why |
|---|---|---|
| `running` | `blocked: interrupted` | Design §5: Factory does not automatically resend a possibly delivered prompt. |
| `queued` with at least one `delivery_attempts` row | `blocked: interrupted` | The attempt is journalled *before* the write (design §5 step 3), so this task may already have been delivered. Its status is the weaker evidence. |
| `queued` with no delivery attempt | stays `queued`, `assigned_session_id` cleared | Nothing was ever sent. Design §5's recovery table keeps queued work queued; clearing the assignment lets selection choose again. |
| `blocked` | unchanged | Already waiting for a human. |
| `done`, `failed`, `cancelled` | unchanged | Terminal. |

The middle row is the one worth defending. A `queued` task that carries a
delivery attempt looks safe and is not, and the only reason Factory can tell the
difference is that §5 puts the journal write before the terminal write. This is
what that ordering was for.

The same rule already appears in migration 3, which rewrites a pre-existing
`running` task to `blocked: interrupted` while rebuilding `tasks` — a task
mid-flight during an upgrade is in exactly the position of a task mid-flight in
a snapshot. One rule, two triggers.

**The audit record is a report file, not a schema change.** `factory restore`
writes `.factory/restores/<timestamp>` — inside `.factory/`, so design §4's
allowlist holds — naming the snapshot, its `user_version`, and every session and
task it changed with before and after values. Adding a table for this would put
a second history beside `workspace_leases`; a report is enough because a restore
is an operator action with an operator reading its output.

## Decision 3: restore does not consult Herdr

The reconciliation above uses only the database. It does not ask Herdr which
panes exist, and it does not try to reattach anything.

A snapshot may be hours or days old. The pane identifiers it names may since
have been reused by entirely different sessions, so a pane that exists is not
evidence that *this* session exists. Establishing identity against a live
runtime needs the adapter's authoritative observations (ADR 0017) and the
restart-class analysis that Slice 9 owns.

The boundary is therefore: **this ADR says what the database may claim on its
own; Slice 9 says what live evidence is allowed to change that claim.** Both
produce `disconnected` sessions, and Slice 9's reconciliation is what can turn
one back into `running` by confirming the same session, or into `stopped` or
`failed` through the stale-lease recovery action ADR 0012 decision 5 already
made mandatory. This is the same split ADR 0012 used when it fixed the
lease-holding *set* and deferred the recovery *timing*.

## Decision 4: a backup covers the database, and the command says so

`factory backup` snapshots `factory.sqlite`. It is a database backup, not an
instance backup, and its output states what it covered and what it did not.
A command's name is a claim, and this one would otherwise be read as protecting
everything beside it.

The rest of `.factory/`, and where its recovery actually comes from:

- **`config.yaml`** — tracked in Git, and the only file under `.factory/` that
  is. Its recovery is Git's, which is better than a snapshot because it carries
  the history of the edits and the comments explaining them.
- **`generated/`** — regenerable by design §4, which requires an ownership
  marker and safe regeneration. Nothing to back up.
- **`secrets.yaml`** — deliberately excluded, and this is a decision rather than
  an omission. A backup of a secrets file is a second copy of the secrets with
  weaker handling than the original, in a directory whose retention Factory does
  not control.
- **`knowledge/` and `memory/`** — durable, not regenerable, not in Git, not in
  the database, and not covered by anything. Today `knowledge/` holds the
  seventeen curated wiki pages the backlog still lists as an open migration, and
  `memory/` does not exist yet. Whichever slice creates them decides their
  durability in the same change; until then Factory must not imply they are
  protected.

## Decision 5: Factory reports on its backup directory and never deletes from it

This closes ADR 0012's open item, which left deliberately unanswered whether
`factory doctor` may warn about disk usage under `.factory/backups/`.

Doctor reports the count, the total size, and the oldest and newest snapshot. It
deletes nothing, and neither does any other command. ADR 0018 makes Factory
create snapshots on its own for the first time, so an operator needs to be able
to see the directory grow — but ADR 0012's reason for refusing to delete stands
unchanged and gets stronger, not weaker, once some of those files are
pre-migration snapshots: a Factory that silently removes the only rollback an
append-only migration has is a worse failure than one that fills a disk visibly.

Report before repair, and here, report *instead of* repair. It is the same
stance ADR 0016 took for scope reconciliation.

## Consequences

- `factory-session` gains a `Starting → Disconnected` transition, justified only
  for restore.
- `factory-task` gains the restore reconciliation, which is the first consumer
  of `delivery_attempts` as evidence rather than as an audit trail.
- Slice 9 inherits a defined starting state: after a restore, every session is
  `disconnected` and every ambiguous task is `blocked: interrupted`, so its
  reconciliation begins from a known claim rather than from whatever the
  snapshot happened to hold.
- `factory backup` and `factory doctor` gain output obligations, which is
  cheaper to satisfy now than to retrofit once operators have learned to read
  the current output.
- The durability of `.factory/knowledge/` and `.factory/memory/` becomes an
  explicit precondition of the slice that creates them.

## Open item created by this ADR

Decision 2 tears down a restored world uniformly, which is right when the
snapshot is old and unnecessarily destructive when it is thirty seconds old and
the machine never stopped. Distinguishing those needs a durable notion of "has
this instance been running continuously since the snapshot," which Factory does
not have and which cannot be reconstructed from the snapshot itself. It is worth
revisiting only if restore turns out to be a routine operation rather than an
incident response; the conservative behaviour is correct for the incident case,
which is the one that matters.
