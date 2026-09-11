# Slice 12 — operator guide

Station 12 gives an agent two things it could not do before: find out who
else exists and who it may hand work to, and write something down that
outlives its own session.

Read `docs/slice-10-operator-guide.md` first if you have not. Everything it
says about the daemon, the socket, exit codes, and registering a scope by
hand is still true and is not repeated here.

The decisions behind this station are in
`docs/adr/0022-agent-discovery-and-durable-writing.md`. Where this guide says
"because", that document says why at length.

## What this station delivers, and what it does not

### Delivers

- **`factory agent list`.** Every registered scope, its agents, which of them
  the caller may target, and whether each is free right now.
- **`factory knowledge write|list|show`.** A shared note graph in the company
  root's `.factory/knowledge/`, one Markdown file per note, linked with
  `[[note-name]]`.
- **`factory memory add|list`.** Scope-local entries under
  `.factory/memory/<scope>/`.
- **A provenance log.** Every write of either kind puts one row in
  `durable_writes`, so who wrote what survives the session that wrote it.
- **One test that holds the whole CLI to design §4's rule** — no command
  writes outside a `.factory/` directory — and that fails when a command is
  added without saying what it writes.

### Does not

- **No rename.** There is no `factory knowledge rename`. The filename is the
  link target, so renaming means rewriting every `[[link]]` pointing at it,
  across every note, atomically. That is a graph rewrite and §8 defers the
  machinery that would make it safe. Write the new note and leave the old one
  for whatever still points at it.
- **No index service, embedding, or retrieval engine.** `knowledge list`
  computes the index by rereading the files. §8's deferral of knowledge
  databases is untouched.
- **No migration of what exists.** The seventeen curated pages in
  `knowledge/wiki/` and the `MEMORY.md` files at scope roots are still where
  they were. Both are gitignored, so a careless move is not recoverable from
  Git; moving them needs its own task with a rollback plan.
- **No custom frontmatter.** A note carries a title, a status, an updated
  date, and sources. An unrecognised key is refused. Whether `knowledge write`
  should ever accept more is an open question, not a decided one.
- **No caller identity.** `agent list --scope` is self-declared and advisory.
  See "Why `--session` exists" below.

## The command surface

```text
factory agent list   (--session <uuid> | --scope <name-or-uuid>)

factory knowledge write --name <name> --title <title> --status <status>
                        --source <path> [--source <path>…]
                        [--update] [--task-id <uuid>]      # body on stdin
factory knowledge list
factory knowledge show --name <name>

factory memory add  --scope <name-or-uuid>                 # entry on stdin
factory memory list --scope <name-or-uuid>
```

Both writing commands read their text from **standard input**. There is no
`--body` flag and there will not be one: prose in argv is mangled by every
shell's quoting rules differently, and a second path into the same field
grows its own escaping bug.

## Procedure 1 — find out who you can work with

```bash
factory agent list --scope alpha
```

```text
alpha      targetable=False  the calling scope itself
alpha-sub  targetable=True
beta       targetable=False  unrelated to the calling scope — a nephew or
                             cousin, reached through its parent
```

Each scope lists its agents with harness, lifetime, `max_sessions`, and
availability.

### What `targetable` means, and who decides it

It is decided by `factory_delegation::rule::check` — the same function
`factory task send` enforces design §6 with. `agent list` does not read
kinship and decide for itself, so the rule cannot drift between the listing
and the sending. When a scope is refused, the reason says which of §6's cases
applies, in words rather than in ids.

### Why `--session` exists

`--session` names the caller by the session actually asking. The scope is
then read out of the session's own row, exactly as `task send` reads it. Use
this form from inside an agent: the answer is the one enforcement will give.

`--scope` answers the same question hypothetically — what *would* that scope
be allowed to target. Use it as a human inspecting the tree.

The difference matters because `--scope` is self-declared and nothing checks
it. That is safe only because `agent list` enforces nothing: a caller that
lies about its scope receives a list of scopes `task send` will refuse it.
The lie costs a round trip and buys nothing. Do not add a self-declared
caller to a command that *does* enforce something — ADR 0021 decision 5
refused exactly that for `task verify`.

### What availability means

| Word | What was seen |
|---|---|
| `idle` | a running session with no task on it |
| `starting` | a session launching, not yet confirmed |
| `busy` | a running session working a task |
| `unknown` | a session seen and then lost — the observer has no current reading |
| `no_session` | no live session at all |

An agent may hold several sessions, so the single word is ranked in that
order. `idle` beats `busy`: an agent with one working session and one free one
is not a queue, and reporting it busy would send you away from work that could
start immediately.

`unknown` is never reported as `idle`. A caller told "idle" about a session
nobody can see queues work behind something that may never free up, and the
queue looks healthy while nothing moves.

An agent that is registered but has no session is **listed** with
`no_session`, never omitted. You cannot act on an agent you were not told
about.

## Procedure 2 — write a note

```bash
printf 'Alpha ships on Fridays. See [[release-process]] for the steps.\n' \
  | factory knowledge write \
      --name alpha-cadence \
      --title "Alpha release cadence" \
      --status draft \
      --source "docs/alpha/plan.md"
```

The file that lands:

```text
---
title: Alpha release cadence
status: draft
updated: 2026-09-11T09:54:26+00:00
sources:
  - docs/alpha/plan.md
---

Alpha ships on Fridays. See [[release-process]] for the steps.
```

`--name` is the stable identifier and the link target. It is one to
sixty-four characters of lowercase letters, digits and hyphens, and it is
never derived from `--title`: a title is edited freely, a name is what other
notes point at.

You do not pass the updated date. Factory stamps it. The field exists so a
reader can tell how stale the note is, and a date the writer typed is a claim
about that which a reader cannot tell apart from a true one.

`--task-id` is optional. Give it and the write is recorded against that task;
leave it out and the note is still written, with a provenance row that names
no task. A human at a terminal has no task, and a record that could not hold
that case would be satisfied only for the callers easiest to satisfy.

### Writing over a note that exists

Refused, unless you pass `--update`:

```text
factory: conflict.note_already_exists: note alpha-cadence already exists
  help: pass --update to overwrite it
```

Whether your new note contradicts the old one is a judgement Factory cannot
make — it would have to read both and decide which is right. That judgement
stays with you. What Factory checks is mechanical: the frontmatter is
complete, no source points into the secrets tree, and the write cannot land
half-finished. What it trusts is the content.

`.factory/knowledge/` is gitignored, so an overwrite is not recoverable from
Git. That is why `--update` is a flag you type rather than something implied
by reusing a name.

## Procedure 3 — read the graph

```bash
factory knowledge list
```

```text
alpha-cadence    Alpha release cadence  links=[release-process]  backlinks=[release-process]
release-process  Release process        links=[alpha-cadence]    backlinks=[alpha-cadence]
unresolved: []
unreadable: [hand-edited.md: note text is not in the expected frontmatter
             format: missing opening `---` line]
```

Four things are reported and all four matter.

**Links are canonical; the graph is derived.** The `[[…]]` references live in
the files. Backlinks and the index are computed by rereading the notes every
time. There is no link table to keep in step, and nothing writes an index to
disk. Delete `.factory/knowledge/`, restore the note files, and you get the
same index back.

**An unresolved link is not an error.** It is a note somebody meant to write.
An index that refused on a dangling link would refuse to show anything the
moment one author's note outran another's, which is the ordinary state of a
wiki being written.

**An unreadable file is one file's problem.** A `*.md` that will not parse is
reported by name with the reason, and every other note, backlink and
unresolved link still comes back. One hand-edited file must not take out the
other ninety-nine.

**A file that is not a note is skipped silently** — no `.md` extension, or a
stem that is not a valid note name. A `.gitkeep`, or a temporary file orphaned
by a crash, is not a corrupt note and is not reported as one.

`factory knowledge show --name alpha-cadence` prints the note exactly as it is
on disk. It resolves no links and renders nothing, so its output pipes
straight back into `knowledge write --update`.

## Procedure 4 — scope memory

```bash
printf 'Alpha prefers small PRs. Decided 2026-09-11.\n' \
  | factory memory add --scope alpha

factory memory list --scope alpha
```

Entries land in `.factory/memory/<scope-name>/`, one file per entry, named
`20260911T095426Z--<uuid>.md`.

**Why that filename shape.** The timestamp is ISO 8601 basic format, with no
colons and no separators. A colon in a path makes `rsync` read everything
before it as a remote host name, is illegal in a Windows filename, and shows
as `/` in the macOS Finder. These files are meant to be copied around with
ordinary tools, so the name has to survive them. Do not "tidy" it back to
RFC 3339. The entry's own timestamp, which `memory list` reports, is
readable and separate.

An existing entry is never modified. Two entries written in the same second
share a timestamp prefix and stay distinct, because the id breaks the tie.

### A scope name that matches two scopes is refused

```text
factory: conflict.ambiguous_scope_name: scope name `alpha` is registered for
2 scopes (…0001, …0004); a memory directory named by scope would merge their
entries into one unrecoverable pile
  help: rename one of the colliding scopes in `.factory/config.yaml` so each
  has a unique name
```

`scopes.name` carries no uniqueness constraint — only `canonical_path` does —
so two registered scopes really can share a name. Naming the directory by the
scope's id instead would remove the collision and the readability together,
and design §4 draws this tree with names on purpose. Two scopes with one name
is a misconfiguration you want to hear about, not something to route around.

Both `memory add` and `memory list` refuse it, independently.

## Procedure 5 — who wrote what

```sql
SELECT d.id, d.kind, s.name AS scope_name, d.path
FROM durable_writes d LEFT JOIN scopes s ON s.id = d.scope_id
ORDER BY d.id;
```

```text
1  knowledge          .factory/knowledge/alpha-cadence.md
2  knowledge          .factory/knowledge/release-process.md
3  memory     alpha   .factory/memory/alpha/20260911T095426Z--….md
4  memory     alpha   .factory/memory/alpha/20260911T095426Z--….md
5  memory     beta    .factory/memory/beta/20260911T095426Z--….md
```

A memory row always names its scope; a knowledge row never does, because a
note is company-wide. The database enforces that pairing, so a row cannot
drift into the other shape.

This is not in `task_events`, and the reason is worth knowing. That table's
`task_id` is `NOT NULL`, because it logs the transitions of one task — a note
written by a human could not be recorded there at all. And design §11
projects `task_events` into throughput and scrap figures, where a knowledge
write would count as a unit of production nobody produced.

### The order a write happens in

1. stage the file — written and flushed, nothing visible under its real name
2. open the store transaction
3. append the `durable_writes` row
4. rename the staged file into place
5. commit

This order is load-bearing. A crash between 4 and 5 leaves a note with no
provenance row: an observation that is missing. The reverse order would leave
a row asserting a note that was never written: an observation that is wrong,
and a wrong observation is worse than none because it looks like an answer.

## What is refused, and what the refusal says

| You wrote | Factory says |
|---|---|
| no `--source` | `note frontmatter is missing its sources field` |
| `--source data/secrets/api.yaml` | `source #1 lies under data/secrets/` |
| `--source data/pub/../secrets/api.yaml` | `source #1 contains a `..` component` |
| `--name Alpha_Cadence` | `note name "Alpha_Cadence" is invalid` |
| an existing name, no `--update` | `note alpha-cadence already exists` |
| nothing on stdin | `note body is empty` |

The secret-source refusal names the position, never the path's contents.

The `..` case is refused outright rather than resolved. Resolving it would
need the filesystem, and a cited source need not exist locally — but leaving
it unresolved let `data/pub/../secrets/api.yaml` past a check whose entire
purpose is that it does not get past.

## The `.factory/` write invariant

> No Factory command writes to a file outside a `.factory/` directory.

`crates/factory-cli/tests/write_invariant.rs` holds the whole CLI to this. It
walks clap's own command tree to every leaf and requires each one to appear in
its table with either an invocation or a recorded reason it cannot be
invoked. **A leaf in neither fails the test.**

That is the feature. A hand-written list of commands passes happily on the day
someone adds the next one, because the list does not know about it.

If you add a command and the test fails, it is telling you to say what your
command writes. Add it to the table with an invocation. Exempt it only if it
blocks forever, execs, or needs a live harness — the three reasons already
there. "It needed a fixture" is not one.

**What the test does not see.** It watches a scratch instance root. A command
writing into `$HOME` or `/tmp` would be invisible to it.

## Where this is proven

| Claim | Test |
|---|---|
| §6 decides targetability, via the shared rule | `targetable_reflects_kinship_computed_by_the_shared_rule` |
| an idle session outranks a busy one | `an_idle_session_outranks_a_busy_one_held_by_the_same_agent` |
| a lost session is unknown, never idle | `a_disconnected_session_is_unknown_never_idle` |
| an agent with no session is still listed | `every_registered_scope_and_agent_is_listed_even_with_no_live_session` |
| a failed rename leaves no provenance row | `a_failed_rename_leaves_no_durable_writes_row` |
| an existing note needs `--update` | `writing_an_existing_name_without_update_is_refused_and_names_the_flag` |
| a colliding scope name is refused | `add_refuses_an_ambiguous_scope_name_and_names_the_collision` |
| a dangling link is reported, not raised | `note_index.rs`'s unresolved cases |
| one bad file does not kill the index | `note_index.rs`'s unreadable cases |
| a filename survives ordinary tooling | `the_filename_contains_no_character_ordinary_tooling_treats_specially` |
| no command writes outside `.factory/` | `no_factory_command_writes_outside_dot_factory` |

## The drill record

The station was accepted by running the real binary against a throwaway
instance, not only by reading its tests. At that point the workspace held 892
passing tests. The drill found five defects none of them caught:

1. **Memory filenames carried colons.** `rsync` would have read them as remote
   host names. Every test created, read and listed these files correctly.
2. **Timestamps carried microseconds** where design asks for a date, and
   where every other timestamp in the system has second resolution.
3. **The delegation refusal was not English** — "X is itself of X". Station
   8's string, wrong since it was written; a listing shows it once per row,
   which is how it finally surfaced.
4. **`agent list` printed UUIDs** in a refusal while the same row carried the
   name.
5. **A refusal echoed its own structured data as raw JSON** after the help
   line.

All five are fixed and each now has a test that would have caught it.

Two more were found and are **not** fixed here, because they are older than
this station:

- Connecting to a daemon under a deep instance root fails with `path must be
  shorter than SUN_LEN`, naming neither the path, nor the limit, nor what to
  do. Any deep scratch or CI directory hits it.
- `task send` with an unknown scope prints `known scopes: alpha, alpha,
  alpha-sub, beta` — a duplicate name listed twice with no remark, which is
  the very collision `memory add` refuses.
