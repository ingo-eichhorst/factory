# Factory

A daemon that hands tasks to coding agents and watches what happens.

A task is created — by a person, by a schedule, or by another agent. The daemon
opens a session in an agent runtime, starts an agent in it, and gives it the
task along with instructions for how to report back. The agent works, reports
progress, and closes the task. Everything is visible live over a CLI, an HTTP
API, and a web UI.

This is a prototype. It runs, and the parts that do not work yet say so.

## The five seams

Everything Factory can be pointed at something else is one of five traits, and
the daemon cannot tell a built-in implementation from a plugin:

| Adapter | What it decides | Ships with |
| --- | --- | --- |
| **Agent** | how a harness is started, and what a task sounds like to it | `claude-code`, `pi`, `codex`, `opencode`, `shell` |
| **Agent runtime** | where agents actually run | `herdr` |
| **Task store** | where tasks live — the CRUD contract, chosen per scope | `sqlite` |
| **Interface** | how the outside reaches the daemon | `cli` (unix socket), `http` (REST + WebSocket + UI) |
| **Knowledge provider** | how the knowledge vault is searched — read side only | `keyword` |

The traits are in `crates/factory-core/src/adapter/`. Nothing in core knows
about sqlite, herdr, axum, or any other concrete choice.

## Quickstart

```sh
cargo build --workspace

mkdir -p /tmp/demo-factory/projects/demo
target/debug/factory-daemon --root /tmp/demo-factory init --scope projects/demo
target/debug/factory-daemon --root /tmp/demo-factory run
```

Then, in another terminal:

```sh
export FACTORY_ROOT=/tmp/demo-factory
alias factory=target/debug/factory

factory status
factory adapters

# The `shell` agent runs the instructions as a shell command. No model call.
factory task create "say hello" -i "echo hello > hello.txt" --agent shell --run
factory task list
factory task log <id>

# A real agent, in the same shape.
factory task create "add a test for the parser" \
  -i "Add a test covering empty input, then run the suite." \
  --scope demo --agent claude-code --run

# Recurring work. A cron schedule is UTC unless it names a timezone.
factory task create "morning sweep" -i "..." --schedule "0 9 * * 1-5" --timezone Europe/Berlin
factory task create "heartbeat" -i "date >> beat.txt" --agent shell --schedule "every 5m"

factory watch
```

The web UI is at <http://127.0.0.1:8787>.

## Task agents and standing agents

Most agents exist for the length of one task. Some should just be there — an
assistant to talk to, a session kept warm in a project you work in every day.
Those are **standing agents**, and a scope declares them:

```yaml
# projects/demo/.factory/config.yaml
version: 1
scope:
  id: 8fc86b67-aeba-4935-a623-d75f59d77acd
  name: demo
  agents:
    - name: watcher
      harness: pi
      lifetime: permanent     # started with the daemon, restarted if it dies
      args: ["--model", "opus"] # arguments for this declaration only
    - name: scratch
      harness: shell
      lifetime: temporary     # startable, but nothing starts it on its own
    - name: reviewer
      harness: claude-code
      lifetime: task          # not standing: offered for tasks in this scope
      sandbox: docker         # declared, not yet enforced -- see below
```

A permanent agent is **never failed for being quiet** — being quiet is what it
is for. It is only ever checked for whether its session is still there, and
restarted if it is not. The run watchdog, with its acknowledgement and run
timeouts, does not touch it.

On startup the daemon lines up what the config declares against what is still
running, and says which of the three things happened: a session that outlived
the daemon is **adopted** rather than duplicated, a declared agent that is not
up is **started** if it wants starting, and a session whose agent the config no
longer declares is **closed** rather than left for somebody to find next week.

`lifetime` may also sit inside the singular `agent:` block, which is how
instances written before standing agents existed already spell it.

`sandbox` says where an agent's runs should execute: `none` (the default),
`docker`, or `srt`. **Nothing enforces it yet.** It is read, stored, and shown
on the L2 Environment page, and an agent declaring `docker` starts exactly the
way one declaring `none` does. The field exists ahead of the machinery so the
gap between what a run needs to reach and what it can reach is written down
somewhere rather than assumed, and so the UI has something true to display.
Enforcing it means solving three host-shaped things a container breaks — the
control socket, the `factory` callback binary, and the run's git worktree,
whose `.git` is a pointer file into the scope's repository — which is why
`srt` ([anthropic-experimental/sandbox-runtime][srt]), which wraps the same
process on the same host, is the likelier one to arrive first.

[srt]: https://github.com/anthropic-experimental/sandbox-runtime

`args` works in both the singular `agent:` block and entries in `agents:`. The
daemon appends these arguments after any defaults supplied by the adapter, so a
declaration can override a repeated flag for one scope without changing the
shared adapter. A task that names a bare adapter with no matching declaration
still uses only that adapter's defaults. The `shell` agent does not accept
`args`: it runs the task instructions as the command itself.

A standing agent never gets a task prompt — `prompt()` is only called for a
task run — so it is told about Factory itself through `launch_spec` instead,
the one call every agent gets. `claude`, `pi`, and `opencode` each get the
guide through their own system-prompt mechanism (a file for `claude` and
`pi`, an env var for `opencode`); a task run on those harnesses gets the same
guide the same way, so the prompt itself stays narrower. `codex`'s own
`developer_instructions` config key takes text, not a path, and a
multi-paragraph argument does not survive `herdr agent start` — verified
live, herdr refuses it outright rather than mistyping it — so `codex` takes
the documented fallback: a task run gets the guide above the task in its
prompt, and a standing `codex` agent gets nothing at all. `shell` is not a
model and gets no guide either way. A standing agent's session that outlived
the daemon is *adopted* rather than restarted, so it keeps whatever guide it
started with; a role given to it afterward with `factory agent role` is not
reflected in a guide already sitting in a launched session.

Every agent has a **role**, and a task names a **concrete agent**, not a
harness. `assistant` and `scratch` are different agents even when both are pi.

```yaml
# projects/demo/.factory/config.yaml
version: 1
scope:
  id: 8fc86b67-aeba-4935-a623-d75f59d77acd
  name: demo
  agents:
    - name: assistant
      harness: pi
      lifetime: permanent
      role: foreman        # worker is the default
```

- **worker** reads tasks, and updates the ones assigned to it — its own scope,
  its own name. It cannot create tasks, start or cancel runs, or hand its work
  to somebody else.
- **foreman** runs a scope: it creates tasks there, edits any of them, and
  assigns them to the other agents in that scope. Its authority stops at the
  scope boundary — a foreman in `demo` cannot touch `root`.

An agent says which one it is by presenting the token Factory put in its
session as `FACTORY_TOKEN`; the CLI sends it on every request. No token means
the owner.

Those two are not the whole list. A role is a **name**, a set of **grants**, and
a **reach**, and an instance names as many as its way of working needs:

```yaml
roles:
  runner:
    describe: starts the work that is already on the board, and nothing else
    grants: [task.run, task.cancel]
    reach: scope          # own (its own work) or scope (everything in it)
  reviewer:
    describe: works its own tasks and says what it found
    grants: [task.edit, task.report]
    reach: own
```

The grants decide **which** requests an agent may make; the reach decides
**whose** tasks, agents and sessions it may make them about. `task.*`,
`agent.*` and `*` are wildcards; a grant that names nothing is refused at load
rather than ignored, and so is an agent given a role the instance never defined
— the message says which agent it was. The grants are:

    task.create  task.edit  task.delete  task.run  task.cancel  task.report
    agent.start  agent.configure  agent.stop  agent.input  run.input
    workflow.create  workflow.edit  workflow.delete  workflow.run  workflow.cancel
    knowledge.write  dataset.edit  bench.run  policy.attest  goals.checkin

Reading is not among them, because reading is open to every agent: one that
cannot see the board cannot coordinate with anyone.

`worker` and `foreman` ship written in that same vocabulary — a worker is
`[task.edit, task.report, run.input]` at `reach: own`, a foreman is everything
at `reach: scope` — and neither can be redefined. An instance that could
rewrite `worker` from one line would widen every agent that never asked for a
role.

What a grant cannot say is written out in `access.rs`, in the arm it belongs
to: handing a task to somebody else is not editing it, so a role that reaches
only its own work may change what its task says and never whose it is.

### Roles down the scope tree

The instance root's `roles:` hold in every scope. A nested scope can add roles
of its own under `scope.roles` in its own `.factory/config.yaml`, and every
scope below it inherits them:

```yaml
# .factory/config.yaml                  (instance root)
roles:
  runner:
    describe: starts the work that is already on the board, and nothing else
    grants: [task.run, task.cancel]
    reach: scope

# projects/.factory/config.yaml
scope:
  id: 3c0f…
  name: projects
  roles:
    reviewer:
      describe: works its own tasks and says what it found
      grants: [task.edit, task.report]
      reach: own

# projects/demo/.factory/config.yaml
scope:
  id: 8fc8…
  name: demo
  roles:
    reviewer:                 # replaces the inherited reviewer, here and below
      describe: reviews, and may also open follow-up tasks
      grants: [task.create, task.edit, task.report]
      reach: own
  agents:
    - name: critic
      harness: pi
      role: reviewer
```

A scope's roles are, in order: the two built in, the instance root's `roles:`,
then each scope's `scope.roles` from the top of the tree down to the scope
itself. The nearest definition wins. A scope's parent is the nearest configured
scope above it **by path**, never by name: names may contain `/` and are
matched loosely on purpose, so a scope named `projects/other` whose directory
is somewhere else is not below `projects`.

- **An override replaces the whole definition** — description, grants and
  reach. Grants are never merged: a merge could only widen, and nobody reading
  either file could tell what the result was.
- **`worker` and `foreman` cannot be redefined at any level**, for the same
  reason they cannot be redefined at the root.
- **Roles never flow up or sideways.** A role defined in `projects/a` does not
  exist in `projects/b` or at the root. An agent given one there is refused,
  and the message lists the roles that scope does have.
- **Inheriting a definition does not inherit authority.** `reach: scope` still
  means the agent's own scope and nothing past it. A foreman in `projects`
  cannot touch a task in `projects/demo`, and a `reach: scope` role inherited
  from `projects` cannot touch a task in `projects`, in a sibling, or in a
  scope below the agent's own. Inheritance decides which roles exist where,
  never how far one reaches.

The instance root writes its roles only in its top-level `roles:`; a
`scope.roles` block in the root's config is refused at startup, because one
file with two role layers would leave a person guessing which one wins. A
`roles:` block written beside a nested scope's `scope:` block, where that file
never reads it, is refused with the file named rather than silently dropped: a
role that quietly does not exist is only noticed once an agent is refused.
A role that disappears from a chain — somebody edited a parent's file — is
refused at load for every agent declared with it, and `authorize` refuses an
agent still holding it, naming the scope.

> **This is not a security boundary.** Every agent runs as the owner of the
> instance and can reach the same socket, so an agent that simply leaves the
> token out is indistinguishable from a person. Roles keep an agent that
> follows its instructions inside its job. They do not stop one that does not.

To give every scope a foreman without writing one into each:

```yaml
daemon:
  foreman:
    enabled: true        # off by default: this starts one agent per scope
    harness: pi
    name: foreman
    exclude: [root]      # the instance root is the company, not a project
```

A scope that already declares an agent with `role: foreman` keeps its own.

A role can also be given to a standing agent while it is running, from the
roster or from the CLI, without editing the config:

```sh
factory agent role demo/watcher reviewer   # until somebody says otherwise
factory agent role demo/watcher            # back to what the config says
```

The assignment is kept with the agent, not written into `config.yaml`: the file
stays something a person owns, and the roster says when an agent is wearing a
role its declaration does not give it. The session it is already in keeps
running — a role is checked when an agent asks for something, so the new one
holds from its next request. Only the owner may do this. An agent that could
hand itself a role would not be bounded by the one it has.

```sh
factory agents                       # scopes, their agents, and what each is doing
factory agent start demo watcher
factory agent stop demo/watcher      # stays stopped until someone asks again
factory agent output demo/watcher
factory agent input demo/watcher --text "how is it going?" --key enter
factory agent role demo/watcher reviewer
```

The roster can add a declaration to the scope currently selected in the rail.
**New agent…** writes the declaration into that scope's own
`.factory/config.yaml`; it does not create an in-memory-only agent or choose a
scope while **All scopes** is selected. The running daemon adopts the saved
declaration immediately, and a permanent declaration whose autostart box is on
is brought up in the background. The `agent.configure` grant carries this
write only to roles with scope reach; it ships with `foreman`, not `worker`.
Locally declared agents can be deleted from the same selected-scope roster;
deleting a standing declaration also closes its session. Synthesized foremen
and undeclared adapter defaults are shown but cannot be deleted there.

CLI arguments are entered one argument per line. A line such as `local model`
is one argument, not two shell words, and their order is preserved in `args:`.
Factory normalizes the separate runtime session name to Herdr's lowercase,
32-character identifier contract without changing the configured agent name.

A Factory scope is a herdr workspace: every standing agent and task run that
belongs to `demo` lands as its own tab inside a workspace labelled `demo`,
resolved by that label if it already exists (a hand-made workspace someone
named `demo` is joined on purpose, not collided with) and created the first
time a `demo` agent starts. The herdr agent name still carries `factory-` as
an outer namespace ahead of the scope, because herdr agent names are global
and Factory adopts any agent already carrying the name it is about to start —
dropping the prefix could make it adopt a person's own hand-made agent by
accident. Each standing agent also carries the command to get into its
terminal yourself — `herdr --session factory agent attach
factory-demo-watcher`. A shell session has no named agent to attach to, so
Factory says so instead of printing a command that would fail.

Stopping a standing agent or ending a task run closes only its own tab, never
the scope's workspace — the workspace is shared by everything else running in
that scope. Workspaces made by earlier versions of Factory (labelled
`factory: …`, one per session) are not migrated; close them by hand.

## Secrets

Factory injects no credentials. The whole of what it adds to a session is six
`FACTORY_*` variables — the scope, the socket, the callback binary, the token,
and a run's task id and attempt.

That is **not** the same as an agent having no credentials. An agent is a shell
running as the daemon's owner, so it reads whatever that user can read:
`~/.claude/.credentials.json`, `~/.config/gh/hosts.yml`, `~/.netrc`, ssh keys,
a scope's own `.env`, the system keychain. The runtime is a terminal
multiplexer, not a boundary.

The L2 Environment page's **Secrets** tab reports exactly that and nothing
more: for each known location, whether a file is there. No value is ever
opened, held, logged, or returned — `present` is the entire result of each
check, and there is no write path, in the UI or over the socket.

## Knowledge

`<root>/.factory/knowledge/` is a vault Factory keeps — one Obsidian-compatible
directory of Markdown pages and any other document, holding authored content
nothing in Factory regenerates (see the `.factory/` rule above). The L5
**Knowledge** tab draws it as a graph. The old `<root>/knowledge/wiki/` (v1's
fixed path) comes in only through an explicit import; nothing moves on its
own, and there is no fallback read of it.

`factory knowledge` and `GET /api/knowledge` rebuild the index from the files
on every call: no link table to keep in step, and an index request never
writes anything. Every `.md` file is a page and a graph node — frontmatter is
optional, and a page's title falls back to its file name when there is none
or it names no `title`. Every other regular file is a document, stat-ed and
never opened. A page carries tags from frontmatter `tags:`/`keywords:` and
inline `#tag` (Obsidian's rules: not purely numeric, not in code or a
heading, not glued to a URL), and can reference a document through
`![[f.ext]]`, `[[f.ext]]` or `[text](rel/f.ext)`; `[text](page.md)` is a page
edge too. A link resolves page-relative, then root-relative, then by a
unique file name in its own namespace (pages or documents); more than one
match is reported rather than guessed at, and an unresolved or ambiguous
target is a gap, not an error. `present: false` names `legacy` when the old
wiki still exists, and the empty state shows the exact import command.

**The index reads; it does not write.** No page's text and no document's
bytes ever reach the browser or the CLI — only titles, tags, links, and file
metadata. A `sources[]` entry under `data/secrets/` is reported as a finding
and nothing under it is ever opened or even stat-ed; that call is decided
from the string alone, before any filesystem access. The knowledge base is
company-wide: the scope rail does not filter this tab.

**Writing only ever adds a file.** `factory knowledge import <dir>
[--into <subdir>] [--overwrite]` copies a directory tree into the vault,
preserving relative paths; `factory knowledge add <file>... [--into]
[--overwrite]` adds one or more files, defaulting a `.md` file to the vault
root and everything else to `documents/`. The UI's "Add documents" uploads
through `PUT /api/knowledge/files?path=&overwrite=`, the one write path a
browser can reach, since it cannot name a path on the daemon's own disk.
Nothing here edits or deletes an existing file except an explicit
`--overwrite` replacement. A target that would escape the vault, or a source
under `data/secrets/` or elsewhere under `.factory/`, is refused before a
byte is touched — the latter two refusals never echo the path back. This is
the one place `Grant::KnowledgeWrite` gates: the knowledge base is
company-wide, so only the owner or a foreman whose own scope *is* the
instance root may hold it.

**Searching goes through a provider.** `factory knowledge search <words>
[--tag <tag>]... [--scope <scope>] [--limit <n>]` and
`GET /api/knowledge/search?q=&tags=a,b&scope=&limit=` ask the instance's
knowledge provider — `daemon.knowledge_provider`, `keyword` unless the config
names another — and answer with page ids, titles and a one-line reason each
matched, best first, plus the vault's path so a hit is one step from its
file. Never page text: the agent opens the file itself. Searching is a read,
open to every agent, and the guide says so; an agent that names no scope
searches from its own. The limit defaults to 10 and is capped at 50, and a
search with neither words nor tags is refused.

The built-in `keyword` provider reads only what the index already carries.
A query term that is one of a page's tags (or the last segment of a nested
one) scores 4, a word of its title 2, a segment of its path 1 when the title
did not already have it; a matched page whose `area` is the asking scope's
last segment gains 1, and every page one link away from a matched page, in
either direction, gains 1 per such page. `--tag` filters matches and
neighbours alike, and on its own is the whole question. Equal scores are
ordered by page id, so the same vault and query always give the same list.
A word that appears only in a page's text is not found — that is a different
provider's job. Writes do not pass through a provider: `import`, `add` and
the upload keep their checks and tell the provider afterwards which files
they wrote, and a provider that cannot keep up is a warning, never a failed
write. A provider keeps at most a derived index, rebuildable from the vault.
See `.specs/adr/0003-knowledge-search.md` in the company repository.

**A task can also be handed pages instead of asking.** With
`knowledge_hints` on (`factory task create --knowledge-hints`, `task edit
--knowledge-hints`/`--no-knowledge-hints`, or the Knowledge box on the task
form) every run of the task is searched for once, as it is dispatched: its
title and instructions are the query, its scope is the asking scope, and the
top five pages travel on the run's binding. A harness agent's prompt lists
them as file paths with the reason each matched; the shell agent gets them as
a `{vault, hits}` JSON file whose path it exports as `FACTORY_KNOWLEDGE_FILE`.
The run's journal records exactly which pages it was handed (a `knowledge`
entry, with the hits as its data), so a prompt can still be explained once the
vault has moved on. Off by default. A search that fails or matches nothing is
journaled and the run starts anyway -- hints are a help, never a reason not
to start.

The Knowledge tab asks the same provider from the Search form above the
graph: the ranked answer is listed with each page's reason, and the hits stay
lit in the graph until the answer is cleared. The filter box in the graph's
own toolbar is unchanged -- it matches labels in the browser and asks nobody.

## Benchmarks

**v1 declared and displayed configurations and ran nothing. v2 adds datasets
and the ability to run a dataset against chosen agents.**

A *configuration* is still what it was: one distinct harness, full arguments,
and sandbox that a task could be dispatched with today, including the foreman
`daemon.foreman` would synthesize. `factory bench` with no subcommand, and
`GET /api/benchmarks`, still show that inventory — the harness itself, and a
model when a declaration's `args` spells out `--model`, `--model=`, or `-m`.
Harness version, tool surface, context policy, and retry budget are recorded
nowhere yet, so every configuration is still `pinned: false`. No argument
value but the extracted model ever reaches a payload — an `args` entry can be
a secret, the same rule the Secrets tab lives by, so everything else is
reduced to its flag with the value elided; a bench attempt's own
configuration snapshot follows the same rule, and its full arguments exist
only long enough to be folded into a sha256 `config_hash` inside the daemon.

A **dataset** is a set of cases, one file at
`<root>/.factory/datasets/<name>.yaml` — authored content, like the knowledge
vault, and the source of truth: it is re-parsed on every read, so a hand edit
shows up on the next call, and `revision` bumps on every write Factory makes
to it. A case names a scope, carries instructions, and may pin a `base`
commit, a `reset` command that runs before an attempt starts, and a `gate`
command whose exit status is the verdict. `gate` and `reset` run as the owner
inside the attempt's own worktree — the same trust as the `shell` agent, and
the dataset view says so. Datasets are built three ways: by hand
(`factory dataset create` and `case add`), generated from recorded tasks
(`factory dataset from-tasks`, which copies title, instructions and scope,
records `origin` for reference only, and never sets a `gate`), or
bulk-imported (`factory dataset import`, accepting `.jsonl`, `.json`,
`.yaml`/`.yml`, or `.csv`; all-or-nothing, naming every line or row and field
that is wrong).

A **bench run** is `dataset@revision × agents × attempts`: a snapshot of the
dataset's cases taken the moment it starts, so a later edit to the dataset
never changes a run already going. Each case's `base` — its own pinned
commit, or the scope's HEAD when the run starts — is resolved once, to the
full commit SHA it names in that scope, so every agent's every attempt at a
case branches from exactly the same commit and `git` itself only ever sees a
SHA, never a case-authored string. A `base` that does not resolve to a
commit there — a bad revision, or one that has since been rewritten away —
never dispatches: every attempt at that case is `skipped`, with a reason
naming the base, the same as an agent that does not resolve. Each attempt is
one ordinary Factory task with its own worktree, so `--attempts N` means N
independent tries, never "retry until it passes," and `--concurrency N`
(default 1) bounds how many attempts are in flight at once.

**The verdict.** A case's verdict is the exit status of its gate command,
recorded with the exit code, the last 4 KiB of its combined output, and the
attempt's wall-clock time — even when the agent itself reported `failed`, the
gate still runs and is still the judge. Judging a settled attempt — which
means running its gate — never happens on a caller's own path: a report, a
cancel, the scheduler's own watchdog, and restart recovery all merely queue
it, so a slow gate (up to the case's own timeout, ten minutes by default)
never makes `factory task report` itself hang, and never stalls anything
else the scheduler is doing meanwhile. A case with no gate still runs, and
its result is `unverified`: the agent's own report of `done` or `failed`,
shown but never counted in a resolve rate (`pass / (pass + fail)`; `null`
when nothing was gated at all). A case whose reset command fails is
`skipped` before the agent is ever dispatched — a case is never run dirty.
An agent that does not resolve in a case's own scope is `skipped` with the
reason, and the run continues past it. A run cancelled mid-flight settles
every remaining attempt `cancelled`, even one whose judgement was already
queued or under way — that verdict, once written, is never overwritten by a
late judgement landing after the cancel. One lost, timed out, or that never
got as far as a report before its run ended is `error`. Cost and tokens are
still not recorded, and the payload says so rather than showing a `0`.

`factory bench run <dataset> --agent <scope>/<agent> [--attempts N]
[--concurrency N] [--case <id>]` starts a run; `factory bench runs`, `show`,
`cancel` and `clean` list, inspect, stop, and — once a run is finished —
remove exactly that run's worktrees and branches, explicitly, and only for
the owner. Restarting the daemon mid-run resumes it: an attempt already
settled but not yet judged is judged, one that never started is started, and
one still in flight is left to the ordinary run watchdog.

## Policies

A **policy** is a catalogue of **controls** — a regulation, a standard, or the
company's own best practice, broken into checkable items — plus a computed
answer to one question per control: is there current evidence it is met?
Nothing here evaluates a policy to *allow* or *forbid* anything; enforcement
already lives in roles, sandboxes and the Secrets seam. This is deliberately a
picture of the plant, not a policy engine — design §8 of the company spec
defers a rule language evaluated at runtime, and this does not narrow that.
See `.specs/adr/0004-policy-controls.md` in the company repository.

**The catalogue.** `<root>/.factory/policies/<framework>.yaml` — authored
content, like the knowledge vault and datasets: hand-written, re-parsed on
every read, never written by Factory. One file per framework:

```yaml
# .factory/policies/cra.yaml
framework: cra
title: Cyber Resilience Act
kind: regulation                # regulation | standard | best-practice
controls:
  - id: annex-i-2-1             # [a-z0-9][a-z0-9-]*, referenced as cra/annex-i-2-1
    title: Identify and document components (SBOM)
    max_age: 30d                # Nd, Nh, or Nw
    maps_to: [iso27001/a-8-8]
    evidence:
      - check: knowledge        # a vault page tagged control/cra/annex-i-2-1
      - check: attestation      # a person's recorded word, with an expiry
      - check: task              # the task's newest *finished* run, done within max_age
        task: sbom-export
        max_age: 30d
```

See `examples/policies/cra.yaml` for a complete one and
`crates/factory-core/src/policy.rs` for every field. `kind` is set per
framework and may be overridden per control; only `regulation` and `standard`
controls count towards a rollup's `compliant` — `best-practice` controls are
shown but never counted, since nobody is out of compliance for skipping a
recommendation. `maps_to` names equivalent controls in other frameworks,
declared one way and read both, one hop only — so one piece of evidence
satisfies every framework asking for the same fact, and deliberately not
transitive, so a chain of loosely related controls can never bootstrap each
other into looking compliant. A file that fails to parse, names a `framework`
other than its own file stem, or repeats a control id is a finding naming the
file; every other file still loads.

**Applicability, down the tree.** Which frameworks bind a scope is `policies:`
at the instance root and `scope.policies` in a nested scope's own config — the
same root-to-leaf chain `Engine::roles_for` walks for roles, resolved fresh
from the live config on every read, never a second copy kept in step. Unlike
roles, where the nearest definition wins, a policy layer may only **add or
tighten**: name another framework, shorten a control's `max_age`, or mark one
`n/a` with a rationale — never drop or loosen a commitment an ancestor already
made, or a child project could opt itself out of the GDPR. A control's
effective `max_age` is the minimum across the catalogue's own value, its
checks' own value, and every layer's tightening, root to leaf; a `tighten`
that would not lower that running minimum has no effect and is reported as a
finding rather than silently ignored. `n/a` needs a non-empty rationale — an
empty one is a finding and the control stays applicable — and every `n/a`, at
whichever scope declared it, is always listed rather than left silent: ISO
27001 calls this a Statement of Applicability, and ADR 0004 keeps the name.

**Checks and statuses.** Evidence is evaluated per check kind, and all nine
are evaluated for real: `knowledge` (a vault page tagged
`control/<framework>/<id>`, or a check's own `tag`), `attestation` (an
unexpired, unwithdrawn attestation recorded for the control), `task` and
`workflow` (the newest *finished* run — done, failed or cancelled; a run
still in progress is skipped, so a control never reads `open` for as long as
its own evidence task happens to be running — is `done`, within the
control's effective `max_age`; with none, any done run counts as current; no
finished run at all is `open`, naming an in-progress one if there is one),
and `gate` (a dataset's gated cases — or, with `case`, one named case — all
`pass` in the newest *settled* bench run of that dataset, within `max_age`;
`unverified` never counts). `task`/`workflow` name their target in the
evaluated scope, by id or by exact title; a title matching more than one is
a finding, reported `open` rather than guessed at.

The remaining four read facts the engine resolves once, synchronously, from
the live config snapshot rather than a store:

- **`roles { forbid: [grant...] }`** — satisfied when no agent Factory would
  actually dispatch in the scope (`Scope::agents_with`, resolved against
  `Engine::roles_for`) is bound to a role holding any of `forbid`; the
  reasons name the offending agent, its role, and the grant. A scope that
  declares no agent is satisfied outright — nothing there can hold the
  grant. `agents_with`, not the narrower `declared_agents`, is deliberate: a
  synthesized foreman (`daemon.foreman`) is a real agent Factory starts and
  hands work to, and it is hard-coded `Sandbox::None` and the `foreman`
  preset role (every grant there is), so a scope that turns it on shows up
  here and in `sandbox` below rather than being silently exempt because
  nobody wrote it down.
- **`sandbox`** — satisfied when every agent `Scope::agents_with` names for
  the scope has a sandbox other than `none` (`ScopeAgent.sandbox`). Same
  empty-scope and same-foreman rule as `roles`.
- **`secrets { absent: [location...] }`** — satisfied when none of the named
  locations is present, read from the same inventory the L2 Secrets tab
  itself reports (`Engine::credential_inventory`): presence only, never a
  value. With no `absent` list (a bare `check: secrets` still parses), the
  default is the scope's own `.env`. The known location ids are `anthropic`,
  `github`, `aws`, `netrc`, `ssh` (machine-wide — every agent runs as the
  daemon's owner, so these are the same regardless of scope) and
  `scope_env` (that scope's own `.env`). This amends the ADR's own wording
  — "secrets reach agents only through the Secrets seam" — since there is no
  such seam (see "Secrets" above); what Factory actually records, and so
  what this checks, is a named location's absence.
- **`daemon { fact }`** — satisfied when the named fact about the daemon's
  own configuration holds. Only a small, unambiguous set is supported:
  `foreman_enabled` (`daemon.foreman.enabled`), `http_loopback_only` (every
  mounted `http` interface binds to a loopback address, or none is mounted
  at all — `Some(true)` either way; a `bind` that does not parse as a socket
  address is left undetermined rather than guessed at with a DNS lookup),
  and `power_assertion` (`daemon.power_assertion`). A `fact` outside this
  set is a finding at catalogue load time and stays `open`.

Neither `roles`/`sandbox`/`secrets`/`daemon` carries a `refs` entry: nothing
behind them is an id a UI could link to yet (an agent name is not one of
`EvidenceRefKind`'s kinds, and a daemon/secrets fact is not tied to any one
record at all) — the L6 Policy tab instead links a gap in one of these to
the level that can close it (Roles, Sandboxes, Secrets, or L1
Infrastructure). A control's status is `satisfied` (a check found current
evidence), `attested` (an unexpired attestation covers it), `stale`
(evidence or an attestation existed but is older than `max_age`, or the
attestation expired), `open` (no evidence), or `n/a` (does not apply here,
with its rationale) — and, wherever the evidence behind it carries an id (a
`task`, `workflow`, `gate` or `attestation` check, never yet a `knowledge`
one), a `refs` list alongside its human reasons, pointing at the task, run,
workflow run, bench run or attestation, for a UI to link straight to. Status
is computed on every read, the same as the knowledge index — no status table
to keep in step. **"Compliant" means the evidence is complete, not that the
company is certified** — a framework's rollup is `compliant` only once every
`regulation` and `standard` control in it is `satisfied`, `attested`, or
`n/a`; `best-practice` controls are counted in their own bucket and never
affect it.

**Attestations** are the one piece of new state this introduces: a person's
recorded word that a control is met, with a pointer to the evidence and an
expiry — the only check kind a person satisfies by saying so. They are
written through the API into the instance database, append-only like the
journal: recording one inserts a row, and withdrawing one inserts another
that references it rather than touching the first, so the history is never
rewritten or lost. `Grant::PolicyAttest` (`policy.attest`) gates both — the
same root-scope rule as `knowledge.write`: an attestation speaks for the
company, not for one project, so only the owner or a foreman whose own scope
*is* the instance root may record or withdraw one.

**The subtree rollup.** Asking about a scope, or the whole instance, folds
every scope in that subtree's own evaluation into one status per control: a
scope where the control is `n/a` has nothing to say about it, and among the
rest the worst status wins — `open` beats `stale` beats `attested` beats
`satisfied` — so a control counts compliant only when every scope it applies
to is, not merely one.

`factory policy [status] [--scope S] [--framework F] [--json]` shows this:
every applicable control's status, findings, and every `n/a` in scope, rolled
up per framework. `factory policy show <fw>/<id> [--scope S] [--json]` is one
control's full detail, including its whole attestation history. `factory
policy attest <fw>/<id> --scope S --evidence <pointer> --expires
<30d|2027-01-01> [--note N]` records one; `factory policy withdraw
<attestation-id> --reason R` withdraws it. `factory policy frameworks
[--json]` lists every loaded catalogue. The same requests answer over HTTP:
`GET /api/policy?scope=`, `GET /api/policy/controls/{framework}/{id}?scope=`,
`POST /api/policy/attestations`, and `POST /api/policy/attestations/{id}/withdraw`.

**Closing a gap.** `factory policy remediate <fw>/<id> --scope S [--agent A]`
creates an ordinary task in `S` through the exact path `factory task create`
itself uses (`Engine::create`) — its validation, its worktree, its
`Event::TaskCreated`, none of it a hand-rolled copy — titled `Close
<fw>/<id>: <title>` and labelled `policy=<fw>/<id>`. Its instructions are the
catalogue's own `remediation:` text (if the control carries one), the
missing evidence — each unsatisfied check and why, verbatim, the same
reasons the L6 tab and `policy show` themselves print — and a closing line
naming every check that would satisfy the control
(`policy::remediation_instructions`). It needs `task.create` in `S`, under
the exact reach rule `Request::TaskCreate` itself is checked against
(`access.rs` maps both to the same grant and the same caller's-own-scope
check) — this is not a second door into creating a task, it is the one
door. Refused outright when the control is already `satisfied`, `attested`,
or `n/a` at `S` — there is nothing to remediate; refused, naming the
existing task by id and title, when a non-terminal task already carries
that label in `S` — a second `remediate` finds the same task rather than
spawning a duplicate. `POST /api/policy/remediate` (`{control, scope,
agent?}`) answers the same way `POST /api/tasks` does, `{"kind":"task",
"task":{...}}` — a remediation task is an ordinary task in every way but how
it was asked for. The L6 tab offers a "Create task" button on every open or
stale row and in the control-detail modal (never on a satisfied, attested,
or n/a control); after it creates one, the button becomes a link straight to
it, and a refusal shows inline, next to the button — this page uses neither
`alert()` nor `confirm()` anywhere.

**Exporting for an audit.** `factory policy export [--scope S] [--format
md|json]` prints a snapshot to stdout: an instance/scope/produced-at header
and the "compliant means evidence complete, not certified" sentence, then
per framework its rollup, then — scope by scope — every control that
framework applies to there with its status, reasons, `refs` (the task, run,
workflow run, bench run or attestation id behind it, never a page's own
text), and its whole attestation history at that scope (evidence pointer,
by, attested, expires, withdrawn — an audit trail, so a withdrawn or expired
one is shown, not dropped), then every `n/a` with its rationale and
declaring scope, then every finding. No page text anywhere in it, only
pointers. `format` defaults to `md`; `json` is the same data
(`policy_export::PolicyExport`, wrapping the `Request::Policy` report plus
each scope's own attestations) as `serde_json::to_string_pretty`, so neither
format can say something the other does not.
`GET /api/policy/export?scope=&format=` answers with the body itself — not
the usual `{"kind":...}` envelope — `Content-Type` `text/markdown;
charset=utf-8` or `application/json; charset=utf-8`, and `Content-Disposition:
attachment; filename="policy-<scope|instance>-<date>.<format>"`, so a browser
click downloads it directly. The L6 tab's header carries an "Export" link
that does exactly that click, for the scope currently selected on the rail.

## Goals

L6 Direction's other tab: the company's **vision, mission and objectives**,
where key results are computed from data Factory already records wherever
possible rather than self-reported. **Goals enforce nothing** — the same
deferral policies rest on (design §8): nothing here starts, stops, or gates
any work. `factory_core::goals` (the catalogue loader, findings, scoring,
evaluation) and `factory_core::metrics` (the shared registry, also read by
the Scenarios tab below, `#100`) are pure and tested on their own; this
section is what `factory-daemon` builds on top of them.

**The catalogue.** `<root>/.factory/goals/` — authored content, the same
pattern as the knowledge vault, datasets and policies: hand-written, re-read
on every request, never written by Factory. `direction.yaml` is the
long-term frame (vision, mission, values, the north star and its inputs,
obstacles); one `<cycle-id>.yaml` per planning cycle carries that cycle's
`objectives` (each a handful of key results) and its `roadmap` (Now/Next/
Later lanes, ProdPad's confidence lanes rather than promised dates). See
`examples/goals/` for a complete pair and `crates/factory-core/src/goals.rs`
for every field. A key result names exactly one of `metric` (computed) or
`manual: true` (recorded by check-in) — both or neither is a finding, never
a hard failure that takes the rest of the file down with it.

**The metric registry.** A metric is a named, documented projection over
data that already exists — not a query language, so this stays inside
design §8 the same way a policy check does. `factory metrics` (or `GET
/api/metrics`) lists every id this instance can compute:

| id | means | source |
|---|---|---|
| `throughput_week` | finished runs, trailing 7 days | `production.rs`'s daily grid |
| `first_pass_yield` | `first_pass/finished` (done and not rework, over finished), trailing 28 days | `production.rs`'s daily grid |
| `scrap_rate` | `scrapped/finished`, trailing 28 days | `production.rs`'s daily grid |
| `compliance.<framework>` | share of counted controls satisfied, attested, or n/a | `policy_report(None)`'s subtree rollup |
| `open_controls.<framework>` | count of counted controls still open or stale | `policy_report(None)`'s subtree rollup |
| `bench.resolve_rate.<dataset>` | the newest settled bench run's resolve rate | `bench::aggregate` |
| `goal_tasks_done.<objective>.<kr>` | count of tasks labelled `goal=<objective>/<kr>` whose status is `done` | task labels, through `TaskStore` |
| `quality.<characteristic>` | share of declared quality scenarios under an ISO 25010 characteristic that are met, across every scope | `quality_report(None)` — see "Quality attributes" |
| `unit_cost`, `tokens_per_run` | cost/tokens per run | **unavailable**: a `Run` records no model, tokens or cost yet (design §12.6) |

`throughput_week`/`first_pass_yield`/`scrap_rate` read `production.rs`'s own
daily grid directly rather than re-deriving "finished"/"scrapped"/
"reworked" a second time — that module's own doc comment is the one place
those words are defined. Every metric is computed **lazily**, like a policy
fact: `Request::Metrics { ids }` only touches `production`/`policy_report`/
the bench store when some asked id actually needs it, and each is read at
most once per call no matter how many ids ask for something behind it. An
id the registry has never heard of refuses the whole call (a typo should
not come back as a quiet `None`); `unit_cost`/`tokens_per_run` — named in
the registry but not yet computable — come back as `value: None` with that
reason, never an error. `ids` empty means every non-parameterised metric
(available or not) plus whatever the loaded goals and policy catalogues
themselves name. Only the three production-based metrics carry a history
today: one point per day over the daily grid's own 53 weeks, each point
that metric's own trailing-window definition evaluated as of that day — the
series' own last point always equals the metric's current value.
A value's `as_of` is when the data behind it is from, not when it was asked
for: `first_pass_yield`/`scrap_rate` are as of the end of the newest day in
their window that finished anything (and have no value at all, with the
reason, when nothing in their trailing 28 days finished — never an older
day's ratio), `bench.resolve_rate.<dataset>` as of
the run it came from settling. `throughput_week` stays as of now — a count
over a window ending now is a current fact even when it is zero. This is
what lets a freshness window (a quality scenario's `max_age`) read a value
as stale at all.

**Scoring.** A key result is scored linearly from `baseline` to `target`,
clamped to `0.0..=1.0`, whichever direction the metric actually improves —
`factory_core::goals::score`. Colour bands differ for the two OKR
disciplines *Measure What Matters* names: a **committed** key result is a
promise, green only once fully met; an **aspirational** one is a stretch,
green once it is clearly winning (`≥ 0.7`). A key result nothing has
computed yet — no metric value, no check-in — carries `score: None`, never
a manufactured `0.0`: "unscored" and "scored red" are different facts.

**Check-ins.** A manual key result's value moves only through an
append-only check-in — `value`, `confidence` (`0..=10`), an optional note —
the same audit-trail pattern policy attestations use: recording one inserts
a row (`GoalsStore`, `crates/factory-daemon/src/goals/store.rs`), and
nothing ever revises or removes it. Refused when `kr` (`objective/kr`)
names no key result in any loaded cycle, when that key result is computed
rather than `manual: true` (its value comes from its metric, never a
check-in), when `confidence` is outside `0..=10`, or when `value` is not a
finite number. `Grant::GoalsCheckIn` (`goals.checkin`) gates it — the same
root-scope-only rule `policy.attest` follows: a check-in speaks for the
company's own goals, not for one project, so only the owner or a foreman
whose own scope *is* the instance root may record one.
`factory goals checkin <objective>/<kr> --value V --confidence C [--note
N]`, or `POST /api/goals/checkins`. `Event::GoalsChanged` publishes on
every check-in, the same as `Event::PolicyChanged` on an attestation.

**`goal=` labels.** A task serves a key result through the label
`goal=<objective>/<kr>`, following the existing `policy=` label pattern —
no new primitive. `goal_tasks_done` reads it back to count done tasks; the
agent guide names the objective and key result a task serves, when it
carries one, resolved from the goals catalogue once at dispatch.

**Scope.** `Request::Goals { scope, cycle }` narrows to objectives (and
roadmap items) whose own `scope:` is the asked scope or a descendant of it
(`Scope::path`, `Config::ancestors_of` — the same "roll up the subtree"
direction `Request::Policy`'s own scope filter runs); an objective with no
`scope:` of its own belongs to the root. `cycle` names one cycle by id
(refused if none loaded has it); left out, the current cycle (`now` inside
its `[from, to]` window) is used, or none if there isn't one right now —
every cycle still gets its own summary (id, window, status, score) either
way.

`factory goals [status] [--cycle C] [--scope S] [--json]` prints the
status view: vision/mission, the north star and its inputs, then per
objective its key results (value, target, score, band), then the roadmap
by lane, then findings. `factory goals cycles` lists every cycle on disk
with its own status and score. `GET /api/goals?scope=&cycle=` answers the
same `GoalsReport`.

## Scenarios

L6 Direction's third tab (`#100`): play out a what-if — a goal changes, a
regulation tightens, capacity drops — without ever touching the real
config. A **scenario** is an overlay, evaluated in memory against the same
policy evaluator, metric registry and goals catalogue Policy and Goals
themselves read; it never writes a scope's `.factory/config.yaml`, a real
policy catalogue, or the goals catalogue. `factory_core::scenario` (the
loader, the policy overlay, the seeded Monte Carlo forecast, the driver
tree, signposts) is pure and tested on its own; this section is what
`factory-daemon` builds on top of it, the same relationship `policies/mod.rs`
and `goals/mod.rs` have to their own pure modules.

**The catalogue.** `<root>/.factory/scenarios/<name>.yaml` — authored
content, hand-written, re-read on every request, never written by Factory,
the file's own `name` must equal its stem. See `examples/scenarios/` for
two complete ones and `crates/factory-core/src/scenario.rs` for every
field:

```yaml
# .factory/scenarios/ai-act-2027.yaml
name: ai-act-2027
title: EU AI Act applies to our agents from 2027
kind: [policy, drivers, goals, narrative]   # which sections this scenario uses
assumptions: |
  High-risk classification for the customer-facing product; human oversight
  required from the 2027 deadline.
from: 2026-09-01        # when the horizon below is measured from -- see "Staleness"
horizon: 26w             # Nd/Nh/Nw; defaults to 26 weeks
policy:                  # an overlay on the real chain -- add or tighten only
  add_frameworks: [ai-act]      # a real catalogue's framework, or a draft's (below)
  tighten: { cra/annex-i-2-1: { max_age: 14d } }
  drop_not_applicable: []       # controls whose n/a no longer holds in this scenario
drivers:                 # overrides on the built-in driver tree -- see "Drivers"
  capacity_factor: "×0.8"        # quoted: an unquoted +N/-N is a finding, not a guess
goals:                   # KR changes -- see "Goal scenarios"
  - { kr: ship-compliant/cra-open-zero, by: 2027-06-30 }
signposts:                # thresholds on registry metrics -- see "Signposts"
  - { metric: compliance.ai-act, below: 0.5, from: 2027-01-01 }
narrative:                # the qualitative workshop layer -- carried verbatim, never scored
  axes: [regulatory pressure, market demand]
  quadrant: tightening-fast
  drivers: [EU AI Act enforcement date, customer contract renewals]
  premortem: [We under-resourced human oversight and missed the deadline]
```

**Drafts.** `<root>/.factory/policies/drafts/<framework>.yaml` — a
framework not yet real enough for `.factory/policies/`, only for a
scenario's own `add_frameworks` to name (`policy::load_all`'s own
non-recursion into subdirectories keeps a draft from ever affecting the
real Policy tab by accident — see `examples/policies/drafts/ai-act.yaml`).
A draft whose `framework` collides with an already-loaded real one is
dropped whole from the merge and reported as a
`FindingKind::DraftCollidesWithReal` finding — a draft is a stand-in for a
framework that does not exist yet, never a second, competing definition of
one that already does; the real catalogue always wins.

**Three layers, never mixed into one number** (`scenario.rs`'s own module
doc comment, the issue's own guardrail):

- **Exact:** the policy delta. `overlay_chain` builds the overlaid chain
  from the real one (`Engine::policy_chain`, root through every ancestor);
  `policy::applicable`/`evaluate` run against it exactly the way the real
  Policy tab's own evaluation does — same evidence-gathering
  (`policies::Engine::dataset_level_facts`/`evidence_for_scope`, extracted
  from `policy_report` for exactly this reuse, so a scenario's overlay
  asking a check kind the baseline never needed — an `add_frameworks`
  draft, say — still gets its facts gathered lazily), same `Evidence`
  shape. Deterministic: a control is open or it is not.
- **Probabilistic:** the forecast, goal-scenario probabilities, and the
  tornado — always a p10/p50/p90 band or a ranked swing, never a single
  number, and always the same band for the same seed and inputs (see
  "Determinism").
- **Qualitative:** `narrative` — a 2×2's axes, PESTLE drivers, a
  pre-mortem — carried through verbatim. Nothing here scores or evaluates
  it; turning a pre-mortem point into a real signpost or driver override is
  an edit to the YAML, done by a person, not a computation.

**Determinism.** Every forecast and goal-probability call seeds a
from-scratch splitmix64 PRNG (`scenario::seed_from`, the first 8 bytes of
SHA-256 over the scenario's own name plus whatever it is projecting — a KR
reference, `"forecast"` — joined with a NUL separator) — never `now`, and
never `std`'s `DefaultHasher`, whose algorithm is unspecified and could
change between Rust releases. The same scenario file and the same
production history always produce the same bands.

**Throughput history.** The forecast bootstrap-samples
(Magennis's method — draw one historical week's throughput, with
replacement, per simulated week) from **26 non-overlapping weekly sums**
off `production.rs`'s own 53-week daily grid, ending today —
`THROUGHPUT_HISTORY_WEEKS`, chosen to match `Horizon::default()`'s own 26
weeks, so a scenario with no explicit `horizon:` draws from a history
exactly as long as what it projects forward. Deliberately *not* a bootstrap
sample of the `throughput_week` registry metric's own series: that series
is a *rolling* 7-day sum taken once per day, so any two points within six
days of each other share up to six of their seven days: resampling that
with replacement draws heavily autocorrelated "weeks" and understates real
week-to-week variance, producing bands that read falsely tight. A request
scoped to an ancestor still sees a descendant's own throughput:
`Engine::production`'s own scope filter is exact-match only, so
`Engine::subtree_daily` sums every scope in the asked subtree's own daily
grid, element-wise, rather than asking `production` once for the ancestor
alone.

**Baseline.** What every scenario in a report is compared against,
computed once and shared: the metric values any scenario's drivers,
signposts or goal changes reference; the driver tree's own baseline values
(a registry-backed driver's current metric value; `capacity_factor`'s
neutral `1.0`, the same default `evaluate_outcomes` itself falls back to
when a driver is absent); the current policy rollup over the asked
subtree; and a baseline forecast — `forecast_completion` over the same
throughput history, backlog = every non-terminal task in the subtree right
now, `Horizon::default()`'s 26 weeks. Chosen over a bare metric trend so it
is the *same* `Forecast` shape every scenario's own forecast carries, and a
fan chart can draw the baseline band and a scenario's band on one axis.

**Drivers.** A small, built-in tree tied to the metric registry where one
exists: `throughput_week`, `first_pass_yield`, `scrap_rate`, `rework_rate`
(registry-backed); `capacity_factor` (an assumption — no data source, a
person's own what-if); `unit_cost`, `tokens_per_run` (named, but **unavailable** —
design §12.6, a `Run` records no cost yet; only `=N`, a pure assumption
needing no baseline, may override one). The one v1 formula:
`effective_throughput = throughput_week × capacity_factor × first_pass_yield`.
An override is authored `×2`/`x2` (multiply), `+20%`/`-20%` (percent
change), `+5`/`-5` **quoted** (delta — YAML reads a bare `+5` as an
integer, losing whether it means a delta or an absolute assumption, so an
unquoted one is a finding, not a guess), or `=0.9` (set, the only variant
that needs no baseline). The tornado varies each driver ±20% one at a time
and ranks the effect on `effective_throughput` — v1's only computed
outcome, so "the scenario's key outcome" has nothing else to name yet.
Driver overrides reach the forecast by scaling, not by replacing, the
throughput history: `effective_throughput` after ÷ before is the factor
every point in the history is multiplied by, so the forecast's bands keep
the real history's own week-to-week shape, just scaled.

**Backlog.** A scenario's own backlog — what its forecast has to clear —
is `newly_open` controls from its subtree-wide policy delta (one
remediation item each) plus every non-terminal task labelled
`goal=<objective>/<kr>` for one of its own `goals:` entries.
`newly_stale` is deliberately excluded: stale evidence needs refreshing,
real work, but a different kind from a from-scratch remediation, and
folding it into the same count would make "backlog" mean two different
sizes of thing at once — `newly_stale` is still visible in the delta
itself.

**Goal scenarios.** Each `goals:` entry re-scores a key result against a
changed `target` and/or `by` (whichever the entry itself sets; left
unset, the key result's own authored `target` or its cycle's own end date)
using the same scaled throughput history the scenario's own forecast uses.
Only a **count-like** key result (`KrShape::CountLike` — a `Unit::Count`
metric whose changed target is *above* the current value, a backlog to
clear) ever gets a probability; a **ratio** key result (every
`compliance.*`, `first_pass_yield`, …) has no rate model to project a
ratio's future value from a throughput history, so it always comes back
`probability: None` with a reason — never a fabricated number. A manual
key result (no bound metric) is always treated as `Ratio` for the same
reason: there is no metric id to read a direction from.

**Signposts.** A threshold (`below`/`above`, either or both) on a registry
metric, evaluated on every read against the same values the baseline
reads — `Quiet`, `Triggered`, `NotYetActive` (before its own `from`), or
`NoData`. Never itself starts, stops, or gates anything (design §8). A
triggered signpost is meant to be visible outside the Scenarios tab too —
on the dashboard, in the inbox, as an **observation**, never an automatic
consequence. The Inbox (`ui/js/dashboard.js`'s `inboxItems`) is built
entirely client-side, off the task list alone, with no daemon-side inbox
aggregate to add to; instead, `ScenariosReport::triggered` flattens every
currently-`Triggered` signpost across every scenario, named alongside the
scenario it belongs to, so a dashboard or inbox reader does not have to
walk every card itself. The L6 Scenarios UI slice
(`ui/js/{scenarios,scenarios-model}.js`, not part of this slice) is
expected to read this field and render it on the dashboard.

**Promote.** `factory scenario promote <name> --scope S [--agent A]`
creates one ordinary task per newly-open control in `S`'s own slice of the
scenario's policy delta, through the exact path `factory task create`/
`factory policy remediate` themselves use (`Engine::create`) — never a
second, hand-rolled door. Titled `Prepare <fw>/<id> for scenario <name>:
<control title>`, labelled both `scenario=<name>` and `policy=<fw>/<id>`,
instructions from the control's own `remediation:` text plus its missing
evidence (`policy::remediation_instructions`, the same text `policy
remediate` itself writes). Skips — never refuses — a control that already
has a non-terminal task carrying that `policy=` label in `S`: a promote
names many controls at once, and one of them already having an open task
is the ordinary case, not a mistake to stop the whole action over. Needs
`task.create` in `S`, the exact reach rule `factory task create` itself is
checked against. `POST /api/scenarios/promote` answers
`{"kind":"scenario_promote","result":{scenario,scope,created,skipped}}`.

**What-if.** `POST /api/scenarios/whatif` (`{scenario?, drivers}`)
recomputes driver outcomes, the tornado and the forecast with slider
overrides applied server-side — pure and read-only, meant for a driver
panel to call on every slider change, debounced client-side. Layering: the
named scenario's own `drivers:` overrides (none, with `scenario` absent),
then the request's own `drivers` on top, request wins driver by driver, so
a slider can override one driver a scenario also names without resending
its other overrides. Each entry parses with the same authored syntax
(`×2`, `+20%`, `+5`, `=0.9`); a value that does not parse is refused
outright (typed input from a live request, not an authored file `load`
can leave partly wrong and still serve the rest of). Backlog: with
`scenario` named, the same subtree-wide policy-delta and goal-task backlog
the report itself computes for that scenario, over the whole instance
(this request carries no `scope`) — which means this endpoint recomputes
the policy delta on every call even though backlog genuinely does not
depend on which driver moved; the UI is expected to debounce rather than
this pretending backlog is free. With no `scenario` at all, backlog is
`0.0` and the forecast is honestly a bare throughput projection with
nothing to clear.

**Never writes config.** `factory scenarios`, `factory scenario show` and
`POST /api/scenarios/whatif` are fully read-only. `factory scenario
promote` writes ordinary tasks and nothing else — never a scope's own
`.factory/config.yaml`, never a scenario file, never a real policy
catalogue. A scenario is data a person authored and Factory only ever
reads; turning one into real work is always the one explicit,
owner/agent-driven `promote` action (design §8).

`factory scenario [list] [--scope S] [--json]` prints the board: the
baseline (forecast, policy rollup), one summary line per scenario
(backlog, forecast, triggered signposts), every currently triggered
signpost, then findings. `factory scenario show <name> [--scope S]
[--json]` is one scenario's full detail: forecast, drivers (baseline vs.
overridden, outcomes, tornado), policy delta (subtree and per scope), goal
scenarios, signposts. `GET /api/scenarios?scope=` answers the same
`ScenariosReport` both read from.

## Quality attributes

L6 Direction's fourth tab (`#107`): which qualities matter for each scope,
how much, what they trade off against, and whether they are being met —
**measured, never claimed**. Goals say where the company is heading and
Policy which external rules it follows; quality attributes say how *good*
the thing has to be on the way. `factory_core::quality` (the compiled-in
ISO/IEC 25010:2023 catalogue plus the ISO/IEC 25059 AI pack, the profile
loader, the add-or-tighten merge and the evaluator) is pure and tested on
its own; this section is what `factory-daemon` builds on top of it.

**Profiles.** `<root>/.factory/quality/<profile>.yaml` — authored content,
hand-written, re-read on every request, never written by Factory; the file
stem is the profile's id. A profile is an ATAM utility tree: at most seven
attributes, each an ISO 25010 id (`characteristic` or
`characteristic.sub-characteristic`) ranked for importance and difficulty
as H/M/L only, each with SEI six-part scenarios. See `examples/quality/`
for two and `crates/factory-core/src/quality.rs` for every field:

```yaml
# .factory/quality/daemon-service.yaml
attributes:
  - id: reliability.recoverability
    importance: H                    # H | M | L -- never finer
    difficulty: M
    scenarios:
      - id: daemon-restart
        kind: usage                  # usage | change
        source: launchd
        stimulus: daemon restarted while 3 runs are active
        artifact: factory-daemon
        environment: normal operation
        response: runs resume reporting; no run is lost or double-dispatched
        measure:                     # the response measure; without one, a draft
          metric: fail_rate          # continual: a registry metric and a threshold
          below: 0.05                # above/below, inclusive; both is a band
          max_age: 7d                # how old the value may be before it reads stale
  - id: maintainability.modifiability
    importance: H
    difficulty: H
    scenarios:
      - id: agent-diff-health
        measure: { check: task, task: quality-gate, max_age: 7d }   # triggered: a Policy check, as a catalogue writes it
tradeoffs:
  - between: [security.confidentiality, performance-efficiency.time-behaviour]
    point: every agent runs in a sandbox; start-up cost accepted
    decision: knowledge/adr-sandbox.md
```

**Declaration.** The instance root's top-level `quality: [profile, …]`
applies everywhere; a scope's own `scope.quality: [profile, …]` adds
profiles for itself and every scope below it by path — the policy chain
exactly (`Config::quality_chain_for_scope`, ancestry by `Scope.path`, never
by name). Down the chain a descendant may only **add** (attributes,
scenarios) or **tighten** (raise importance, raise `above`, lower `below`,
shorten `max_age`); anything looser is a finding and the inherited value is
kept. A scope whose chain binds nothing is left out of the report: an
undeclared attribute is "not a stated concern", never "failing".

**Statuses.** Each scenario is `met`, `not_met`, `stale`, `no_data` or
`draft`; an attribute is the **worst** of its scenarios. There is no score
anywhere — per scope or company-wide — because an average is how a failing
H attribute hides behind three green L ones.

- A **metric** measure reads the metric registry (the same
  `Engine::metrics` Goals and Scenarios read). An unknown or unavailable
  metric (`unit_cost`) is `no_data` with the registry's reason; a value
  whose `as_of` is older than `max_age` is `stale`, never green.
- A **check** measure is judged by `policy::evaluate` itself, as a
  one-control catalogue, on evidence gathered by the Policy tab's own
  `dataset_level_facts`/`evidence_for_scope` — lazily, only for the check
  kinds some scenario asks. `task`/`workflow` name one in the evaluated
  scope; a name that matches more than one is an `ambiguous_check_target`
  finding. "Never gathered" and "nothing finished yet" are `no_data`, not
  `not_met`.
- An **`attestation`** check is `no_data`, with the reason. The attestation
  store keys a row by a policy `ControlRef` it parses back on every read,
  and a quality scenario's `quality/<attribute>/<scenario>` does not parse,
  so there is no honest way to record one yet. Measure such a scenario
  with a task, workflow or gate check instead.
- A measure naming a `quality.*` metric is a `self_referential_metric`
  finding and always `no_data`: it would be computed from the scenario
  being judged.

**`quality.<characteristic>`.** The registry's own metric for a Goals key
result or a Scenario signpost to target: of every declared scenario under
one ISO 25010 characteristic, counted once per scope it applies in across
the whole instance, the share that is `met`. Drafts and `no_data` count
against it — declared but not shown to be met is not met. `None`, with the
reason, when nothing is declared under it (that is not "all met").
Company-wide only: a scope name may hold `/`, which a metric id segment
cannot, so there is no per-scope parameter. `Engine::metrics` computes the
metrics a quality evaluation reads in the same pass as the ones it was
asked for, so `production`/`policy_report` are read once per call and
nothing loops; if the quality evaluation fails, only the `quality.*` values
come back `None` with the error as reason — a dashboard or Goals read never
fails over it, and never publishes `quality_changed`.

**The agent guide.** A task's run is told its scope's H-importance
attributes, one line each: every scenario's measure in words, and a status
word (`not met`, `stale`, `no data`) only when it is not met — never a value
or a time, so the guide stays byte-for-byte the same from one dispatch to
the next while nothing moved. Judged once at dispatch, like the policy
frameworks line, and reused for 60 seconds per scope while the profiles'
fingerprint holds — single-flight, so a burst of dispatches into one scope
judges it once. A scope with nothing ranked H gets no block at all, and a
profile that cannot be read never stops a dispatch (a failure is never
cached). A standing agent's
guide carries no block: it is written once for the agent's whole life, and
any status in it would soon be stale.

**Remediation.** `factory quality remediate <attribute>/<scenario> --scope
S [--agent A]` (or `POST /api/quality/remediate`, `{scope, attribute,
scenario, agent?}`) creates an ordinary task through the exact path `factory
task create`/`policy remediate` use, labelled
`quality=<scope>/<attribute>/<scenario>`, with the scenario's six parts, its
measure and why it is not met as instructions. It needs `task.create` in
`S`. It is refused for a `met` scenario, for a `draft` (with no measure
there is no gap, only a measure to write), and for a `no_data` scenario no
task could ever give data to — an `attestation` check, or a `quality.*`,
unknown or unavailable metric — where the fix is an edit to the profile.
Checking for an open task and creating one are not atomic, the same as
`policy remediate`: two calls racing can both create. When a non-terminal task with
that label is already open in `S`, that task comes back with `created:
false` and nothing new is made (`#98`); the report's `open_tasks` names it
per scenario so a reader can show it up front.

**`quality_changed`.** Goals and Policy publish their events on their own
writes; quality has none (a remediation task already fires `TaskCreated`),
and nothing in Factory watches files. So every report fingerprints the
profiles and every scope's chain it loaded, and publishes
`Event::QualityChanged` when that moved since the last read — the issue's
"on the next read". Only a successful `Request::Quality` records it, under
one lock with the comparison, and a read that loaded its profiles earlier
than the recorded one never overwrites it. The first read after a start
publishes nothing.
Evidence changing (a fitness-function task finishing) is not this event;
it arrives as `RunUpdated`.

**Enforces nothing.** A quality attribute starts, stops and blocks nothing
(design §8). Whether a fitness function gates a workflow is that
workflow's business.

`factory quality [status] [--scope S] [--json]` prints one line per scope
and attribute — (importance, difficulty) and its rollup. `factory quality
scope <name>` is one scope's whole tree: every scenario with its measure,
status, reasons and any open remediation task, then its trade-offs.
`factory quality findings [--scope S]` lists what is wrong with the
profiles or the chains that bind them. `GET /api/quality?scope=` answers
the `QualityReport` all three read from: per scope, its evaluated tree and
open tasks; the findings; the nine-characteristic catalogue (every column a
heatmap draws); and the history of any metric a scenario reads, for a
sparkline.

## Operations

L4 Process's third tab (`#106`), next to Tasks and Workflows: how the line
is running, exception first. **A picture, not a controller** (design §8):
nothing here starts, stops or retries anything on its own; the only new
lever is that a person can pause a schedule. `factory_core::operations` is
the pure model -- every word below is defined there, and tested on its own;
`crates/factory-daemon/src/operations.rs` gathers what it reads.

**The projection.** `GET /api/operations?scope=&window=7d|30d[&detail=charts]`
and `factory stats` answer one `OperationsReport`, computed on read from the
store like `/api/production`, with no store of its own. A scope means that
scope and every scope nested under it -- the subtree Policy and Scenarios
read too -- so a parent shows its children's work:

- `attention` -- what needs a human now, most severe first, then oldest.
- `flow` -- per scope, work in flight by state (queued, dispatching,
  running, blocked), queue depth, queue wait p50/p95, sessions in use and
  how many runs are waiting on a retry. Nothing in Factory limits a scope's
  sessions -- the `max_sessions` older configs carry is read and ignored --
  so `sessions_max` is absent, never a made-up limit.
- `aging` -- every run in progress with its age against the p50/p70/p85/p95
  of the same task's finished runs (five or more), else its scope's, else
  "not enough history" and no colour at all.
- `health` -- over the window and the one before it: throughput per day,
  cycle time p50/p85, first-pass yield, rework, scrap and fail rates,
  scrap by `fail_kind`, time to recover, queue wait, and interventions per
  100 runs. A figure that cannot be computed says why instead of reading
  zero, and one read off a fact older runs do not record says from when it
  is on record.
  With `detail=charts` (the tab asks; the Inbox and `factory stats` do
  not), each window also carries `days` -- its 24-hour steps, oldest first, with
  the counts its figures are made of and what stood waiting and in
  progress at each step's end, the step ending now counting the queue --
  and the current one `finished_runs`, the
  newest 2000 runs that finished in it with each done run's cycle time: the
  tab's small multiples, cumulative flow diagram and cycle-time scatter.
- `schedules` -- every scheduled task: due, late, missed or paused, with its
  timezone.

It reads runs overlapping the last 60 days (or two windows, if longer) --
open runs included, plus the newest run of any failed task none of whose
runs fall in that span, so a task that failed with no retry left stays in
the queue however long ago it failed -- open runs' own journals for their last word and a
blocked run's reason, `schedule_skipped` entries from the last day, and
answers and run requests over both windows (`TaskStore::entries_of_kinds`,
one query rather than a walk over every journal; a store that cannot search
returns nothing). Triggered signposts are reused for a minute while the
scenario files are unchanged -- computing them runs the metrics they name.

**Exceptions.** Only what a person can act on:

| kind | when | severity |
|---|---|---|
| `blocked` | the run is `Blocked`, said by its agent or a lifecycle hook; the reason is its own words | high |
| `suspected_stuck` | the runtime suspects the run is waiting, or it has said nothing past the p95 of its history -- always marked a **suspicion**, never a status | medium |
| `failed_exhausted` | the newest run failed and no retry is left; a failing run that will be retried is a `retrying` count in Flow instead | high |
| `aging` | running past the p85 (medium) or p95 (high) of its history | medium/high |
| `schedule_late` | a slot passed two ticks ago and nothing was dispatched | medium |
| `schedule_missed` | slots the scheduler passed over (`schedule_skipped`) in the last day | medium |
| `liveness_lost` | a **permanent** agent's session is gone -- never judged on being quiet | high |
| `triggered_signpost` | a scenario signpost is past its threshold -- an observation, on the unscoped report only | low |

A run appears once, as its most severe kind, with any other kinds it matched
in `also`. Each exception lists the actions it allows.

**Actions.** Each is an existing request, or a narrow one beside it, and
each is journaled with who asked (`source: owner` or `agent`, `data.by`) and
why (`data.reason`, when given):

```sh
factory task run <id> --reason "..."          # run again; journals run_requested
factory task cancel <id> --reason "..."       # journals cancel_requested; counts as scrap
factory task edit <id> --pause-schedule --reason "stop the line"
factory task skip-next <id> --reason "..."    # task.skip_next; journals slot_skipped
factory run answer <run-id> "text" --reason "..."   # run.answer
```

`task.cancel` takes an optional `run` (`run_id` over HTTP): the attempt the
caller was shown. If another run is the task's active one by then -- a
retry that started in the meantime -- the cancel is refused and nothing is
ended.

`task.skip_next` moves the schedule to the slot after the next one -- or the
first one after now, when the next has already passed. A queued retry is
the next thing that would fire, so skipping it ends the streak and brings
back the regular slot it stood in front of (or the first one after now, if
that has passed too). An optional `slot` names the firing the caller means
to skip; if the schedule has moved on since, the skip is refused. Skips and
the scheduler's firing take one lock and re-read the task under it, so a
slot is fired or skipped, never both. It is journaled as
`slot_skipped`, never as `schedule_skipped`: a person's decision is not a
missed slot. `run.answer` types the text into the blocked run's own session
and presses enter; it is refused unless the run is `Blocked` and has a
session, the reason is required, and only the reason is journaled
(`answer`), not the text -- as soon as the text is in the session, so a
keypress that then fails (journaled as `answer_unsent`) never leaves typed
text off the record. The run stays blocked until its agent reports
otherwise; an agent that unblocks itself in the moment between the check
and the typing gets the answer in whatever it is doing next. Over HTTP: `POST /api/tasks/{id}/run`, `.../cancel` and
`.../skip-next` take an optional `{"reason": ...}` body (skip-next also
`slot`), `PATCH
/api/tasks/{id}` a `reason` beside the patch's fields, and `POST
/api/runs/{id}/answer` `{text, reason}`. Skipping is `task.edit`, like
pausing; answering is `run.input`, with the same reach -- no new grants.

**Interventions** are what the record shows the owner doing: a manual run
of a task whose previous attempt failed or was cancelled, a cancel by the
owner, and an answer given through `run.answer` by the owner. A run again
or a cancel an agent asked for is not one -- `task.run` journals who asked,
and an agent's cancel is `cancelled_by_agent`. An agent that leaves its
token out *is* the owner to Factory, though, so the count can overstate
what people did: treat it as a ceiling. (Text typed straight into a run's
terminal, not through `run.answer`, leaves no record and is not counted.)

**Live updates.** No event of its own: everything the report reads changes
through `task_updated`, `run_updated`, `task_entry` or `agent_updated`, and
a viewer re-reads on those.

**The tab.** Top to bottom: what changed since this browser last looked
(kept in `localStorage`, a convenience and never a record); the attention
queue, each row with its allowed actions behind a confirmation that takes a
reason (required for an answer), and bulk run again / cancel only after a
preview of every row; flow per scope, with capacity shown as unknown and a
link to Occupancy; Vacanti's Aging WIP chart per scope; process health
7d|30d as small multiples over the previous window's ghost, with the
scatter and the CFD behind toggles; and the schedules. Every chart has a
table twin. One read per load, narrowed by the daemon to the rail's
selection and its subtree. The Dashboard's Inbox is the same attention
list, every scope, less observations, read without the charts' detail. A paused schedule carries a `paused` badge wherever a
scheduled task is drawn.

`factory stats [summary] [--scope S] [--window 7d|30d] [--json]` prints the
attention queue's first five rows, flow, aging, health against the previous
window, and schedules; `factory stats attention [--scope S]` prints every
exception with its reason and actions.

## Tasks and runs

A **task** is the standing intent: what to do, where, with which agent, and on
what schedule. A **run** is one attempt at it — the session it opened, what it
reported, when it ended.

Running a task a second time makes a second run. A task that failed and is
started again has two runs, numbered `attempt 1` and `attempt 2`, and both are
kept with their own journal, their own outcome, and their own terminal
transcript. The task itself mirrors the newest run, so a list stays cheap to
read; the history lives on the runs.

Everything a task carries can be set when it is created and changed afterwards
— scope, agent, schedule, labels, and how patient the daemon is with it:

```sh
factory task create "nightly sweep" -i "..." \
  --scope demo --agent assistant \
  --schedule "0 3 * * *" --timeout 1800 --ack-timeout 120 --label area=infra

factory task edit <id> --agent scratch --schedule "every 15m"
factory task edit <id> --no-schedule --default-timeouts
```

`--ack-timeout` is how long the agent has to say it has started; `--timeout` is
how long the whole run may take. Both fall back to the instance defaults.

A cron schedule's fields are read in UTC unless `--timezone` names an IANA
zone, in which case they are that wall clock's: `--schedule "0 9 * * 1"
--timezone Europe/Berlin` is nine o'clock in Berlin every Monday, in summer
and in winter, and its UTC firing moves with the clock change. On the two days
a year the clock jumps, a firing that falls in the spring-forward gap runs at
the first minute after it, and one in the autumn overlap runs once, not twice.
`--timezone` goes with `--schedule` — an edit restates the whole schedule —
and a name Factory does not know is refused when it is set, not discovered
when it fails to fire. An `every` schedule is an interval and takes no
timezone.

```sh
factory task edit <id> --pause-schedule    # keep the schedule, fire nothing
factory task edit <id> --resume-schedule   # next firing counted from now
```

A paused schedule keeps its expression and the slot it would have fired, and
nothing -- neither a slot nor a queued retry -- fires until it is resumed.
Resuming counts the next firing from the moment of resuming, so the slots
that passed while paused are not caught up in a burst. The same holds for
slots that pass while the task's previous run is still going, or while the
daemon is down: the overdue slot fires once, late, and the journal records the
ones after it as a single `schedule_skipped` entry with how many, the first
and the last.

```sh
factory task run <id>          # a retry is just another run
factory run list <id>          # every attempt, newest first
factory run show <run-id>
factory run log <run-id>       # that attempt's journal
factory run output <run-id>    # its terminal, live or from the transcript
```

Only one run of a task can be in progress at a time — two attempts at once
would race for the same working directory — so starting a second is refused
until the first ends or is cancelled.

## Workflows

A workflow is reusable Process-level intent: a scoped, finite DAG of ordinary
task templates. A workflow run keeps an immutable snapshot of the definition
revision it started with. Root nodes create and run tasks immediately; every
other node waits until all incoming predecessors have reported `done`.
Fan-out starts every newly eligible node and fan-in waits for every parent.
A failed or cancelled task stops the attempt and leaves downstream nodes
`skipped`; a blocked task simply pauses it. The task remains authoritative for
all of these states, including after the run itself has an outcome: a sibling
still running when the run fails keeps moving to its own `done`/`failed`/
`cancelled` rather than freezing, and a task deleted out from under an active
node fails that node truthfully instead of leaving it pending forever.

Workflow definitions and runs are daemon orchestration state. They are stored
in additive tables in the instance root's `.factory/factory.db`, never in a
scope config or browser storage, even when that scope uses a plugin task store.
Each spawned task carries `workflow_origin` with the definition, run, and node
IDs. The run persists its chosen task ID before task creation, so restart
reconciliation recreates that exact decision instead of spawning a duplicate.
A row neither table can decode is skipped (and named in a warning) rather than
failing the whole list or recovery pass; fetching it directly is still an
error, but only for that one id.

**A `workflow.run` grant is not a way to launder the caller into the owner's
authority over tasks.** A run remembers who started it (the owner, or a scoped
agent) and re-checks that actor's *current* role -- not a snapshot of what it
could do at the moment it clicked Run -- against the same `task.create` and
`task.run` authority a hand-typed request would need, every time a node
spawns: the first preflight before anything is persisted, and again at every
later spawn a downstream fan-out or a restart recovery makes. A role that has
since lost the grant fails just that node (recorded as its error) rather than
the run silently keeping the authority it started with.

The web UI exposes **Workflows** beside **Tasks**. Its canvas supports moving,
connecting, duplicating and deleting task nodes, with pan/zoom, zoom/fit
controls, and a properties inspector. A Design/Run toggle switches between
editing the live definition and viewing one specific run: Run mode renders
that run's own immutable `definition` snapshot with live status overlays,
read-only, so a canvas edited since a run started is never what the run
appears to be doing. A "Recent runs" list beside the definitions picks which
run Run mode shows; starting a run switches to it. The ordered textual
summary and keyboard node/edge controls carry the same graph for people who
do not use the canvas, including a link to any node's spawned task.

A node's task is dispatched with its **direct parents'** outputs, never a
transitive ancestor's — computed at dispatch from that moment's workflow-run
state, so it survives a restart without `recover_workflows` needing to know
about it. A harness agent (`claude-code`, `pi`, `codex`, `opencode`) gets a
labelled "Output from the workflow steps this task follows" section in its
prompt, one entry per parent with its title, node/task id, and result (or a
plain "no result reported"). The `shell` agent instead writes the same
outputs as JSON to a file and exports its path as `FACTORY_UPSTREAM_FILE`, so
a downstream command can do `cat "$FACTORY_UPSTREAM_FILE"` to see what its
parents said — the acceptance bar is that literal command. Either way, each
parent's result is tail-truncated to a byte budget first, so one noisy
upstream step can't blow up every prompt downstream of it.

## Compliant workflows

A run used to be `done` the moment its own agent said so. Policy controls and
quality attributes already *state* what good work includes -- an SBOM, a
security scan, tests -- but nothing obliged a run to go through those steps or
to prove it had (`#118`). Now a control, or a quality attribute, can say which
steps a **category** of work must pass, and the line enforces it: a run that
owes a step is only `done` once the step has left evidence, produced by the
daemon rather than by the agent that did the work.

```yaml
# .factory/policies/house.yaml -- a control's `requires:`
- id: tested
  title: Changes are tested
  requires:
    - { applies_to: [feature, bugfix], step: tests, gate: "cargo test --workspace" }
    - { applies_to: [release], step: sbom, gate: "make sbom", before: publish }
    - { applies_to: ["*"], step: lint, gate: "cargo clippy -- -D warnings", timeout_seconds: 900 }
```

The same `requires:` list goes on an attribute of a quality profile
(`.factory/quality/<profile>.yaml`), where it is named `quality/<attribute>`.

- **Categories.** A task (`factory task create --category feature`) or a
  workflow (`category:` on its definition, overridable per node) says what kind
  of work it is. Leaving it out is the category `default`, never "no plan":
  `applies_to: [default]` catches uncategorised work, and `*` catches every
  category. A category is a name (`[a-z0-9][a-z0-9_-]*`); nothing else is
  checked.
- **The control plan** for a scope and category is resolved from what already
  applies there, down the same chain `policies:` and `quality:` use: add or
  tighten only. A scope adds a framework and with it the framework's
  requirements; the same step and command required twice is one step naming
  both controls; the same step with two commands runs both; a timeout only
  shortens. The one way a requirement leaves the plan is an `n/a` with a
  rationale on its control, and the plan lists that as a waiver.
- **Injection at dispatch.** When a workflow run starts, each task node's plan
  is merged into the run's immutable snapshot as **locked `gate` nodes**,
  chained after that node in the plan's `before:`/`after:` order; whatever
  followed the node now follows its last gate. Gates go after *every* task
  node, not only the last ones, because every node works in a worktree of its
  own. An authored gate node for the same step satisfies the requirement
  instead. A standalone task is planned as an implicit one-node workflow
  through exactly the same injection. The stored definition never changes, and
  a run keeps the plan it started with: the steps are fixed on the run at
  dispatch (`Run::required_steps`), and the agent is told them in its
  reporting contract.
- **`verifying`, and the `done` gate.** A run with required steps that reports
  `done` becomes `verifying`. The daemon runs each gate itself -- the bench
  gate runner, `sh -c` in the run's worktree (the scope directory for a
  `--no-worktree` task), exit 0 passes, ten minutes unless the requirement
  says otherwise -- in plan order, stopping at the first failure. Every gate
  appends an **attestation**: the step, who ran it (`factory-daemon`), the
  verdict, exit code, output tail, and the commit and dirtiness of the tree
  it judged. The run is `done` only when every required step has a passing
  attestation from this round and not from the executing agent. Otherwise it
  goes to **`blocked`**, with the reason as its journal's block line -- so it
  is in the Inbox, answerable like any other block. Its session stays open:
  the agent fixes the problem, reports `done` again, and the gates run again.
  While a run verifies the agent may add a note or give the run up, nothing
  else; the run timeout and the session-gone check do not apply to it, and a
  restart verifies it again from the start. Bench attempts are not planned --
  their own case gate already judges them.
- **Gate nodes mirror, never execute.** In a workflow run, a gate node's status
  is read off its subject run's attestations; it never spawns a task, and a
  node downstream of it starts only once the work the gate judged is itself
  `done`.
- **`factory workflow lint`** is the author's preview: the effective plan per
  category, which steps a run would get injected and which authored gates
  already satisfy one, waivers, findings (a gate step with no command -- it
  can never pass, so every such run blocks), and ordering violations (a
  `before: publish` whose `publish` node can start with no scan before it).

```sh
factory workflow lint <workflow-id>                 # what a run of it gets
factory workflow lint --task <task-id>              # a task, as its one-node workflow
factory workflow lint --scope demo --category release
factory run attestations <run-id>                   # the evidence a run carries
```

The same over HTTP: `GET /api/workflow-lint?workflow=|task=|scope=&category=`
and `GET /api/runs/{id}/attestations`. Attestations live in the append-only
`run_attestations` table next to `policy_attestations`.

**Not in v1.** `review` and `approval` steps parse and show in the plan and in
`lint`, marked not enforced -- they need a functionary other than the daemon
(another agent, a person), which is v2 along with a rework task proposed on a
failed gate and a canvas that draws injected nodes distinctly (the Workflows
canvas only labels them `GATE 🔒` today). The `attested` policy check and the
conformance metrics are v3.

## How a task actually runs

1. `task.create` resolves the scope, agent, and runtime — from the request, then
   the scope's local config, then the instance defaults — and refuses right away
   if any of them names an adapter that does not exist.
2. `task.run` opens a run, mints a callback token for it, and asks the runtime
   for a session in the scope's directory.
3. The agent adapter produces the prompt and, through the harness's own
   system-prompt mechanism, injects a short guide to Factory itself — what it
   is, who this agent is, and which commands its role allows. The prompt
   itself stays narrower: the task, the working directory, and the reporting
   contract — the exact commands the agent is to run. The guide never repeats
   those; it just says where to find them. The same values are in the
   session's environment as `FACTORY_TASK_ID`, `FACTORY_TASK_TOKEN`,
   `FACTORY_SOCKET`, and `FACTORY_BIN`.
4. The agent runs `factory task report <id> --status running …`, then finishes
   with `done`, `failed`, or `blocked`. The report lands on whichever run of
   that task is in progress; the token says it is that run's agent speaking.
5. On a terminal report the daemon keeps the last of the terminal output as a
   journal entry on the run and closes the session.

**Status comes from the agent, not from the terminal.** A runtime can say
whether a session is alive; it cannot say whether the work is finished, and
anything that reads that from a terminal's appearance will be wrong sometimes.
Runtime status is used only to notice sessions that died -- with one carved-out
exception. Some runtimes (herdr, for `pi`) have a harness that tells them
directly, through a lifecycle hook, that the agent is waiting on a human: that
is a report, not a guess, so the daemon trusts it enough to mark the run
`Blocked` on its own, with no callback from the agent at all. The same runtime
guessing `blocked` from the screen's appearance -- what it does for `claude`,
`codex` and `opencode` -- is not trusted the same way: it is recorded as a
suspicion the UI can show, and it changes no status and stops no timeout.

The same carve-out covers a turn that ended without a report -- an agent whose
session went quiet mid-task, or whose response an API error cut off. For `pi`,
herdr's own hook says the session is idle, and the daemon fails the run on the
next tick. herdr has no hook for `claude`, only a guess from the screen, so a
`claude-code` run carries its own: Factory writes a per-run settings file
(passed as `--settings`, never into the worktree) with Claude Code `Stop` and
`StopFailure` hooks that call `factory task turn-ended` the moment the turn
ends. They are `async`, so they never hold a turn up, and Claude Code merges
them with the worktree's own `.claude/settings.json` hooks rather than
replacing them. A `StopFailure` -- an API error, which no hook can override --
fails the run at once. A `Stop` is only held on the run, because any other
`Stop` hook in the session (the scope's own, or the user's) may keep the turn
going; it stands after 30 seconds, on a tick where the session also looks
idle, unless the agent reports something first. The screen can hold that back
but never trigger it. Either way the run is left alone if it is `Blocked`
(waiting for a person is what it was told to do) or if Claude Code still has a
background task or session cron that will wake it; otherwise it fails with a
reason saying which hook fired and, for an API error, what the API said.
`codex` and `opencode` have neither, and still answer to the timeouts below.

Three timeouts catch the rest:

- `ack_timeout_seconds` (180 by default) — the agent is up but has not said a
  word. This is what an agent sitting on a first-run trust prompt or a login
  looks like.
- `task_timeout_seconds` (3600 by default) — a cap on the whole run, counted
  from `started_at`: the run has not finished within this many seconds,
  however often it has reported in between.
- `blocked_timeout_seconds` (86400 by default) — a run a hook reported
  `Blocked` is exempt from the two above and given this much longer clock
  instead, counted from when the block began rather than when the run did, so
  a person has a real chance to see it and answer before the daemon gives up.

None of the three above notice a host that cannot execute at all. A run's
`task_timeout_seconds` is spent by wall clock, and wall clock keeps moving
while a sleeping host has nothing running -- so `power_assertion` (on by
default) holds an OS-level "do not sleep" assertion for as long as any run is
active, released the moment the last one ends. macOS only for now
(`caffeinate`, behind a seam so every other platform reads the setting and
no-ops); the host-level power settings that let this happen in the first
place are a separate fix, and this is only ever the defence-in-depth for the
moments that one does not reach.

## Configuration

The instance root's `.factory/config.yaml` owns daemon-wide settings and may
also configure the root directory as a scope. Daemon state — the database,
socket, plugins, and task worktrees — stays in this root `.factory/`.

```yaml
version: 1
instance:
  id: 7138c969-31ce-46bc-9f10-677d1f85c457
  name: my-company

daemon:
  task_store: sqlite
  interfaces:
    - kind: cli              # unix socket at .factory/factory.sock
    - kind: http
      bind: 127.0.0.1:8787   # 0.0.0.0:8787 to reach it from the network
  tick_seconds: 5
  ack_timeout_seconds: 180
  task_timeout_seconds: 3600
  blocked_timeout_seconds: 86400
  default_agent: claude-code
  default_runtime: herdr
  power_assertion: true        # hold the host awake while a run is active

infrastructure:              # optional: the AI accounts behind the agents
  providers:
    - name: claude-max
      vendor: anthropic
      kind: subscription       # subscription | api-key
      plan: Max 20x            # free text, optional
      harnesses: [claude-code] # default binding for every agent on these

scope:                       # optional: make the instance root a scope too
  id: cc23161d-82b9-4e75-8d88-e5195bc6d6e8
  name: root
```

Every other scope owns a `.factory/config.yaml` in its own directory. That file
is both the opt-in marker discovery looks for and the source of the scope's
stable ID, name, agents, and overrides. Its path comes from the directory, so it
is not repeated in YAML:

```yaml
# projects/demo/.factory/config.yaml
version: 1
scope:
  id: 8fc86b67-aeba-4935-a623-d75f59d77acd
  name: demo
  agent: pi                  # this scope's default, overriding the instance's
  agents:
    - name: reviewer
      harness: claude-code
      lifetime: task
  task_store: file-store     # this scope's tasks live somewhere else
```

Only directories with a valid local scope block appear in the CLI or UI. A
configured scope is nested below its nearest configured ancestor in the rail;
ordinary directories get no row and cannot be selected or receive work.
Discovery is performed at daemon startup; add or remove a marker, then restart
to refresh the scope list. Duplicate IDs and names, malformed files, and
missing IDs or names stop startup with the files involved named in the error.

### Where a scope's tasks live

`daemon.task_store` names the engine for the instance; a scope that names its
own overrides it. That is how one project's tasks can be issues in a tracker
while another's stay in the built-in sqlite, and it is the whole of the
configuration: the adapter has to be registered, as a built-in or a plugin, and
a scope naming one that is not registered stops the daemon at startup rather
than quietly landing that project's tasks in the wrong database.

What does not move with the task is the ledger. Runs, the journal, the standing
agents and the liveness history stay in the instance's default store, whatever
engine holds the task:

| | where the scope says | the instance's default store |
| --- | --- | --- |
| the task, and its schedule | yes | |
| runs: attempt, token, session, start and end | | yes |
| journal entries | | yes |
| standing agents, liveness history | | yes |

That is not a limitation dressed up as a design. An issue has a title, a body
and a state; it has nowhere to put an attempt number or a callback token, and
an `AgentSession` is not something a tracker has heard of. Keeping one ledger
is also what makes `active_runs()` complete, so the watchdog and the scheduler
never have to ask every engine in turn whether it has forgotten a run.

The practical consequence for whoever writes a store adapter is six methods:
`create`, `get`, `list`, `update`, `delete`, `due`. The daemon asks a scope's
store nothing else. `examples/plugins/file-store/` refuses the rest out loud, so
a store that is asked something it should not be says so instead of guessing.

The engine a scope uses is shown in the Roster, and is not editable there
-- see the last section of this file for why.

### The AI accounts behind the agents

An agent declaration says which harness it runs, not which account pays for
that harness's model calls. The root config says that, in an
`infrastructure.providers` list, and `factory infra` (or
`GET /api/infrastructure`, the L1 Infrastructure page) shows it next to the
host and the daemon: each provider's name, vendor, kind and plan, and every
agent it serves.

```yaml
# root .factory/config.yaml
infrastructure:
  providers:
    - name: claude-max
      vendor: anthropic
      kind: subscription
      plan: Max 20x
      harnesses: [claude-code]
    - name: openrouter
      vendor: openrouter
      kind: api-key
      env: OPENROUTER_API_KEY    # the variable's NAME; its value is never read
      harnesses: [pi, opencode]
```

```yaml
# any scope's own config
scope:
  agents:
    - name: model-lab
      harness: claude-code
      provider: openrouter       # overrides the harness default
```

**Declared, never discovered.** Factory does not open
`~/.claude/.credentials.json`, `~/.pi/agent/auth.json`, the Keychain or any
`.env` to find out which accounts exist, and nothing about a provider is ever a
secret: `env:` is the *name* of the variable an api-key's key lives in, shown
as written, and the variable itself is never read.

The binding rule: an agent's own `provider:` wins; otherwise it gets the one
provider whose `harnesses:` lists its harness. `shell` makes no model call and
never has a provider. A model agent nothing claims is not an error -- it is
listed as **unassigned**, which is the answer to what to declare next. The
synthesized foreman is bound like any other agent, by its harness.

Refused when the config loads, naming what is wrong: a `provider:` that names
no declared provider (also refused by the Roster before it writes the file), two
providers claiming the same harness, the same provider name twice, `env:` on a
`subscription`, a `kind` other than `subscription` or `api-key`, a provider
claiming `shell` or a shell agent naming one, and an `infrastructure:` block in
a nested scope's own file -- only the root's is read.

The host half of the page is read live on every request, from `sysctl` and
`libc` rather than a subprocess per field: model, chip, cores, memory, OS,
uptime, load and the root filesystem. Any of those that cannot be read is
`null`, never a failed request, and only macOS answers all of them. Usage and
spend per provider are not shown: Factory does not record tokens yet.

## Writing a plugin

A plugin is any program that reads one JSON object per line on stdin and writes
one per line on stdout. No shared ABI, no Rust, no rebuilding the daemon.

Put it in `.factory/plugins/<name>/` with a manifest:

```yaml
name: my-agent
kind: agent                  # agent | task
description: what it is
command: ["python3", "adapter.py"]
timeout_seconds: 30
```

The protocol:

```
in   {"id": 1, "method": "agent.prompt", "params": {...}}
out  {"id": 1, "result": {...}}
out  {"id": 1, "error": {"message": "..."}}
```

Every plugin answers `describe`. The daemon calls it at startup, so a plugin
that is broken says so then rather than when a task needs it. A plugin that
will not load is reported and skipped; it never stops the daemon.

An **agent** plugin answers two more methods. `agent.launch_spec` says how the
runtime should bring the agent up — either a harness the runtime knows by name
(`{"kind": {"named": "gemini"}}`) or a command to run in the session
(`{"kind": {"command": ["my-agent", "--headless"]}}`). `agent.prompt` returns
the text to submit. Both are given the task, the working directory, the path to
the `factory` binary, the callback token, the knowledge pages the run was
handed (`knowledge` on the binding, only when the task asked for them),
`reporting_contract` — the exact
wording the built-in agents use to say how to report back — and
`factory_guide` — the same wording they use to say what Factory is, who this
agent is, and which commands its role allows. Paste both rather than
rewriting them.

A **task** plugin answers `task.create`, `task.get`, `task.list`, `task.update`,
`task.delete`, `task.append_entry`, `task.entries`, `task.due`, and the run
side: `run.create`, `run.get`, `run.update`, `run.list`, `run.active`,
`run.active_all`, and `run.entries`. That is what it takes to back tasks with an
issue tracker instead of the local database.

`examples/plugins/shell-plugin/` is a complete, working example in about eighty
lines of Python. Copy it. `examples/plugins/file-store/` is the same for the
task-store seam -- tasks in one JSON file, and a refusal of everything that
belongs in the instance's ledger store instead.

A plugin may not take the name of an adapter that already exists; the registry
refuses the collision rather than silently shadowing a built-in.

## Interfaces

Both interfaces speak one request envelope, so they cannot drift apart. The
socket carries it directly, one JSON object per line:

```sh
echo '{"op":"task.list","params":{}}' | nc -U .factory/factory.sock
```

HTTP maps REST onto the same thing — `GET /api/tasks`, `POST /api/tasks`,
`POST /api/tasks/{id}/run`, `GET /api/tasks/{id}/runs`, `GET /api/runs/{id}`,
`GET /api/runs/{id}/entries`, `GET /api/tasks/{id}/entries` (`?task_only=true`
for only the task's own lines, none of its runs'), `GET /api/runs/{id}/output`, `GET /api/agents`,
`GET /api/agent-runtime`, `GET /api/environment`, `GET /api/infrastructure`,
`GET /api/knowledge`,
`GET /api/knowledge/search?q=&tags=&scope=&limit=`,
`PUT /api/knowledge/files?path=&overwrite=` (raw bytes, its own 50 MiB body
limit), `GET /api/benchmarks`, dataset CRUD under `/api/datasets` (plus
`POST /api/datasets/{name}/cases`, `.../import` and `.../from-tasks`, and
`DELETE /api/datasets/{name}/cases/{id}`), bench runs under
`POST /api/bench/runs`, `GET /api/bench/runs[?dataset=]`,
`GET /api/bench/runs/{id}`, `POST /api/bench/runs/{id}/cancel` and
`.../clean`, policy status and detail under `GET /api/policy?scope=` and
`GET /api/policy/controls/{framework}/{id}?scope=`, attestations under
`POST /api/policy/attestations` and
`POST /api/policy/attestations/{id}/withdraw`, closing a gap under
`POST /api/policy/remediate`, an audit export under `GET
/api/policy/export?scope=&format=` (a download, not the ordinary envelope —
see "Policies" above), computed metrics under `GET /api/metrics?ids=a,b`
(empty `ids` is every non-parameterised metric plus whatever the loaded
goals and policy catalogues imply), the L6 Goals tab under
`GET /api/goals?scope=&cycle=`, check-ins under
`POST /api/goals/checkins` (see "Goals" above), the L6 Scenarios tab under
`GET /api/scenarios?scope=`, turning one into real work under
`POST /api/scenarios/promote`, and recomputing driver outcomes and the
forecast under `POST /api/scenarios/whatif` (see "Scenarios" above), the
L6 Quality attributes tab under `GET /api/quality?scope=` and a scenario's
remediation task under `POST /api/quality/remediate` (see "Quality
attributes" above),
L4 Operations tab under `GET /api/operations?scope=&window=`, skipping a
schedule's next slot under `POST /api/tasks/{id}/skip-next` and answering
a blocked run under `POST /api/runs/{id}/answer` (see "Operations" above),
workflow CRUD under
`/api/workflows`, workflow-run
start/list/cancel under `/api/workflows` and `/api/workflow-runs`, and
`POST /api/rpc` for the raw envelope. `GET /ws`
is the event stream: a snapshot of every task first, then one message per event.

Adding an interface — mcp, or anything else — means translating to that
envelope, not inventing a second API.

## The web UI

The views are grouped by level in the header: **Dashboard** is the landing view,
then **Activity**, **Site plan**, **Inbox**, **Tasks**, **Workflows**,
**Occupancy**, **Roster**, **Agent-runtime**, **Roles**, **Sandboxes**,
**Secrets**, **Benchmarks** and **Knowledge**. Switching between them is a small
registry — one container shown, one button lit,
and whatever that view needs to start or stop doing while it is not the one on
screen.

**Tasks** is the list; clicking one opens it in a modal with its
runs, the selected run's journal, and its terminal. The terminal is shown from
the moment a run exists — live from the session while it runs, and the
transcript kept at the end once it does not — so there is never a button to
press to find out what an agent is doing.

Both pages show a terminal, and it is a real one. The daemon asks the runtime
for whole frames -- the grid herdr has already rendered, roughly four times a
second, sent only when it changed -- and every keystroke goes back as the bytes
a terminal would send. Arrows, Tab, Ctrl-C and an agent's own escape hatch all
work, because herdr passes the bytes to the pane untouched and there is no
table of key names in between to fall behind what a keyboard can do. Click the
screen and type — there is nothing else under it, because there is nothing a
button could do that a key does not. A run that has ended has no session left
to mirror, and then what is shown is the transcript the daemon kept.

The page has two themes, **foundry dark** and **foundry light**, switched from
the header and remembered per browser. Dark is what a page with nothing stored
gets. There is no `prefers-color-scheme` rule anywhere: a stored choice and an
OS query fighting over the same tokens is a bug that only turns up on somebody
else's laptop. The terminal keeps its dark ground in both, because an agent
draws for a dark pane and inverting it would invert its own colours.

The type is IBM Plex, loaded from Google Fonts. A machine with no internet
falls back to a system stack and the page is still readable, just not
condensed — the only thing this page fetches from anywhere but the daemon.

There is no terminal emulator in the page. herdr's frames have already had
every cursor move and scroll applied, so what arrives is text and colour, and
eighty lines turn that into HTML.

**Occupancy** is a chart: one row per agent, grouped by scope, drawn against a
shared clock. Three layers, kept
deliberately apart because they are different kinds of evidence:

- a **solid block** is a run — Factory started it and the agent reported back;
- a **dashed block** past the now line is a schedule's next firing, drawn as
  wide as that task's explicit estimate when it has one, otherwise as the
  median of its own finished runs, or as a marker when it has none to measure;
- a **dotted yellow outline** around an active run is the task's user-authored
  estimated duration; the solid run grows past it when the estimate is exceeded;
- the **thin strip** underneath is liveness: what the runtime saw on the
  screen. A guess about a terminal, never a claim about work.

The right-hand column is utilisation over the window. Clicking any block opens
the task. The chart looks a quarter of a window ahead of now, so the next
scheduled run has room to be read.

**Roster** is the list: each scope, then the agents it declares, then
what each one is doing. A standing agent can be started, stopped, and opened —
its terminal is live, and there is a line to type into it with keys for Enter,
Esc, ↑, ↓ and Ctrl-C. That is enough to answer the prompt an agent is sitting
on, which is usually a first-run trust dialog or a login. Every agent also
offers **Start task…**, which opens the create form with that scope and agent
already chosen. A task run's terminal takes the same input.

**Agent-runtime** shows the connection underneath those sessions. It groups
scopes that use the same effective runtime and asks the adapter for a
read-only diagnostic. For Herdr that includes its session and socket, client
and server versions and protocols, compatibility, restart-needed state and
capabilities. An unsupported adapter or failed probe remains a card with an
honest state; it does not take the daemon or the other runtime cards down.

**Roles** (`#<scope>/harn/roles`) says, for every role in effect in the
selected scope, what it is for; what it may *and may not* do, in the phrases
the daemon checks with, grouped into tasks, agents, runs and workflows; how far
it reaches, in words; where it was defined — `built-in`, `instance`,
`inherited from projects`, `defined here`, or `defined here · overrides
projects`, linking to the scope that defines it; and which agents in the scope
hold it, with a role given by `factory agent role` marked apart from a declared
one. A compact matrix above the cards compares them at a glance, and scopes
below the selected one that add or override roles are listed as links, so one
scope's view is not mistaken for the whole tree. With **All scopes** it shows
the inheritance itself: the built-in and instance roles once, then each scope
that defines roles, nested by path, with what it adds and what it replaces.

**New role…**, **Edit**, **Delete** and **Override here** act on the selected
scope and write its own `.factory/config.yaml` — `scope.roles`, or the top-level
`roles:` for the instance root — leaving every other line of the file where it
was. **Override here** copies an inherited role into this scope for editing; a
child never edits its parent's file. Grants are picked from the daemon's own
list and reach from own or scope, never typed. Changing a role definition is
the owner's alone (`role.define` and `role.delete` need the owner, not
`agent.configure`): an agent that could rewrite a role would not be bounded by
the one it holds. Redefining `worker` or `foreman` is refused, and so is
deleting a definition any agent still resolves to — declared or given, in that
scope or below — with every holder named; so is any save that would leave an
agent on a role nothing defines. A changed role holds from each agent's next
request; a guide already in a running session is not rewritten. The page says
in its own markup, not in anything it fetches, that roles are guard-rails and
not a security boundary.

**Dashboard** is five KPI tiles, a by-scope table and an inbox, all read from
the same `state.tasks` and `state.scopes` every other view already holds —
nothing here is fetched specially. The inbox lives inside the dashboard rather
than beside it: every blocked, failed and cancelled task, and every schedule
that missed its own next run, newest first. `blocked` is a real, first-class
status — "the agent needs a human before it can go on" — so this is never a
stub of one; what the daemon genuinely does not record is the *question*
itself, and the closest thing to an answer is the journal entry the agent
wrote when it blocked, shown if it wrote one.

**Activity** is a live tail, not an archive: every event this page has seen
since it was opened, filterable by kind and by free text. There is a `/ws`
stream and a journal per task, but no queryable history behind either yet, so
the banner says plainly that nothing earlier than "now" is shown here.

**Site plan** draws the same scopes as a place: one hall per scope, and a
figure for every agent actually present — never a bay, because Factory has no
bay ("a row is an agent, not a bay", `occupancy.rs`). A second, lit three.js
render of the same facts toggles from the same HUD, orbits, and picks the same
hall the plan would. In that 3D render the scope rail is a focus rather than a
filter: the full site remains standing while the camera moves to the selected
hall, its roof opens, and its present agents move onto the shop floor. Clearing
the scope returns to the fitted site. The isometric Plan keeps the narrower
scoped view used by the rest of the interface.

A hall carries two signals, and `factory-core/src/building.rs` is the one place
that decides either. **Size** — the files, bytes and directories a bounded walk
of the scope finds (`/api/site`, cached for five minutes) — becomes a tier, a
floor count, a footprint and a number of window bays. **Activity** — runs in
flight, tasks queued, agents standing up, all out of the daemon's own records —
becomes lit floors, a roof beacon and how fast it beats. Neither reaches the
other: activity may light a hall and never build one, or height would stop
meaning size and a run starting would shove the hall's neighbours across the
apron. Both steps are stepped and sticky, so a metric sitting on a threshold
does not flip the building between two shapes on every poll, and the page eases
between them rather than cutting.

Both views fall back to honesty over invention. A scope whose directory cannot
be read is drawn plain rather than small — "could not be read" is not a
measurement of zero — and its floor says "not recorded" instead of a treemap
drawn from nothing; a walk that hit its cap says its numbers are a lower bound;
the file a session is editing is not read at all, and is not drawn.

**Benchmarks** gets a segmented control — **Datasets**, **Runs** and
**Configurations** — each with its own hash route
(`#<scope>/imp/benchmarks/datasets/<name>`, `.../runs/<id>`,
`.../configurations`) that boots directly and survives reload and
back/forward. **Datasets** lists every dataset, company-wide; selecting one
shows its cases, findings, and *New dataset*, *Add case*, *From tasks…*,
*Import…* and *Run…* — the last starts a bench run and switches to **Runs**.
**Runs** lists every bench run and, for the selected one, a results table per
configuration, an attempts matrix of case × configuration whose verdict chips
link to each attempt's task, and a small scatter of resolve rate against mean
wall-clock; it updates live on `bench_run_updated` and offers *Cancel* and,
once a run has finished, *Remove worktrees*. The rail narrows a selected
dataset's own cases and a selected run's attempts matrix; it never narrows the
company-wide dataset list or the aggregated results table, which stays the
daemon's own numbers for the whole run. **Configurations** is v1's cards,
unchanged.

## What this prototype does not do yet

- **Runtime, interface and knowledge plugins.** The manifest accepts
  `kind: runtime`, `kind: interface` and `kind: knowledge`, and the daemon says
  plainly that it will not load them. The traits are there; the proxies are
  not.
- **No schema migrations.** The database carries a version; one written by a
  different version is dropped and rebuilt. The daemon warns when it does this.
  Fine for a prototype, not for anything you would miss.
- **Liveness only exists from when Factory started writing it down.** herdr
  answers "what is this agent doing now" and keeps no history, so the daemon
  records every change it observes into an append-only table. Nothing before
  the first recording exists, and the chart says so rather than drawing a flat
  line back to the beginning of time. The runtime is polled, so a flip and a
  flip back between two ticks leaves no trace.
- **A task always opens its own session**, even when it names a standing agent.
  Sending work into an agent's existing session — so it keeps its context — is
  a different feature, with its own questions about whose transcript is whose.
- **The socket is the security boundary.** It is `0600` in `.factory/`, and the
  callback token only stops one running agent from closing another's run by
  mistake. The HTTP interface has no authentication at all. It binds to
  loopback by default; `bind: 0.0.0.0:8787` puts it on the local network, where
  anyone who can reach it can start a task **and has a full keyboard on every
  agent's terminal** — arrows, Ctrl-C, and whatever escape hatch the agent
  itself offers, which for most of them is a shell. That bypasses the scope and
  agent config entirely and runs as whoever runs the daemon. The daemon warns
  on every start when it is bound past loopback, and prints the address a
  person would actually type. The warning is the whole of the protection: put
  this on a network you would hand a shell to, or leave it on loopback.
- **A scope's task engine is configuration, not a control.** The Roster says
  which engine a scope's tasks live in; changing it means editing
  that scope's `.factory/config.yaml` and restarting. A selector that rewrote the
  configuration over an interface with no authentication is a different
  decision, and it has not been made.
- **First-run agent prompts.** An agent that has never seen a directory may ask
  a human to trust it before it will read the task. Factory cannot answer that
  for you. For a harness whose runtime reports through a lifecycle hook (`pi`,
  today), this now surfaces as a hook-reported `Blocked` run rather than
  silence, and is governed by `blocked_timeout_seconds` instead of the run
  timeout. For every other harness it is still exactly what it always was: the
  task times out and tells you where to look.
- **One herdr workspace per scope, one tab per session, closed on completion.**
  Ending a run or stopping a standing agent closes only its own tab; the
  workspace stays for the rest of the scope. A task that never reaches a
  terminal state leaves its session open on purpose, so it can be looked at.

## Layout

    crates/factory-core      domain, events, wire protocol, the five adapter traits
    crates/factory-plugins   built-in adapters, the plugin host, the registry
    crates/factory-daemon    engine, scheduler, interfaces, the binary
    crates/factory-cli       the `factory` binary
    ui/                      the web UI, compiled into the daemon
    ui/index.html              the page skeleton and the two view containers
    ui/app.css                 every style
    ui/js/core.js              DOM helpers, client state, the HTTP call, the socket
    ui/js/app.js               the wiring: which page shows, what an event means
    ui/js/{tasks,task-form,agents,occupancy,terminal,modal}.js   one per view
    ui/js/{dashboard,activity,site,site-render}.js               the new views
    ui/js/{sandboxes,secrets}.js                                 L2's two tabs
    ui/js/{benchmarks,knowledge}.js                              L5's two tabs
    ui/js/knowledge-graph.js                                     the knowledge graph's pure layout, filter and tail logic
    ui/vendor/three.min.js     vendored so the site's lit render works offline
    examples/plugins         a worked example of an out-of-process adapter
