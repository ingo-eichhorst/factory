# ADR 0022: Agent discovery, and the two durable writing paths

- Status: Accepted
- Date: 2026-09-11
- Owners: Business Factory

## Context

Backlog §12 asks for `factory agent list`, `factory knowledge write|list|show`,
and `factory memory add|list`, and it names three things to settle before the
interface is written:

1. how the note body reaches the command;
2. whether `knowledge write` may update an existing note or only create one;
3. the note-name rules, since the filename is the link target and a rename
   breaks every `[[link]]` pointing at it.

Accepting station 11 surfaced three more that the backlog cannot state because
they only exist once stations 8, 10, and 11 are in the tree.

The first is an ownership question. §12's acceptance criteria require both
writing commands to "record the write as a task event". Task events live in
the store, and ADR 0014 makes the daemon the sole owner of store mutations. So
these commands cannot be CLI-local file writes with a event recorded beside
them; the file and the event are one act, and one process has to own both.

The second is that the `.factory/` write invariant is to be "asserted once
against every mutating command, not per command, so a command added later
cannot quietly escape it". A test that lists the commands by hand satisfies the
words and fails the purpose: the command added later is exactly the one nobody
adds to the list.

The third is `agent list`'s caller. It marks which scopes "the calling scope
may target", and Factory has no caller identity — ADR 0021 decision 5 refused a
self-declared `--author-session` on `task verify` for precisely that reason.

## Decision

### 1. `knowledge write` and `memory add` are daemon operations

Both become operations in the decision 9 table — `knowledge.write` and
`memory.add` in the writing group, `knowledge.list`, `knowledge.show`,
`memory.list`, and `agent.list` in the reading group. The CLI is a client of
them, as it is of every other operation.

The alternative was to let the CLI write the file and ask the daemon to record
the event. That splits one act across two processes: a crash between them
leaves a note nobody can trace, or an event naming a note that was never
written. It also gives Factory a second writer of durable state, which is the
thing ADR 0014 exists to prevent.

### 2. The write is recorded in its own table, not in `task_events`

Migration 7 adds `durable_writes`: what was written (`knowledge` or `memory`),
its name, the path relative to the instance root, the scope for a memory entry,
a nullable `task_id`, a nullable `author_session_id`, and a timestamp. It is
additive — a table that starts empty, the class migrations 4 and 5 belong to,
not the drop-and-rename of migrations 2 and 3.

`task_events` was the obvious home and is the wrong one, for two reasons its
own schema states. Its `task_id` is `NOT NULL REFERENCES tasks (id)`, because
it logs the transitions *of a particular task*; a note written by a human at a
terminal is not a transition of anything, so the criterion as literally worded
would be unsatisfiable for that caller rather than merely inconvenient. And
design §11 projects `task_events` into throughput and scrap figures (§12.5) — a
knowledge write counted as a task event is a unit of production that nobody
produced.

Neither the backlog nor design §7 asks for the table by name. Both say the
write is recorded "as a task event so the provenance survives the session", and
it is the second half that states the purpose. `durable_writes` serves it with
a task id when a task is named and without one when none is, and widens no
CHECK on a log that is never edited.

### 3. The row is committed after the file is in place, never before

The daemon writes the note to a temporary file in the destination directory,
flushes it, opens the store transaction that inserts the `durable_writes` row,
renames the temporary file into place, and only then commits.

The ordering is chosen by which failure is survivable. With the commit last, a
crash leaves a note with no provenance row: an observation that is missing.
With the commit first, a crash would leave a row asserting a note that does not
exist: an observation that is wrong. ADR 0017 settles which of those is worse —
"a wrong observation is worse than none, because it looks like an answer" — and
`knowledge list` derives everything it reports from the files themselves, so a
missing row costs provenance and nothing else.

### 4. The note body arrives on standard input

`factory knowledge write` reads the body from standard input; the frontmatter
fields arrive as flags (`--name`, `--title`, `--status`, `--source`, repeated).
`memory add` takes its entry the same way.

Prose does not belong in argv: it contains newlines, quotes, and backticks, and
every shell mangles a different subset. Standard input has none of those
problems and is what an agent already uses to pipe a generated note. There is
deliberately no `--body` flag as a second path, because two paths into the same
field drift apart and each one grows its own escaping bug.

An empty body is refused. A note whose frontmatter validates and whose body is
empty is a filename with sources attached, and it would be indistinguishable
from a failed pipe.

### 5. An update is allowed, but only when it is asked for by name

`knowledge write` creates a note. Writing a name that already exists is
refused unless `--update` is given, and the refusal says the note exists and
names the flag.

Whether a new note contradicts an existing one is a judgement the CLI cannot
make: it would have to read both and decide which is right. That judgement
stays with the agent. What the command verifies is mechanical and complete —
the frontmatter carries a title, a status, an updated date, and at least one
source; no source path lies under `data/secrets/`; the write is atomic. What it
trusts is the content. Requiring `--update` makes the overwrite of somebody
else's note a thing the caller asked for rather than a thing it did by reusing
a name.

### 6. A note name is a stable identifier, and version 1 cannot rename one

`--name` is given explicitly and never derived from the title, because a title
is edited freely and a name is a link target. A name is one to sixty-four
characters of lowercase ASCII letters, digits, and hyphens, with no leading or
trailing hyphen and no consecutive hyphens. Anything else is refused, naming
the rule.

There is no `factory knowledge rename`, and its absence is a decision. Renaming
a note means rewriting every `[[link]]` that points at it, across every other
note, atomically. That is a graph rewrite, and §8 defers the graph machinery
that would make it safe. A caller who needs a different name writes the new
note and leaves the old one pointing at it.

### 7. `agent list` names its caller by session, and a scope only for a
hypothetical

`factory agent list` takes exactly one of `--session` or `--scope`. With
`--session` it resolves the caller's scope the way `task send` already does,
through `factory_session::scope_of_session`, and the answer is the one
enforcement will give. With `--scope` it answers a different, openly
hypothetical question — what *would* that scope be allowed to target — which is
what a human inspecting the tree wants.

ADR 0021 decision 5 refused a self-declared author on `task verify`, and
`--scope` looks like the same flag. The difference is what the answer is used
for. `task verify`'s guard is enforcement: a self-declared author lets an agent
inspect its own scope by naming someone else, and a guard that can be talked
out of its answer is worse than none, because it reads like one. `agent list`
enforces nothing. §6 is enforced where it always was — in
`factory_delegation::queue_from_session`, which takes a *session* and reads that
session's scope out of the database rather than believing a scope in the
payload. A caller that lies to `agent list` gets a list of scopes `task send`
will refuse it. The lie costs a round trip and buys nothing.

`--session` exists so the two answers cannot drift: the advisory listing and
the enforced refusal resolve the caller through the same function.

### 8. The rule is reused, and the mutation is run at the new call site

`agent list` calls `factory_delegation::rule::check`. It does not reimplement
§6, and it does not read kinship and decide for itself.

Station 11 established that a mutation on a shared predicate must be run at
every call site rather than once for the predicate. `check` already has tests
in `factory-delegation` and in `task send`'s path; neither says anything about
whether `agent list` calls it, or calls it with the right sender. So the
deleting mutation is run against `agent list` as its own case.

### 9. Availability answers "can I send work here now", and never guesses

An agent registered with no live session is listed with its availability
stated, never omitted. Where ADR 0011's session observation has no reading, the
entry says `unknown` — it does not say `idle`. An agent that is busy but
reported idle is the failure this prevents: a caller queues work behind a
session that will not free up, and the queue looks healthy while nothing moves.
ADR 0017's rule applies unchanged.

One agent may hold several sessions at once (`max_sessions`), so the single
word has to be ranked. The ranking follows from the question the word answers —
can a caller send work here now:

1. any `running` session with no running task → `idle`
2. else any `starting` session → `starting`
3. else any `running` session with a running task → `busy`
4. else any `disconnected` session → `unknown`
5. else → `no_session`

`idle` outranks `busy` because an agent holding one working session and one free
one is not a queue: the free session takes the work immediately. Ranking `busy`
first was tried and is the mirror of the failure design §7 names — it sends a
caller away from an agent that was available. `unknown` sits below `busy`
because a known fact beats an unknown one, and above `no_session` because a
session that was seen and then lost is not the same as no session at all.

### 10. The write invariant is discovered from the parser, not from a list

The test that asserts no command writes outside a `.factory/` directory
enumerates the command surface by walking `clap`'s own command tree from
`Cli::command()`, recursively, to every leaf. Each leaf must appear in the
test's table with either an invocation that is run against a scratch instance
root, or a recorded reason why it cannot be invoked there. A leaf in neither
fails the test.

The point is which way the test fails. A hand-written list of commands passes
happily on the day someone adds the fifteenth command, because the list does not
know about it. Walking the parser means the new command is discovered the moment
it parses, and the test fails until somebody says what it writes.

### 11. Both modules live in one crate, and the separation stays at the surface

`factory-knowledge` holds the note graph and scope memory as two modules. The
two commands, the two directories, and the two kinds of material stay as
separate as design §7 describes them.

The separation design §7 asks for is of the material — shared and sourced
against scope-local and unsourced — not of the code that makes a file appear on
disk without a half-written state. That primitive is the same for both, and
written twice it would be correct in one place and nearly correct in the other.

### 12. A memory directory is named by scope, and an ambiguous name is refused

`.factory/memory/<scope-name>/`, exactly as design §4's tree draws it —
`memory/business-factory/`, `memory/finance/`, `memory/irrlicht/`. Not the
scope's UUID.

`scopes.name` carries no UNIQUE constraint; only `canonical_path` does. So two
registered scopes can share a name, and the readable directory the design asks
for would then hold two scopes' memory merged into one pile, with no way to
tell afterwards which entry belonged to which. `memory add` and `memory list`
therefore refuse when the name they resolve matches more than one registered
scope, and say which scopes collided.

The alternative was to key the directory by scope id, which cannot collide. It
was rejected because it makes every path unreadable to the person who has to
look in it, and design §4 chose the readable form deliberately. A refusal on a
collision keeps the readable form and costs only a case that is a
misconfiguration anyway: two scopes with one name is something the operator
wants to know about, not something to route around silently. The refusal is the
observation; merging them would be the wrong answer that looks like one
(ADR 0017).

### 13. Factory stamps the `updated` date; the caller does not pass it

`knowledge write` takes `--name`, `--title`, `--status`, and `--source`
(repeatable), and the body on standard input. It does not take the updated
date. The daemon stamps it at the moment of the write.

The field exists so a reader can tell how stale a note is. A date the writer
types is a claim about that, and a note carrying a date its author chose is
worse than one carrying none, because a reader has no way to tell the two
apart. A date Factory stamps is a fact about when the bytes were written, which
is the question the field is asked.

`Frontmatter::updated` stays a required field in `factory-knowledge`, and
`Note::new` still refuses an empty one. The crate validates notes from any
source, including ones read back off disk and ones a later migration produces;
only this command's caller is relieved of supplying it.

## Consequences

`.factory/knowledge/` is gitignored at the business-factory root, and
`.factory/memory/` is too, so neither is recoverable from Git after a careless
write. This is the reason `--update` is explicit rather than implied.

The seventeen curated pages in `knowledge/wiki/` and the `MEMORY.md` files at
scope roots are not migrated by this station, as §12 says. They need their own
task with a rollback plan, in the way the task store had one.

`knowledge show` prints one note as it is on disk. It is not a rendering
surface, and it resolves no links.
