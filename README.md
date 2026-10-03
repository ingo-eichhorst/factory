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
| **Agent** | how a harness is started, what a task sounds like to it, and how to check it starts | `claude-code`, `pi`, `codex`, `opencode`, `shell` |
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
    - name: codex
      harness: codex
      max_sessions: 3         # at most 3 sessions of this agent at once (#179)
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

**`max_sessions` is enforced (`#179`).** An agent declaring it may have at
most that many sessions open at once -- a run of it `Dispatching` or already
holding a session, plus one if it is also a live permanent agent under the
same name. A scope may declare its own `max_sessions` too (in its `scope:`
block, root included), a cap shared across every agent in it; the two apply
together, whichever is tighter. `0` is refused at load, naming the scope,
since it would never run anything. Absent, on either, is unlimited -- today's
behaviour for a config that names neither. A dispatch that would exceed
either cap is **held**, not failed: the task stays `Pending`, carrying a
`slot_wait` (agent, scope, trigger, when it became due, when the hold began)
and a `capacity_held` journal entry, and starts the moment a slot opens --
another run of the same agent ending, or a scope-mate's. Every trigger goes
through the same admission check (manual, a schedule's slot, a queued retry,
a workflow node, a bench attempt), so all are held the same way and released
the same way: FIFO by when they became due, oldest first. `factory status`
prints each capped agent's `in_use/max` and how many are waiting; the
Operations tab's Flow card and `factory stats` show a scope's own cap the
same way they show everything else about a scope (see "Operations" below).
A held task appears in the L4 pending board as "waiting for a slot" rather
than "due" or "manual, not run".

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

**`codex` commands do not see their own session's environment (#147).** Codex
CLI runs every interactive session on one shared background app-server, and
runs shell commands in that server's host process. Both are started once, by
whichever `codex` session comes first, and keep *its* environment. A later
run's commands therefore see an earlier run's `FACTORY_*` values, with a
token that died when that run ended, and an earlier run's `HERDR_*` values,
possibly those of the company's live herdr session. So a `codex` task run's
reporting contract writes the socket and the run token into every command
(`factory --socket … --token … task report <id> --run-token … --status …`,
and `task attach --id … --run-token …`). Flags beat the variables in the CLI,
so reporting works whatever the host inherited. The token is in the prompt,
and so in the harness transcript. It is good only while the run is active.
Everything else a codex agent runs still sees the stale variables, so
**never start a throwaway instance's codex run while a live one might own
the shared server**: its herdr calls can land in the live session.
`codex --no-daemon` would avoid the shared server, but on Codex 0.157 its
fresh command host timed out on this machine, so it is not used.

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
- **triager** coordinates the intake gate and runs nothing itself: it receives,
  triages, assesses and decides intake items across its scope, but holds no
  `task.*`, `agent.*` or `workflow.*` grant at all — starting a triage run
  (`intake.triage`) is as far as it reaches, and that run executes as the
  scope's own worker or whichever agent it names, never as the triager.

An agent says which one it is by presenting the token Factory put in its
session as `FACTORY_TOKEN`; the CLI sends it on every request. No token means
the owner.

Those three are not the whole list. A role is a **name**, a set of **grants**,
and a **reach**, and an instance names as many as its way of working needs:

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
    intake.add  intake.info  intake.triage  intake.assess  intake.decide
    dashboard.edit

Reading is not among them, because reading is open to every agent: one that
cannot see the board cannot coordinate with anyone.

`worker`, `foreman` and `triager` ship written in that same vocabulary — a
worker is `[task.edit, task.report, task.attach, run.input, intake.assess,
intake.decide]` at `reach: own`, a foreman is everything at `reach: scope`, and
a triager is exactly the five `intake.*` grants, named rather than written as
`intake.*`, at `reach: scope` — and none of the three can be redefined. An
instance that could rewrite `worker` from one line would widen every agent
that never asked for a role. `task.*` does not expand to any `intake.*` grant,
and `intake.*` (or `*`) is what does — the two vocabularies are separate on
purpose, so a role written for tasks does not quietly pick up the intake gate.

Before intake had its own grants (`#172`), it reused the task ones: handing
something in was `task.create`, triaging, assessing or deciding one was
`task.edit`. An instance role that named `task.create` or `task.edit` so it
could work the intake gate keeps those task grants, but loses intake access
until it also names the matching `intake.*` grants. And `triager` is now a
name Factory itself defines: an instance or a scope that already names a role
`triager` fails to load, exactly the refusal a `worker` or `foreman`
redefinition already got.

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

A scope's roles are, in order: the three built in, the instance root's
`roles:`, then each scope's `scope.roles` from the top of the tree down to the
scope itself. The nearest definition wins. A scope's parent is the nearest configured
scope above it **by path**, never by name: names may contain `/` and are
matched loosely on purpose, so a scope named `projects/other` whose directory
is somewhere else is not below `projects`.

- **An override replaces the whole definition** — description, grants and
  reach. Grants are never merged: a merge could only widen, and nobody reading
  either file could tell what the result was.
- **`worker`, `foreman` and `triager` cannot be redefined at any level**, for
  the same reason they cannot be redefined at the root.
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

## Dependencies

The L2 Environment page's **Dependencies** tab keeps two different facts
together without confusing them: components shipped in a scope's product, and
services its agents are expected to reach. It always shows one scope at a
time. The SBOM segment shows the newest document per lifecycle state and every
finding a scan reported; the Services segment shows declarations from that
scope's own config, including whether a credential location matches a row the
Secrets inventory knows is present.

A scope opts in with a strict `dependencies:` block (unknown fields are
refused). An `agents:` filter may name only an agent that same scope declares:

```yaml
scope:
  name: assistant
  dependencies:
    scan_workflow: dependency-scan
    max_age: 30d
    services:
      - name: bank-main
        transport: network          # network | socket | file
        endpoints: [fints.example-bank.de:443]
        effects: read               # read | write
        data: [financial, personal]
        credential: keychain:fints-bank-main  # a location, never a value
        agents: [assistant-finance]  # absent means every declared agent
      - name: bank-exports
        transport: file
        path: ~/Documents/Bank/Exports
        direction: in               # in | out | both
```

Scanning is an ordinary workflow, not an adapter. A scan task produces two
CycloneDX JSON documents (version 1.6 or newer) and sends their bytes through
its authenticated callback:

```sh
factory task attach --kind sbom sbom.cdx.json
factory task attach --kind vulnerabilities vulnerabilities.cdx.json
```

The daemon never follows the task's path. It validates the bytes and stores an
immutable raw copy plus run/attempt/time metadata under
`<root>/.factory/dependencies/<scope>/<run-id>/`; another attachment always
creates another file. The SBOM must declare `metadata.lifecycles` (`pre-build`
for declared, `build` for built, `operations` for running) and must not contain
a `vulnerabilities` field. Vulnerability documents carry `vulnerabilities[]`
whose `affects[].ref` values point into the SBOM by `bom-ref`. See
`examples/dependency-scan.sh` for a declared-state Syft/Grype example; tests
use checked-in CycloneDX fixtures and require neither tool.

For opt-in source call analysis, run `examples/dependency-scan.sh --reachability`.
It adds OSV-Scanner v2's native JSON output to the existing vulnerability
attachment via `examples/dependency-reachability.mjs`, matching advisory
ids/aliases and exact package ecosystem/name/version from the SBOM's PURL.
`factory:reachability`, `factory:reachability-detail` and
`factory:reachability-evidence` are JSON objects keyed by exact `affects[].ref`.
Unsupported packages, missing analysis and ambiguous negative evidence remain
`unknown`; a reported call is `reachable`, and exclusively explicit uncalled
evidence is `unreachable`. These are scanner claims, not exploitability or VEX
verdicts. The helper preserves authored `analysis`, severity and exploit flags.
OSV exit 1 means findings and is accepted; scanner failures attach no evidence.

The finding's reported reachability and analysis detail are shown unchanged in
the CLI, HTTP projection and shared Dependencies/Doctor card. A scalar
`factory:reachability` property is a vulnerability-wide claim; a per-ref object
is component-specific. `analysis.detail` is preserved as prose and never
classified. Duplicate conflicting properties remain explicit conflicting
claims. The original evidence document/run/time is retained even if a later
scan resolves the finding. No claim changes status, policy counts, reporting
deadlines or authored VEX; absent evidence stays unknown.

[OSV call analysis](https://google.github.io/osv-scanner/usage/scan-source/)
requires language tools. Rust support is experimental, compiles the project,
and executes its build scripts: use it only for trusted source in a suitable
sandbox. Unsupported code, including dynamic linkage and some macro-generated
paths, cannot establish a safe verdict. The independent `source-reachability`
node in Factory's workflow scans declared checkout dependencies; its evidence
does not describe installed binaries or the separate built/running documents.

`factory dependencies <scope>` and `GET /api/dependencies?scope=` read the
same projection. A finding is `open` when the newest scan still reports it,
`assessed` when that scan carries CycloneDX `analysis`, `resolved` when a newer
scan of the same lifecycle no longer reports it, and `stale` when the newest
scan is older than the scope's `max_age`. These are derived statuses: there is
no write API for them. `factory:` properties on a vulnerability carry KEV,
EUVD and EPSS signals.

For the `factory` scope only, L2 omits the running document and its findings:
that evidence describes this installed instance and appears in L1 Doctor.
The socket/CLI projection still carries every lifecycle so
`factory dependencies factory` remains the detailed read side.

VEX judgments are authored content at
`<root>/.factory/vex/<scope>/*.cdx.json`, separate from every SBOM. `factory
dependencies vex <scope>` validates them and prints one merged CycloneDX VEX
document for a scan workflow to consume. Factory never edits those files.

Factory's own v2 workflow is checked in as
`workflows/factory-dependency-scan.yaml`. Its `built` node runs
`cargo auditable build --workspace --release`, scans the two release binaries,
and attaches a `build` SBOM plus a separate vulnerability document. Its
`running` node scans exactly `~/.local/bin/factory` and
`~/.local/bin/factory-daemon` (or `FACTORY_INSTALL_DIR`) from their embedded
audit data and attaches the equivalent `operations` documents. Both nodes use
`examples/factory-dependency-scan.sh`; Syft, Grype, jq and cargo-auditable are
workflow tools, not daemon dependencies.

Those Factory SBOMs identify the product by the workspace version and a
`factory:git-sha` property on `metadata.component`. Both binaries expose the
same identity in `--version`, embedded at build time, so the installed scan
does not infer a commit from the current checkout. `factory dependencies
factory` prints that version and commit beside built and running document
rows.

The L1 **Doctor** tab reads `GET /api/doctor`, a read-only projection of the
newest Factory `build` and `operations` documents. The daemon derives
`current` when both identities match, `behind` when both exist and differ, and
`missing` when either document or identity is absent. Doctor shows scan age
and findings from the running state only. The daemon never scans itself, and
Doctor does not yet diagnose daemon health, the store, socket, plugins,
harnesses or repository hygiene; that broader work belongs to #157.

Policy catalogues may read the same evidence:

```yaml
- check: dependencies
  sbom_max_age: 30d
  built_sbom: true
  max_open: { critical: 0, high: 0 }
  exploited_open: 0
```

The scan workflow supplies the build and installed-binary evidence used by
`built_sbom` and Doctor, plus optional declared-source reachability evidence.
Observed services still require sandbox enforcement evidence; the CRA Article
14 reporting clock's 24-hour and 72-hour deadlines
(`#157`, phase 1) read this same evidence -- see "Policies" below.

`Engine::exploited_findings` (daemon `dependencies.rs`) is that clock's L2
read: one `ExploitedFinding` per (scope, vulnerability id) sighted against a
`built` or `running` SBOM that carries `factory:kev` or `factory:euvd` and
whose `analysis.state` is not `not_affected`, `false_positive`, `resolved`,
or `resolved_with_pedigree` -- a `declared` SBOM never counts. Once sighted,
CRA counts from that first sighting's own document time regardless of what a
later scan shows: the item survives a scan that drops or resolves it
(`reported_now: false`), and is excluded only when the *newest* built or
running document that still mentions the vulnerability reports it
`not_affected` or `false_positive`.

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

**Checks and statuses.** Evidence is evaluated per check kind, and all eleven
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
the live config snapshot rather than a store — except the `backup_*` names
`daemon` also carries (`#154`), which read L1 Backup's own captured state
(`Engine::backup_fact`: the backup store and one destination listing), and
so are gathered lazily and asynchronously, only when some scope's applied
controls actually name one:

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
  `power_assertion` (`daemon.power_assertion`), and three read off L1
  Backup's own fact (`#154`): `backup_recent` (the newest backup is within
  its schedule, or the unscheduled yardstick, plus grace), `backup_offsite`
  (the destination is not on the same device as the instance root), and
  `backup_verified` (the newest verification of a snapshot still in the
  destination passed, within 30 days). All three read `false` when no
  backup is configured at all, and indeterminate (`open`, "could not be
  determined") when the destination is missing or unmounted — neither
  "yes" nor "no" is honest about an archive nobody can currently list. A
  `fact` outside this whole set is a finding at catalogue load time and
  stays `open`. For example, DSGVO Art. 32(1)(c),(d) (restore availability,
  tested regularly) is not mechanically provable end to end, so it pairs a
  daemon check with a person's attestation:

  ```yaml
  - id: art-32-restore
    title: Availability of personal data can be restored in a timely manner, and restoring is tested regularly (Art. 32(1)(c),(d))
    evidence:
      - check: daemon
        fact: backup_verified
      - check: attestation
  ```
- **`dependencies`** — satisfied when the newest declared SBOM is within
  `sbom_max_age`, a build SBOM exists for the newest release when
  `built_sbom: true`, open findings do not exceed each `max_open` severity
  limit, and KEV/EUVD findings do not exceed `exploited_open`. It reads the
  same derived projection as L2 Dependencies; no policy-specific copy is
  stored.
- **`attested { category, step, max_age }`** (`#158`, phase 1) — whether
  finished runs of `category` actually *conformed* to their own control
  plan's `step`, not just that a person said so: every enforced step's
  newest attestation, by someone other than the run's own agent, passed.
  `max_age` is required, since coverage needs a window. Satisfied when at
  least one `done` run of `category` ended within `max_age` and every one
  that did passed `step`; stale when none did but the newest one in
  `(max_age, 2×max_age]` passed; open when an in-window run is missing or
  failed evidence for `step` (naming up to three, including one never held
  to it at all), or when nothing ended `done` within `2×max_age`. Evidence
  is one L4-owned read (`Engine::attested_runs`, `factory-daemon/src/verification.rs`)
  of finished runs and their `StepAttestation`s, shared with the
  `conformance_rate.<category>`/`gate_fail_rate` metrics below so a run is
  never judged twice; a review or approval step leaves no attestation in v1
  (`#118`), so a check on one reads `open` until that lands. `refs` names
  the task and run behind each reason, the same as `task`/`workflow`.

Neither `roles`/`sandbox`/`secrets`/`daemon`/`dependencies` carries a `refs` entry: nothing
behind them is an id a UI could link to yet (an agent name is not one of
`EvidenceRefKind`'s kinds, and a daemon/secrets fact is not tied to any one
record at all) — the L6 Policy tab instead links a gap in one of these to
the level that can close it (Roles, Sandboxes, Secrets, Dependencies, or L1
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

**The CRA Article 14 vulnerability reporting clock** (`#157`) is L6 Policy's own
projection over evidence two other levels already keep: L2's exploited
findings (`Engine::exploited_findings`, see "Dependencies" above) and L4's
confirmed security reports (`Engine::confirmed_security_reports`, `#170`).
It covers the 24-hour early warning and 72-hour notification
deadlines Art. 14(2)(a)/(b) sets from awareness — a finding's first
qualifying sighting, or a report's `received_at`. The 14-day final report
runs from evidenced corrective/mitigating measure availability, not awareness
or confirmation. A confirmed report that was split (`#170`'s intake split,
`ConfirmedSecurityReport.parent`) counts once, at its split chain's root
awareness, however many parts of it are also confirmed.

`factory_core::reporting_clock::compute` is pure and stateless — no status
table, computed fresh from its three inputs on every read (ADR 0004) — and
is the one place `ClockItemRef` (`finding:<scope>:<vulnerability>` or
`report:<task-id>`, the CLI's own text form for either) and a deadline's
`due`/`overdue`/`met`/`late` state are decided. `factory policy clock
[--scope S]` and `GET /api/policy/clock?scope=` read it, rolled up over a
subtree exactly like `factory policy`/`GET /api/policy` themselves are;
`policy.clock` needs no grant, the same as `policy`.

A submission — an early warning, notification or final report actually sent — is
recorded as an ordinary attestation carrying a `clock: {item, deadline}`
mark, through the same `policy.attest` door and the same root-scope rule
every other attestation goes through:

```sh
factory policy attest cra/art-14 --scope demo \
  --evidence https://example.com/early-warning-sent \
  --clock-item finding:demo:CVE-2026-1234 --deadline early-warning
```

`--expires` defaults to `520w` (ten years — `Duration` has no year unit) once
`--clock-item` is given, since CRA evidence is kept far longer than an
ordinary attestation's expiry is ever checked against; the clock itself
ignores `expires_at` entirely; only a withdrawal ever un-meets a deadline.
`policy_attest` refuses a `clock` mark against any control but `cra/art-14`
(the only control phase 1 wires — `examples/policies/cra.yaml` shows it), an
item that does not exist or whose own scope is not *exactly* the canonical
`--scope` given (a subtree read would otherwise let a root-scope attestation
cover a child's item), an excluded item (a later `not_affected`/
`false_positive` finding has nothing left to report), and a deadline that
already has a live, unwithdrawn submission. A row carrying `clock` is
deliberately never enough on its own to satisfy `cra/art-14`'s `check:
attestation` — `direct_status` skips it — so an ordinary attestation still
has to cover the control as a whole; the clock mark is additional evidence
of one specific deadline, not a substitute.

**The UI (`#170` phase 2)** reads the clock through the same router, never
computing a deadline itself: `ui/js/clock-model.js` is the one pure module
that shapes a `ReportingClock` answer into rows, shared by all three
surfaces —

- **Intake** reads `/api/policy/clock` only once a card on the loaded board
  carries a *confirmed* security report (the only state the clock ever has
  a `report:` item for), and shows that report's own deadlines as badges on
  its card and in the item modal. The clock's computed `report_items`
  membership resolves every split descendant to the root, including nested
  splits, without duplicating Policy or Inbox deadlines.
- **The Inbox** reads the whole instance's clock alongside `/api/operations`
  and adds its own overdue and due-soon deadlines (a `finding:` item too,
  linked to Dependencies) to the daemon's attention queue — a second,
  independent list, since a clock deadline is not one of `operations.rs`'s
  own exception kinds.
- **L6 Policy** shows every item on the clock over the selected subtree —
  `finding:` and `report:` alike, including an excluded finding's own note —
  in its own section, read independently of the framework board above it.

A clock read failure degrades gracefully everywhere: the rest of each page
still renders, and the clock's own section or badges simply have nothing to
show.

Record the corrective/mitigating measure's availability explicitly, with
an evidence pointer and RFC3339 timestamp:

```sh
factory policy attest cra/art-14 --scope demo \
  --evidence https://example.com/mitigation-available \
  --corrective-item report:TASK_ID --available-at 2026-10-01T09:00:00Z
factory policy attest cra/art-14 --scope demo \
  --evidence https://example.com/final-report-sent \
  --clock-item report:TASK_ID --deadline final-report
```

These are append-only policy observations, not countdown state or an upward
call from Intake. `corrective: {item, available_at}` carries the anchor;
`--expires` defaults to `520w` for this record too. Future availability,
duplicate live anchors and final submissions without an anchor are refused.
Withdraw an incorrect record before replacing it; expiry does not erase
its historical anchor. Neither a corrective record nor a submission alone
satisfies the whole policy control. Before an anchor exists, Intake and
Policy say the final report awaits measure evidence; there is no invented
date to put in the Inbox. Afterward all three surfaces show its deadline.
This is the vulnerability rule of Art. 14(2)(c), not the different severe-
incident final-report rule of Art. 14(4)(c). Reachability evidence remains
later work under #157.

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
| `agent_hours` | hours covered by run blocks, with overlaps for one agent counted once | occupancy `busy_seconds` |
| `blocked_hours` | blocked hours inside those run blocks, included in rather than subtracted from agent hours | occupancy `blocked_seconds` |
| `compliance.<framework>` | share of counted controls satisfied, attested, or n/a | the selected scope's policy subtree rollup |
| `open_controls.<framework>` | count of counted controls still open or stale | the selected scope's policy subtree rollup |
| `conformance_rate.<category>` | `conforms()/held()` (every enforced required step's newest non-self attestation passed, among runs held to at least one) over finished runs of `category`, trailing 28 days; a held run whose own failure never reached an agent (`FailKind::is_infrastructure`: an ack timeout, a run timeout, a vanished session, a dispatch that never happened) is excluded from both sides of the ratio | `Engine::attested_runs`'s finished runs and `StepAttestation`s (`factory_core::conformance`, #158) |
| `gate_fail_rate` | failed gate attestations over every gate attestation, across every category and re-verification round, trailing 28 days | `Engine::attested_runs`'s finished runs and `StepAttestation`s (`factory_core::conformance`, #158) |
| `review_reject_rate` | failed independent review decisions over all review decisions on finished runs, trailing 28 days; each step/round counts once (newest duplicate wins), excluding self-review and actors other than the frozen reviewer; missing reviews are unknown | the same L4 `AttestedRun` fact port and append-only run attestations (#158); available in Goals, Scenarios, signposts and dashboard tiles through the existing registry |
| `bench.resolve_rate.<dataset>` | the newest settled bench run's resolve rate | `bench::aggregate` |
| `goal_tasks_done.<objective>.<kr>` | count of tasks labelled `goal=<objective>/<kr>` whose status is `done` | task labels, through `TaskStore` |
| `quality.<characteristic>` | share of declared quality scenarios under an ISO 25010 characteristic that are met | the selected scope's quality subtree — see "Quality attributes" |
| `unit_cost` | API-equivalent USD spent per run ended `done` (failed and cancelled runs' cost included), trailing 28 days | each run's measured usage (`Run.usage`, #117) |
| `tokens_per_run` | mean tokens of every type per finished run, trailing 28 days | each run's measured usage (`Run.usage`, #117) |
| `cost_week` | known API-equivalent USD spent over runs *started* (not ended) in the trailing 7 days; no value if any run in the window is unknown, cost-unknown or partial | `Engine::spend`, the same read `factory cost` and `GET /api/costs` answer from (#164) |
| `estimate_accuracy` | share of finished runs with an `original_estimate` whose terminal wall time fell inside its `[low, high]`, trailing 28 days | each run's `original_estimate` and wall time (`Run.original_estimate`, #168) |
| `ready_rate` | ready decisions over every intake decision event (ready, needs-info, wontfix, split), trailing 28 days | the task journal's intake decision events (#165) |
| `needs_info_rate` | needs-info decisions over every intake decision event, trailing 28 days -- the same shared denominator as `ready_rate` | the task journal's intake decision events (#165) |
| `duplicate_rate` | wontfix decisions closed as a duplicate over every intake decision event, trailing 28 days -- an invalid or out-of-scope wontfix, and a split, count in the denominator only | the task journal's intake decision events (#165) |
| `intake_lead_time` | median (nearest rank) of a ready decision's own time minus the item's `Intake.received_at`, seconds, over items released ready, trailing 28 days | the task journal's intake decision events (#165) |
| `backup_age_hours` | hours since the newest backup snapshot | L1 Backup's own captured state (`BackupFact`, #154) |
| `backup_verified_age_days` | days since the newest verification of a snapshot still in the destination that passed | L1 Backup's own captured state (`BackupFact`, #154) |

`factory metrics --scope <name> --window day|14d|90d [ids…]` and `GET
/api/metrics?ids=a,b&scope=<name>&window=day|14d|90d` select one scope plus
its descendants and one trailing interval. Both parameters are optional.
Without `scope`, scope-aware metrics cover the whole instance. Without
`window`, established defaults stay unchanged: seven days for throughput and
`cost_week` (over runs *started*, unlike the 28-day usage metrics below,
which read `ended_at`), 28 days for production ratios, operations, usage,
and the intake metrics, and 14 days for the hour metrics. An explicit window
overrides all run-backed families, the intake metrics included. Unknown
scopes and unsupported windows are errors, not empty reports.

Every definition in the response registry carries `coverage`:
`scope_aware` means it follows that subtree; `instance_wide` means it does
not. `bench.*` and `goal_tasks_done.*` are deliberately instance-wide
because neither underlying record belongs to a scope; `backup_age_hours`
and `backup_verified_age_days` (#154) are instance-wide for the same
reason L1 Backup itself is -- one instance, one destination. All other
current families are scope-aware.

`throughput_week`/`first_pass_yield`/`scrap_rate` read `production.rs`'s own
daily grid directly rather than re-deriving "finished"/"scrapped"/
"reworked" a second time — that module's own doc comment is the one place
those words are defined. Every metric is computed **lazily**, like a policy
fact: `Request::Metrics { ids, scope, window }` only touches `production`/`policy_report`/
the bench store when some asked id actually needs it, and shares each
backing read across all ids that need it (production reads each exact scope
once when it aggregates a subtree). An
id the registry has never heard of refuses the whole call (a typo should
not come back as a quiet `None`); a metric named in the registry but not
yet computable would come back as `value: None` with its reason, never an
error — none is today. `unit_cost`/`tokens_per_run` count only runs whose
usage the runtime measured start to end; an unmeasured run is left out,
never taken as free, and with none left the value is `None` with a reason.
`estimate_accuracy` follows the same rule for a run with no `original_estimate`
at all -- left out of the share, never scored as a miss. `ids` empty means every non-parameterised metric
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
over a window ending now is a current fact even when it is zero. The intake
metrics (above) follow the same rule: as of the newest decision event
counted, never the moment asked for. `backup_age_hours`/
`backup_verified_age_days` (#154) are as of the instant L1 Backup's own
state was captured (`BackupFact.at`), read at most once per call; `None`,
with the reason, when no backup is configured, the destination cannot be
reached, no snapshot exists yet, none has ever been verified, or the newest
verification failed — never a bare `0`. This is
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
person's own what-if); `unit_cost`, `tokens_per_run` (registry-backed since
#117, but not in the forecast formula until #117 v3, so the What-if panel
keeps their sliders disabled). The one v1 formula:
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
expected to read this field and render it on the dashboard. A signpost
names any registry metric with no scenario code of its own required to
support it -- `backup_age_hours` and `backup_verified_age_days` (`#154`,
"Policy facts and metrics" under "Backup") work exactly the same way as
`throughput_week` or `unit_cost`:
`signposts: [{ metric: backup_age_hours, above: 30 }]` reads `Triggered`
once the newest snapshot is more than 30 hours old, a scenario-authored
signpost on top of the backup job's own drills (`#156`), not a
replacement for them.

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

L5 Improvement's third tab (`#107`): which qualities matter for each scope,
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

## Intake

L4 Process's second tab (`#119`), next to Tasks: the inbound quality gate
before anything becomes a task. An item arrives as a task held in
`TaskStatus::Intake` — no run, not on any board a scope works from — and is
triaged against seven built-in readiness axes (scope, specification,
verifiability, observability, context, independence, reversibility), each
pass or fail with one sentence of evidence; a category; a priority from
impact × urgency; a range estimate from a complexity level (1–10); and a
route to a scope and, optionally, a workflow. `factory_core::intake` is pure
— no store, no clock but the one passed in — and `factory-daemon/src/intake.rs`
owns the transitions: receive, triage, assess, decide (`ready`, `needs-info`,
`split`, or `wontfix` with a verified reason).

**GitHub receipt and decomposition (`#180`).** A scope whose `git` is a
GitHub remote polls open issues carrying the explicit `factory:intake` label.
Title, body and comments are copied into one Intake item. While that item is
still in Intake, later edits synchronize; a changed issue in `needs-info`
returns to Received for triage. Once released, Factory is canonical and the
GitHub issue is only an outbound mirror. The source keeps repository, issue
number and GitHub's stable external id as well as the public URL, so a renamed
repository does not create a second item; older URL-only rows are upgraded on
their next poll.

The daemon first probes one fixed labelled-issues REST URL with its ETag.
`304 Not Modified` ends that repository's poll without the heavier
comment-rich read; a changed ETag triggers the full synchronization. This is
deliberately polling rather than a webhook: a local daemon needs no public
endpoint.

An assessment may include two to eight `split` parts. The original shape is
still a proposal for the manual `intake decide <id> split` fallback. A plan
whose parts also provide `owns`, `interface`, and `estimate_seconds` is
executable: `intake assess --decide` validates standalone instructions and
acceptance, an acyclic graph, estimates, and disjoint ownership between
parallel parts. A valid automatic plan starts one generated workflow run,
without another approval stop. Its `expand` node materializes ordinary child
tasks carrying real `parent_task_id`, `decomposition_part`, and task-id
`depends_on` fields; labels are only a compatibility mirror. Root tasks start
immediately. Successors appear scheduled and are dispatched only after their
prerequisites have completed and been merged; their prompts receive each
prerequisite's result as upstream output. The normal L3 session limit still
bounds dispatch. A failed or missing prerequisite never silently releases a
child. Automatic plans are one level deep, leave `routing.workflow` empty,
and do not create GitHub child issues.

For a GitHub item, Factory fetches `origin/main` once and creates
`factory/issue-<number>`. Each runnable child branches from that integration
ref as it then stands. The workflow's single-writer integrator merges clean,
committed child branches in dependency order; a conflict returns only that
child with concrete rework feedback. After all merges, every part's acceptance
command runs again in the combined worktree. A failed combined check likewise
returns its owning part, up to the expand node's rework limit. On success the
integrator pushes without force and opens one PR to `main`, whose body lists
the internal parts and closes the source issue. Factory never merges, approves
or enables auto-merge on that PR. It records the PR before removing child and
integration worktrees and merged local branches, so no unpushed result is
discarded.

**Per-scope definitions of ready (`#169`).** The seven axes are fixed —
compiled in, never removed — but a scope can add its own checks on top of
them, and tighten two of the built-in rules, the same add-or-tighten,
authored-content pattern `quality.rs` uses for quality profiles. See
`crates/factory-core/src/ready.rs` for every field; its module doc records
the one deviation from the issue that first asked for this (below).

- **Files.** `<root>/.factory/intake/ready.yaml` is the root layer: it
  applies to every scope automatically, with no list to bind it — the one
  way this differs from quality's top-level `quality: […]`. Every other
  file, `<root>/.factory/intake/<name>.yaml`, is bound by a nested scope's
  own `scope.intake: [name, …]` for itself and every scope below it by path
  (`Config::intake_chain_for_scope`, ancestry by `Scope.path`, resolved
  root-first — a sibling never inherits, and the instance root's own scope
  entry refuses `scope.intake` outright, since it has no list of its own to
  move a binding to).
- **Schema.**

  ```yaml
  # .factory/intake/security.yaml
  checks:
    - id: threat-model                 # slug; an addition to the seven built-in axes
      pass_condition: "A security-relevant change names its threat model."
      categories: [security-report]    # optional; absent means every category
  max_complexity: 6                    # 1-8, default 8; complexity 9-10 always needs info
  observability_tolerance: low         # medium (default) | low | none
  ```

- **Add or tighten only.** Checks are combined by `id`, first-declared
  order down the chain (a file bound at two layers folds once, at the
  higher one). A descendant redeclaring an inherited check may only widen
  its `categories` (absent is already the widest — every category);
  `max_complexity` and `observability_tolerance` take the stricter of every
  declared value. Anything looser — a narrower `categories`, a higher
  `max_complexity`, a more permissive tolerance — is a finding, and the
  inherited value is kept, never silently loosened.
- **Fail closed.** A file that does not parse, or a name a `scope.intake`
  binds with no file behind it, is never a silent fallback to what an
  ancestor declared: it is a visible finding, and every assessment routed
  to that scope gets its own blocker — "definition of ready for `<scope>`
  could not be read: …" — so the verdict is needs-info until it is fixed. A
  broken nearer file can therefore never weaken inherited readiness without
  anyone seeing it. With no `ready.yaml` and no bindings anywhere, the
  effective definition is exactly today's seven axes, unchanged.
- **Enforcement.** `Assessment` gains a `checks: Vec<CheckResult>`
  (serde-defaulted, so an assessment made before this feature still
  deserialises and validates). `validate` requires exactly one evidenced
  result for every check the routed scope's effective definition applies to
  the assessment's own category, and refuses an unknown or inapplicable id.
  `evaluate` blocks on a failed applicable check, on complexity over the
  scope's own `max_complexity` (independently of the fixed 9-10 rule), and
  on a failed Observability axis whose cost the scope's own tolerance does
  not cover — `medium` (the default) tolerates low or medium cost exactly
  as intake always has; `low` only low; `none` tolerates nothing.
- **Surfaces.** The triage run's instructions list the item's own scope's
  extra checks and limits, and extend the submitted JSON's shape with
  `checks`. `IntakeBoard`'s routes each carry their scope's effective
  definition and findings (`board.axes` itself stays the seven built-in
  axes) — the assess dialog renders the extra checks against the routed
  scope, and `factory intake` / `factory intake list --scope` prints them.
  Every one of these re-reads `.factory/intake/` fresh on each request, the
  same no-cache rule quality and policies follow.
- **Backup.** `.factory/intake/` is authored content, copied whole in every
  snapshot and parsed (never enforced) by `factory backup verify`, which
  warns — never fails the backup — on a file it cannot read.
- **Deviation from the issue text.** The issue names one file,
  `.factory/intake/ready.yaml`, as if it were a scope's own. Two rules argue
  against that literal reading — AGENTS.md's "a scope owns only its
  `.factory/config.yaml`", and `backup::AUTHORED`, which only ever walks
  paths under the instance root — so every definition lives under the
  instance root instead, exactly as quality profiles and policy catalogues
  already do, and a nested scope opts in with a name rather than a path.

`factory intake [--scope S]` (or `factory intake list --scope S`) prints the
board: every open item by column, the routes it can go to, and — since
`#169` — each route's own extra checks, limits and findings. `factory intake
show <id>` is one item's whole record. `POST /api/intake/<id>/assess` is
where `validate` and `evaluate` are enforced; `GET /api/intake` answers the
`IntakeBoard` the UI and the CLI both read.

**Reference-class estimates and measured accuracy (`#168`).** An assessment's
range is `ir:triage`'s fixed complexity table (1–2: 15–45m, … 7–8: 2.5–5h) by
default, but Factory replaces it with measured evidence when enough exists —
completed work in the *same exact canonical scope* (never a subtree, so one
project never trains another's estimate) and the *same effective category*.

- **The sample.** `Done` tasks whose runs are all terminal, whose last run
  ended in the trailing 90 days, that are not themselves a triage
  bookkeeping task (`intake-triage`) and that have at least one run. A
  task's time is the sum of its runs' wall seconds (the same total `#117`'s
  task comparison measures); its cost is the sum of its runs' final
  measured cost, and only when every run has one — one run with an
  unmeasured or partial reading makes the whole task's cost unknown, never
  zero.
- **Harness narrowing.** The routed agent's own class (scope + category +
  agent) is used instead of scope + category alone, but only when that
  narrower class by itself has at least 5 samples; otherwise the agent is
  not counted as evidence and the wider class is used.
- **Percentiles.** At least 5 samples gives nearest-rank p10/p50/p90, for
  time and, independently (its own sample count), for cost. Fewer gives an
  explicit "insufficient evidence (n of 5)" rather than a guess.
- **Precedence.** The assessor's own range (still only with a named driver
  in the summary) beats the reference class, which beats the complexity
  table. `Triage.estimate_basis` records which one applied — source, scope,
  category, the agent when it narrowed the class, the window, the sample
  counts and percentiles, and the fallback reason — and is what the intake
  card, `factory intake show` and the `triage_verdict` journal entry read to
  say, for example, "p10–p90 of 12 completed bugfix tasks in factory, last
  90 days" or "complexity table: 2 of 5 samples". `intake::Estimate` itself
  gained an optional `expected_seconds` (the explained projection — a
  reference class's p50, or an assessor's own — as opposed to the plain
  average `midpoint()` falls back to without one) and an optional `cost`
  range; both are serde-defaulted, so a `Triage` from before this shipped
  still deserialises exactly as it read before.
- **On release**, the patch now writes the task's full `Estimate` range
  (not only `estimate_seconds`, its expected value) and — the fix this
  issue makes — sets `Task.category` from the assessment itself, not only
  the `category=` label a released task already carried. This means a
  released item now also picks up any control plan (`#118`) declared for
  its category, which an intake-released task did not before: the
  category label was cosmetic, and the field is what `#117`'s reference
  classes and `#118`'s control plan actually key on.
- **Measuring the accuracy** of every estimate, however it was set, is the
  `estimate_accuracy` registry metric (see "Goals") and the cost report's
  own `estimated_runs`/`within_range`/`median_actual_over_expected` (see
  "What a run used").

Left out on purpose: re-estimating at the first turn (`#117` already owns
that), budgets and forecasting (`#164`), and pooling samples across scopes
or up a subtree.

**The security fast lane (`#170` phase 1).** A possible security report gets
ahead of the ordinary queue and nothing may quietly release, split or close
it away.

- **Flagging.** `Intake.security: Option<SecurityFlag>` holds the state
  (`possible` | `confirmed` | `dismissed`), who flagged it and when, a
  reason, and -- once a person has looked -- who decided, when and on what
  evidence (required to dismiss, optional to confirm). Three sources, all
  deterministic, no classifier: `intake add --security` (or the UI
  checkbox) at receipt, `intake flag-security <id> --reason "..."` on an
  item already in the gate, from anyone who could assess it, and an
  assessment whose category is `security-report`, flagged automatically the
  moment it is recorded. Flagging only adds scrutiny, so it needs no more
  than `intake.assess` already grants; refused once the item already
  carries a flag of any kind -- a decided flag is not reopened by a second
  one, and a `possible` one is not restated.
- **The fast lane.** `board()` sorts a `possible` or `confirmed` item ahead
  of everything else in every open column (received, triaging, needs-info),
  oldest first within that band; a `dismissed` one sorts as ordinary. The
  card carries the flag, and the triage instructions say a suspected
  vulnerability is always `security-report`, whatever else it might also
  look like, and is never proposed `wontfix`.
- **No auto-rejection, and no auto-release either.** While a report stays
  `possible`, `check_decision` refuses `ready`, `split` and `wontfix` for
  every caller -- needs-info still goes through, since asking the requester
  is not a rejection. Once confirmed, `wontfix` alone stays refused: a real
  security report is not "won't fix", only a dismissal is. Once dismissed,
  the item is ordinary again.
- **Confirm or dismiss.** `IntakeSecurity { id, verdict: confirm | dismiss,
  evidence }` is the one decision `Needs::Owner` gates outright -- an agent
  may flag, whatever role it holds, but never decide (AGENTS.md's "bounds
  what an agent does by accident, not what it could do", not a stronger
  claim). Both verdicts are journaled (`intake_security_confirmed` /
  `intake_security_dismissed`) with the evidence.
- **Durability.** A confirmed report is CRA evidence: `TaskDelete` refuses a
  task that carries one, naming why, and the record survives release --
  `Intake` (and the flag on it) stays on the task whatever stage or status
  it reaches next.
- **The fact, ahead of fact ports.** `Engine::confirmed_security_reports`
  reads every confirmed report live over a scope's subtree, including an
  item that has since left intake -- `awareness_at` is always
  `Intake.received_at` (for a GitHub-sourced item, the issue's own
  `createdAt`, never the confirmation or fix time). Exposed as
  `Request::IntakeSecurityReports`, `GET /api/intake/security-reports` and
  `factory intake security-reports`; plain serde data with no methods, so
  `#193`'s later L0 fact port can take it unchanged. The CRA reporting
  clock that turns awareness into the 24-hour, 72-hour and 14-day deadlines
  is `#157`, phase 2 of this issue -- intake persists and exposes the fact,
  and never computes a deadline or calls up into the clock itself.
- **Surfaces.** CLI: `intake add --security`, `intake flag-security <id>
  --reason "..."`, `intake security <id> confirm|dismiss [--evidence
  "..."]`, `intake security-reports [--scope]`. HTTP: `POST
  /api/intake/{id}/flag-security`, `POST /api/intake/{id}/security`, `GET
  /api/intake/security-reports`, beside the other intake routes. UI: a
  badge and a left band on a fast-lane card, the flag's detail in the item
  modal, and confirm/dismiss buttons with an evidence prompt -- the browser
  sends no token, so every UI caller already is the owner `IntakeSecurity`
  requires. Once confirmed, the card and item modal also show its reporting
  clock deadlines (`#170` phase 2, `ui/js/clock-model.js`), read through
  `GET /api/policy/clock` -- never computed here either. The agent guide's
  `intake.assess` line mentions `flag-security` and says confirming or
  dismissing is the owner's alone.

Left out on purpose: any countdown state of intake's own (the reporting
clock -- see "Policies" below -- computes deadlines fresh on every read, and
`#170` phase 2 shows a confirmed report's own 24-hour/72-hour/14-day deadlines as
badges on its card and in the item modal, and on the Inbox), notifications,
a text or LLM classifier, and GitHub security advisories. The 14-day final
report appears only after evidenced corrective-measure availability is recorded.

**Outbound: approved GitHub triage comments and labels (`#171`).** Intake's
first outward effect: one maintained triage comment plus labels on the
GitHub issue a decided item came from. The daemon never posts anything on
its own -- a decision only ever records that publishing is possible; a
person, or a role given the standing permission, has to ask for it.

- **The grant.** `intake.publish` is checked exactly like every other intake
  grant (`access.rs`'s `Needs::Grant`), but it is the one outward-effect
  grant intake has, and that is deliberate: left out of the `foreman` preset
  (otherwise `Grant::ALL`) and the `triager` preset, and excluded from
  wildcard expansion (`*`, `intake.*`) -- `Grant::expand` names it exactly
  or not at all. The owner always passes; publishing itself is the
  approval.
- **Areas.** `Assessment.areas: Vec<String>` (serde-defaulted, slug-validated
  like `category`) lets a triager name what part of the system an item
  touches -- `process`, `quality`, `intake` -- carried straight through to
  the released GitHub item's labels.
- **The comment and the label plan, pure (`factory_core::intake`).**
  `triage_comment(item, triage, decision)` renders the triage skill's own
  assessment format, starting with `> *This was generated by AI during
  triage.*` and ending with a hidden marker, `<!-- factory-intake:<item-id>
  -->`, that a re-triage's publish finds and edits in place rather than
  posting again. `github_labels(triage, decision) -> LabelPlan { add,
  remove }`: category maps `bugfix` -> `bug`, `docs` -> `documentation`,
  everything else -> `enhancement`; state maps `ready` -> `ready-for-agent`,
  `needs_info` -> `needs-info`, `wontfix` -> `wontfix` (plus `duplicate` or
  `invalid` for those two wontfix reasons); areas are added as given;
  `remove` is always the other two state labels, so the three stay mutually
  exclusive on the issue. `factory:intake` is never touched -- the poller keys
  on it, and removing it is a person's own decision.
- **Stored state.** `Intake.outbound: Option<Box<OutboundRecord>>`
  (`awaiting_approval` | `published` | `failed`, the comment id and URL, the
  labels applied and skipped, a digest of the last effect, the last error) --
  boxed for the same reason `security` is. A decision on a GitHub-sourced
  item that carries an assessment records a fresh `awaiting_approval`,
  replacing whatever was there; a `Decision::Split` and a possible or
  confirmed security report (see above) never get one -- a vulnerability
  report is never disclosed in a public comment, and `intake publish`
  refuses it again even if something else got an item this far.
- **`factory-daemon/src/github_outbound.rs`.** `intake_publish` accepts only
  a `SourceKind::Github` item whose stored reference passes the poller's own
  canonical-URL validation (`github_intake.rs`'s `parse_canonical_issue_url`,
  shared rather than re-derived). It journals `outbound_attempted` with a
  digest, then: `gh label list` (only a label that exists is applied; one
  that does not is skipped and journaled, never created); `gh api
  .../issues/<n>/comments --paginate` to find the comment carrying this
  item's marker; PATCHes it, or POSTs a new one; `gh issue edit
  --add-label/--remove-label` for whatever the label plan and the
  repository's own labels agree on. Each call gets the poller's 30-second
  timeout. The `gh` program's path is injectable, exactly like the poller's
  own tests inject a fixture in place of the real binary. A failure at any
  point is caught, journaled as `outbound_failed` with the error, and left
  on the record -- never a panic, and never something that stops the
  engine; replaying finds the marker and edits in place, so nothing is
  posted twice even after a crash between the GitHub write and the local
  one.
- **Surfaces.** CLI: `factory intake publish <id>`. HTTP: `POST
  /api/intake/{id}/publish`, beside the other intake routes -- every browser
  caller is the owner, which always passes. UI: "Approve and post to GitHub"
  on a decided GitHub card (needs-info or, since a released item still shows
  its card for the ready window, ready too), with the state, the comment
  link, and any skipped labels or the last error. The agent guide's
  `granted_command_lines` gets an `intake.publish` line, shown only to a
  role that holds it.
- **Left out on purpose:** closing, reopening or assigning the GitHub issue;
  a comment or label for a `split` or an undecided item; GitHub Enterprise;
  webhooks; the Inbox; a non-GitHub source; creating a label that does not
  exist.

**Intake sources: email and chat (`#167`).** Two more source kinds,
`email` and `chat`, alongside `cli`, `ui`, `agent` and `github` -- a way for
a conversational request (a mail, an iMessage) to reach the same quality
gate everything else does, instead of becoming a task directly.

- **Relayed, not trusted.** The daemon runs no mail or chat client of its
  own; something else -- today, the company assistant's `factory_task`
  tool -- reads the message and hands it in. Any caller holding
  `intake.add` may relay one, Owner or Agent alike: `factory intake add
  --source email|chat --provider <name> --reference <message-id>
  --requester <who> [--received-at <RFC3339>]`, or the same fields on `POST
  /api/intake`. The daemon trusts nothing about *what* is claimed beyond
  that it is relaying: `IntakeSource.relayed_by` is always the caller's own
  `Caller::describe()`, recorded next to the claimed `provider` and
  `reference`, never something a caller can spell as somebody else.
- **What a relay must give.** `--reference` is the provider's own message
  id and is required -- the free-text "an issue URL, a mail id" every other
  kind's `reference` already was, made load-bearing here since it is also
  the identity. `--requester` is the sender's own address or handle and is
  required too; unlike an agent's ordinary delegated item (folded into
  `"<on-behalf> (via <caller>)"`), a relay keeps `requester` and
  `relayed_by` as two separate fields. `--received-at` is the provider's own
  receipt time -- refused if it is in the future, defaulted to now when
  absent. `--provider` (`apple-mail`, `imessage`, …) is free text and
  optional. `github` is never accepted this way -- it stays the poller's
  alone -- and `--provider`/`--received-at` without `--source email|chat`
  is refused outright, the same as a missing message id or requester: a
  malformed relay, not a fallback to an ordinary item.
- **Identity and replay.** An email or chat item's identity is `(kind,
  provider, reference)` -- the same shape GitHub's canonical issue URL
  already gave `github`. `Engine::receive_intake` looks an item's identity
  up and creates it under one lock (`#166`'s poller went through the exact
  same path already; this generalises it), so a relay that retries --
  hands in the same message twice, or two callers relay it at once --
  finds the item it already made and returns it unchanged, with no second
  `intake_received` journal entry. `cli`, `ui` and `agent` items are
  unaffected: they still always create, exactly as before, and `#166`'s
  duplicate-candidate search is their only signal that a repeat came in.
- **Surfaces.** `intake show` and the board print the kind, the provider
  when there is one, and who relayed it. `ui/js/intake-model.js` labels
  the two new kinds.
- **Left out on purpose:** an adapter that reads mail or chat itself --
  the daemon is handed items, never fetching them; autonomous ingestion
  by any assistant-side watcher (a human or its own policy decides what
  gets relayed); a Gmail bridge; replies or any other outbound effect;
  updating an item when its source message is later edited; duplicate
  detection across sources.

## Line

L4 Process's last tab (`#106`), next to Tasks, Intake and Workflows: how the
line is running, exception first. It was called Operations until `#185` gave
that word to the running systems (see "Operations" below); the API and the
CLI keep their names (`/api/operations`, `factory stats`), and an old
`#<scope>/proc/operations` link opens Line. **A picture, not a controller** (design §8):
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
  how many runs are waiting on a retry. `sessions_max` states the scope's
  own `max_sessions` (`#179`, see "Agents" below) where one is declared,
  and stays absent otherwise -- never a made-up limit. An agent's own cap
  is enforced at dispatch but is not summed into this figure: two agents'
  limits do not add into one meaningful scope ceiling. `sessions_in_use`
  counts a run still dispatching or already holding a session -- an
  approval hold with no session yet does not spend one. A task held on
  either cap counts in `queue_depth` as an ordinary queued item, aged from
  when the hold began.
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
factory task close <id> --reason not_planned --note "..."   # task.close; journals closed
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

## Operations

The systems Factory builds and runs (`#185`), as an operator means the word:
for every environment -- staging, production, a review environment -- what
release runs there, whether it is healthy, whether it meets its SLO, and how
releases fare. Not Factory's own production line, which is L4 Line above.
`factory_core::environments` is the pure model and every decision in it;
`crates/factory-daemon/src/environments/` records, checks and reports.

**Environments are declared** in the scope that deploys them, beside its
agents:

```yaml
# a scope's .factory/config.yaml
scope:
  name: factory
  environments:
    - name: production
      tier: production                 # production | staging | ephemeral (the default)
      url: https://factory.example.ts.net:8790
      checks:
        - { kind: http, path: /api/status, expect: 200, every: 60s, timeout: 5s }
      slo: { availability: 99.5%, window: 28d }
    - name: staging
      tier: staging
      promotes_to: production
      url: https://factory.example.ts.net:8791
      checks:
        - { kind: http, path: /api/status, body: '"instance"', every: 60s }
        - { name: disk, kind: command, command: 'test "$(df -P . | awk "NR==2{print \$5+0}")" -lt 90' }
      slo: { availability: 99%, window: 28d }
```

An environment's name is unique across the instance, because it names
metrics (`availability.staging`); `promotes_to` may name one in another
scope. A check is `http` (a GET through `curl` on the daemon's `PATH` -- the
daemon has no HTTP client, and what is worth checking is behind TLS -- with
an expected status, 200 unless said, and an optional body substring), `tcp`
(`host`, `port`), or `command` (run with `sh -c` in the scope's directory;
exit 0 is healthy). `every` defaults to 60s and is at least 5s; `timeout`
defaults to 10s and is at most 120s and never longer than `every`. `paused:
true` stops an environment's checks: its status reads `unknown`, and a paused
stretch neither spends nor earns error budget. Everything is checked at load.

**Health.** A loop in the daemon runs every due check of every declared,
unpaused environment, reading the configuration fresh each tick. Each check
runs in a task of its own, in its own process group, killed at its timeout;
one that hangs, cannot start or panics is a failed **sample**, and nothing a
check does can take the daemon down. An environment is `up` when every check's
latest answer was healthy, `down` when every one failed, `degraded` between,
and `unknown` before anything was checked -- with how long it has been so.
Two consecutive failures of a check open an **incident** from the first of
them, the next success closes it, and overlapping incidents of different
checks are one. Only a change of status is published
(`environment_status_changed`), never a sample. Samples are kept 90 days --
the one table here that is pruned, since a sample is an observation and not a
record of anything anyone did -- so an SLO window is at most `90d`.

**Deployments** are recorded by whoever makes them, in an append-only
`deploy_events` table, so the history survives every release and restart:

```sh
id=$(factory deploy start --env staging --commit "$SHA" --describe "$(git describe)" \
       --profile release --via release.sh)          # prints the deployment id
factory deploy finish "$id" --status succeeded      # or failed / rolled-back --reason "..."
factory deploy record --env review13 --commit "$SHA" --status succeeded   # by hand, in one go
factory deploy list [--env staging]
factory release add --scope factory --commit "$SHA" --version v0.4.0
factory release list
factory env [<name>] [--scope S]
```

A deployment records its release (commit, describe, version, profile, dirty,
source, and the commit's committer time -- asked of the scope's repository
when not given -- where lead time starts), its actor (the run and task whose
token recorded it, a standing agent, or the owner), `via` (what recorded it:
`release.sh`), `manual` (the owner with nothing in between), start and end,
status (`running`, `succeeded`, `failed`, `rolled_back`), the reason, and the
release that was running before -- what a rollback targets. **A success is
verified**: the environment's own checks run once, right then, and a
deployment only counts as `succeeded` if they pass; otherwise it is recorded
as `failed` with the failing checks, and `deploy finish` exits non-zero.
`--no-verify` skips that and says so on the record. A new deployment to an
environment with one still `running` ends the old one as failed, naming the
new one, rather than leaving it running forever. An environment nobody
declared -- a throwaway review environment -- is still recorded, as
`ephemeral`, without checks or an SLO. Recording needs `deploy.record` in the
environment's scope; with `own` reach only from a run, and a run finishes only
a deployment it started. Reading is open to every agent.

The **release catalogue** is every `(scope, commit)` a deployment or
`release add` recorded -- builds that actually happened, not git tags --
with where each is running now and how many of its deployments failed.

**SLA and release effectiveness** are computed from samples and deployments,
never typed in, and are metrics in the registry like any other -- the Goals
tab, the Scenarios drivers and dashboard tiles can read them:

| metric | what |
|---|---|
| `availability.<env>` | healthy samples over the SLO window (28 days without one) |
| `error_budget.<env>` | what is left of the budget: 1 untouched, 0 spent, below 0 overspent |
| `incidents.<env>` | incidents in the window |
| `mttr.<env>` | mean incident duration, over incidents that ended in the window |
| `deploy_frequency.<env>` | successful deployments per week (DORA) |
| `lead_time_p50.<env>` | committer time to deployment finished, median (DORA) |
| `change_failure_rate.<env>` | finished deployments that failed, were rolled back, or were followed by an incident within an hour and before the next deployment (DORA) |
| `time_to_restore_p50.<env>` | median incident duration (DORA) |

DORA's keys are read per environment; the one at the end of a promotion path
(production) is the one that speaks for the path. A figure with nothing to
compute it from is `None` with the reason -- no samples is not 100%.

**The tab** is L1 › Operations (`#<scope>/infra/environments`), narrowed by
the rail's scope: environment cards in promotion order (status and since,
current and deploying release, uptime 24h / 7d / SLO window against target,
error budget, last check, a 24-hour strip per check, incidents, the DORA keys
and MTTR); the deployment timeline, running first, failed and rolled-back ones
standing out, with who, via, duration and verification; and the release
catalogue. Polled every 30s and refetched on `deployment_updated` and
`environment_status_changed`. `GET /api/environments?scope=` answers it.

**Factory's own environments.** `.claude/skills/release/scripts/release.sh`
records every release with the company daemon, whichever environment it went
to: `deploy start` before the swap, `deploy finish` once the environment is
up and reachable (a start the old CLI could not record is recorded whole at
the end), a failed release as failed. It never fails a release because the
daemon could not be reached. `envs.conf` still gives the scripts their ports
and policies; the `environments:` declaration above, with the real Tailscale
URLs, belongs in the `factory` scope's config once a daemon that reads it is
installed.

**Not yet:** promoting a release from the page (a `release` task gated by
policy and approval, `#118` v2), mirroring deployments to GitHub's
Deployments API, `ensure.sh`'s restarts as journaled actions, "slow" as a
reason for `degraded`, alerting beyond Factory's own events, external
monitoring as a check source, and more than one host.

## Tasks and runs

A **task** is the standing intent: what to do, where, with which agent, and on
what schedule. A **run** is one attempt at it — the session it opened, what it
reported, when it ended.

Running a task a second time makes a second run. A task that failed and is
started again has two runs, numbered `attempt 1` and `attempt 2`, and both are
kept with their own journal, their own outcome, and their own terminal
transcript. The task itself mirrors the newest run, so a list stays cheap to
read; the history lives on the runs.

Creating a task does not start it. There is no queue behind `pending` and no
capacity for a task to wait on: the scheduler starts only a scheduled slot
that has come due (or a queued retry), so an unscheduled task stays `pending`
until someone runs it -- `--run` on create, `factory task run <id>` later, or
the Run button. `task create` says which it is, the task's page says so while
nothing has started it, and the dashboard's **Due** figure counts only what
the scheduler will actually fire -- the same number as Line's
`flow.queue_depth` -- with manual tasks named beside it, not in it (`#124`).

### A failure is Blocked; closing is a person's act (#122)

A run that fails stays `failed` -- the attempt really did fail, and every
`FailKind` says why. Its **task** does not: it goes to `blocked`, with
`failure` (the fail kind, the run, the attempt) mirrored beside the run's
`error`, because Blocked is the one column a person has to act on. The
board's card says which kind of block it is -- *attempt 2 failed: ran past
its timeout* reads differently from an agent waiting on a question. It is
never a blocked *run*, so `blocked_timeout_seconds` never finds it and it
cannot time out into another failure.

A scheduled task keeps its retry streak: while a retry is queued it stays in
Scheduled and says it is retrying; once the retries are used up, or with
`retry: none`, it is blocked on the failure instead of looking healthy. Its
schedule keeps firing, and the next run that succeeds clears the block.

Closed is `done`, or closed on purpose with a reason -- never the default
outcome of a failure:

```sh
factory task close <id> --reason not_planned --note "the client dropped it"
factory task close <id> --reason duplicate --duplicate-of <other-id>
factory task close <id> --reason completed      # it was done by hand
factory task reopen <id> --reason "worth another go"
factory task list --status failed               # blocked by a failed run
factory task list --status closed               # done or cancelled
```

`POST /api/tasks/{id}/close` takes `{reason, duplicate_of?, note?}`,
`POST /api/tasks/{id}/reopen` `{reason?}`. Closing works with no run at all
and is refused while a run is active (cancel it first); it is journaled
(`closed`) with who closed it and the note, and needs the `task.close`
grant, which also covers reopening. A person cancelling a running task still
closes it, as *won't do*. A new run clears the close record and the failure,
like every other mirrored field. Tasks stored as `failed` before this are
moved to blocked-on-that-failure when the daemon starts (`migrated` in their
journal).

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

### What a run used (#117)

Every run records what it used — tokens by type, API-equivalent cost with the
price table it was priced by, the model — as the **agent runtime** observed
it. Factory is the process layer: it never reads a harness transcript, never
learns a harness's file layout, and never talks to an observability tool
directly. Usage reaches it through one door, `AgentRuntime::usage`, which
answers in the versioned contract `factory_core::usage::SessionUsage`
(schema 1, cumulative per harness session). A runtime with no source for it
answers `None`.

herdr answers through a plugin: whichever installed herdr plugin offers an
action called `usage` (Irrlicht's) is invoked over herdr's socket API with the
run's pane as `context.focused_pane_id` — the pane reaches the plugin as
`HERDR_PANE_ID` — and its stdout is read back from the plugin command log.
`herdr plugin action invoke` itself cannot be used: it has no pane argument
and acts on whichever pane is focused. Nothing names the plugin; installing or
removing it takes effect on the next run. With two plugins offering `usage`,
`FACTORY_HERDR_USAGE_PLUGIN` picks one. A read is bounded to five seconds.

A run is read three times: once its session is up and before the task is
handed over (the **baseline**), at every turn end the harness reports, and as
it ends, before the session is closed. Each reading is kept append-only
(`factory run usage <run-id>`), answered or not, and the run's `usage` is the
difference between the baseline and the newest reading that answered — so a
pane reused from an earlier run is never billed twice. Rules:

- **Never a guessed number.** A count the runtime could not see is `?`, and a
  total with one unknown part is unknown. A run whose runtime had no answer is
  `unknown` with the reason, never `$0.00`; so is a pane with no harness
  session in it at all.
- **A lower bound says so.** Figures as of a turn end because the last
  reading failed are marked partial ("at least").
- **Prices are snapshotted.** Every cost carries its `pricing_source`.
- Subagents are listed apart from their parent in the contract and are added
  to it.

```sh
factory run show <run-id>        # the usage block
factory task show <id>           # every run's usage and the sum
factory cost --by issue --since 7d   # task | issue | scope | agent | provider | workflow
factory cost --by agent --scope projects/factory --since 2026-09-01
factory cost --by provider --since 30d
factory cost --by workflow --since 7d
```

`GET /api/costs?group_by=&from=&to=&scope=`, `GET /api/tasks/{id}/usage` and
`GET /api/runs/{id}/usage` answer the same. Grouping by `issue` reads the
task's `issue=<n>` label; grouping by `workflow` (#164) reads the task's own
`workflow_origin.workflow_id`, labelled with that workflow definition's name
-- `(no workflow)` for a standalone task, `(deleted task)` when the task
itself is gone (its workflow is unknown, never "standalone"). Both
`factory cost` and `GET /api/costs` read `Engine::spend`, the one path every
consumer of spend -- the CLI, the HTTP endpoint, and the `cost_week` metric
below -- calls. Sums are over the runs that knew the number, and beside them
is how many did not: an unmeasured run is counted, never dropped and never
free. The registry metrics `unit_cost`, `tokens_per_run`, `cost_week` and
`estimate_accuracy` (#168) read the same usage and run data (see "Goals").

Each group's row also carries its own estimate-vs-actual (#168):
`estimated_runs` (how many of the group's runs carry an `original_estimate`
at all), `within_range` (of those, how many terminal ones landed inside their
own `[low, high]`), and `median_actual_over_expected` (the nearest-rank
median of actual wall seconds over the estimate's `expected`, over the same
terminal ones). A run with no estimate is in none of the three -- it has no
range to have missed, never a silent zero. `factory cost --by …` prints them
as an `ESTIMATE` column (`3/5 in range, 1.20x median`, or `-` when nothing in
the group carries one); `GET /api/costs` carries the same fields on every
`CostRow`.

Tasks can carry low/expected/high time and cost estimates. The CLI accepts
`--estimate-low`, `--estimate`, and `--estimate-high` (and the corresponding
`--estimate-cost-*` flags); the old expected-only form remains a point range.
The effective original estimate is copied onto each run, so later task edits
do not rewrite history. At the first turn end Factory records one re-estimate
from completed runs with the same canonical scope, agent, and task category,
using low/median/high first-turn-to-final factors. With no usable cohort it
records why the re-estimate is unavailable. Run usage compares each run's
wall time and cost with the range it started with, including the
actual/expected ratio and whether the actual landed inside the range; active
time is reported beside them, and has no estimate of its own to be compared
with. A task compares the sum of its runs' actuals with the sum of those same
runs' ranges, and leaves the comparison out when a run carries no estimate.
Every attempt carries the estimate it started with, so a retry adds a second
range beside the failed attempt's: the task's figure answers "these runs
against their estimates", and each run's own comparison is the one to read
for a single attempt.
Neither claims a verdict before it is final: wall time waits for the run to
end, and cost for a complete run-end reading.

For subscription accounts, positive changes in a provider's rate-limit window
are attributed to the runs active during that observation interval in
proportion to their measured token growth. One positive consumer is `direct`;
several are `apportioned`. A measured zero-token run is excluded from the
division, while a missing baseline or token measurement leaves that exact
account/window/interval explicitly unknown. Known shares and unresolved gaps
are both retained, so a later allocatable interval never hides an earlier one.
Intervals are bounded by when the runtime sampled each reading (the contract's
`sampled_at`), not when Factory asked, so a cached or delayed answer is charged
to the runs active when it was sampled; a runtime that gives no sample time
has the request time stand in.

Not yet (v3 and later): budgets, Scenario cost drivers, the `budget_within`
policy check, and runtime-specific observation work tracked outside Factory.

## Workflows

A workflow is reusable Process-level intent: a scoped, finite DAG of ordinary
task templates. A workflow run keeps an immutable snapshot of the definition
revision it started with. Root nodes create and run tasks immediately; every
other node waits until all incoming predecessors have reported `done`.
Fan-out starts every newly eligible node and fan-in waits for every parent.
A failed or cancelled task stops the attempt and leaves downstream nodes
`skipped` -- a task blocked by a failed run is a failed node, even though
the task itself waits in Blocked for a person; a task blocked on a question
simply pauses it. The task remains authoritative for
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
do not use the canvas, including a link to any node's spawned task. The
workflow panel edits the declared **inputs**, and a task node's inspector
edits its ordered `check:` and `agent:` **exits**. The canvas draws each exit
as a labelled conditional edge with its order, condition and (for a backward
exit) round budget. Run mode distinguishes nodes skipped by a route, shows a
routed node as `done → target`, and still shows each backward round, links to
the tasks earlier rounds superseded, who sent the work back, and the values
the run was started with.

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

### Dynamic expansion and integration (#180)

An `expand` node is a daemon-owned fan-out boundary. It does not run an
agent: its immutable `expand.children` list identifies the task nodes produced
from an Intake decomposition. The generated graph carries the parts' declared
dependencies, so independent roots may run together while a dependent child
waits until its predecessors have reached the integration branch. The web
canvas labels the node `EXPAND` and shows its child count in Design and Run
views.

`expand.join.tolerate` is the number of failed independent children the join
may accept; zero is the default `all_succeeded` behavior used by integrated
code plans. `expand.cancel` is `terminate` by default, which cancels running
children with their parent, or `abandon`, which leaves already-dispatched
children alone. Integration-backed expands also carry a bounded
`max_rework_rounds`: merge conflicts and acceptance failures return the
responsible child with the same task identity and a new run. A resumable
harness continues its session; other runtimes start a fresh run from the
current integration ref. All of this state—the base ref, merge ledger, checks,
PR URL and cleanup—is stored on the workflow run, so restart reconciliation
continues rather than opening a second PR or losing which branches were
accepted.

### Inputs and ordered exits (#140, #149)

A workflow can be told what to work on, and a review step can send work back.

```yaml
# factory workflow create --file ticket.yaml
name: ticket
scope: factory
inputs:
  - { name: issue, description: "GitHub issue number" }
nodes:
  - id: implement
    task: { title: "Implement #{{issue}}", instructions: "...", labels: { issue: "{{issue}}" } }
  - id: review
    task: { title: "Review #{{issue}}", instructions: "..." }
    exits:
      - { to: implement, agent: "concrete findings the implementer can fix alone", max_rounds: 5 }
edges:
  - { id: e1, from: implement, to: review }
```

- **Inputs.** A definition declares `inputs:`; a run is started with a value
  for each (`factory workflow start <id> --input issue=42`, or
  `POST /api/workflows/{id}/run` with `{"inputs": {"issue": "42"}}`, or the
  form the UI's Run button opens). A missing or undeclared one refuses the
  start. `{{name}}` in a task node's title, instructions and label values is
  replaced verbatim in the run's snapshot, so the run shows what actually ran
  and the stored definition keeps its placeholders. Once a definition
  declares any input, a `{{name}}` it does not declare is refused as a typo;
  with none declared, braces are left alone (`{{.Names}}` belongs to some
  other tool). A gate's command is never substituted -- the daemon runs it
  itself. A label `issue={{issue}}` is what `factory cost --by issue` groups
  by.
- **Ordered exits.** A task node's `exits:` are checked top to bottom after
  its agent reports `done`; the first match wins. `check:` runs a bounded
  shell command in that node's worktree (`0` holds, `1` does not, any other
  result blocks the node with its output). `agent:` holds only when the agent
  reports `done --send-to <to>`; the reporting contract lists the allowed
  targets, their exact rules, and rounds left. If nothing matches, all plain
  outgoing edges remain the default. A forward exit needs a matching plain
  edge. A backward exit points to an ancestor task node, stays off `edges` so
  the graph remains acyclic, and requires `max_rounds`.

  Taking an exit is exclusive. A forward skip marks bypassed nodes
  `skipped_by_route` with a reason such as `skipped (review -> ready)`; those
  nodes count as finished, and a child is eligible when every predecessor is
  done or skipped by route and at least one is done. A backward exit sends the
  path from `to` through the reporting node back to `unstarted`. Each
  re-spawned task is
  titled `(... rework k)`, the node run keeps its earlier tasks in
  `superseded_task_ids`, and `to`'s new task is dispatched with the sender's
  `--result` as an extra upstream entry, "... sent this work back -- rework
  round k of N". Once every round is used, another `--send-to` is refused and
  tells the agent to report `blocked` with the open findings. A superseded
  task's late state changes are ignored. Stored legacy
  `rework: { to, max_rounds }` definitions and run snapshots load as one
  `agent:` exit, so an in-flight loop keeps its remaining rounds.
- **Failures do not route.** `failed` means the attempt broke and fails the
  workflow node like any other failure. Review findings that should go back
  are a successful `done --send-to`, so the review task closes completed and
  is not counted as a failure or left in the attention queue.
- **Escalating to a person** is what it always was: the agent reports
  `blocked`. The node, and so the run, waits, and the block shows in the
  Inbox until someone answers.

```sh
factory workflow list
factory workflow show <id>
factory workflow update <id> --file ticket.yaml
factory workflow start <id> --input issue=42
factory workflow runs <id>
factory workflow run <run-id>        # nodes, tasks, rework rounds
factory workflow cancel <run-id>
```

## Compliant workflows

Release outputs can be captured with
`factory task report --status done --artifact dist/release.tar` and queried
with `factory run provenance <run-id> --json` or
`GET /api/runs/{id}/provenance`. The daemon emits append-only, unsigned
in-toto/SLSA v1-format evidence only after the frozen required steps pass.
See [artifact provenance](docs/artifact-provenance.md) for the byte/source
checks, format, limits and local trust model (#158).

A run used to be `done` the moment its own agent said so. Policy controls and
quality attributes already *state* what good work includes -- an SBOM, a
security scan, tests -- but nothing obliged a run to go through those steps or
to prove it had (`#118`). Now a control, or a quality attribute, can say which
steps a **category** of work must pass, and the line enforces it: a run that
owes a step is only `done` once the step has left evidence, produced by the
daemon, an independent agent, or a person rather than by the agent that did
the work.

```yaml
# .factory/policies/house.yaml -- a control's `requires:`
- id: tested
  title: Changes are tested
  requires:
    - { applies_to: [feature, bugfix], step: tests, gate: "cargo test --workspace" }
    - { applies_to: [feature, bugfix], step: review, by: independent }
    - { applies_to: [release], step: approval, by: person }
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
  is merged into the run's immutable snapshot as locked control nodes.
  `approval` is a prerequisite before the task launches; deterministic `gate`
  nodes and then `review` nodes follow the task in plan order; whatever
  followed the node now follows its last control. Gates go after *every* task
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
  `before: publish` whose `publish` node can start with no scan before it), and
  any review for which the scope has no independent functionary. The Policy
  tab shows the same injected placement and functionary gaps for stored
  workflows.

```sh
factory workflow lint <workflow-id>                 # what a run of it gets
factory workflow lint --task <task-id>              # a task, as its one-node workflow
factory workflow lint --scope demo --category release
factory run attestations <run-id>                   # the evidence a run carries
factory run approve <run-id> --reason "release owner checked it"
factory run reject <run-id> --reason "missing release evidence"
factory run rework <run-id>                         # accept the verifier's proposal
```

The same over HTTP: `GET /api/workflow-lint?workflow=|task=|scope=&category=`,
`GET /api/runs/{id}/attestations`, and `POST /api/runs/{id}/{approve,reject,rework}`.
Attestations live in the append-only `run_attestations` table next to
`policy_attestations`.

An approval holds the run before an agent session starts. A review is assigned
to the first declared concrete task agent in stable scope order whose name is
not the subject executor; that choice is frozen in the run snapshot. The
reviewer reports plain `done` to pass or `done --send-to <subject-node>` with
concrete findings to reject. A failed gate or rejected review blocks with an
evidence-backed rework proposal. Accepting it uses the workflow's bounded
send-back path, or a same-task retry for standalone work, for at most five
rounds before a person must resolve it. Gate, Review, and Approval are distinct
locked cards on the Workflows canvas, and approval/rejection/rework decisions
are available in the Inbox. The `attested` policy check, conformance metrics,
and Goals/Scenarios wiring (#158 phase 1) use these enforced controls too;
release provenance remains a later phase of #158.

## How a task actually runs

1. `task.create` resolves the scope, agent, and runtime — from the request, then
   the scope's local config, then the instance defaults — and refuses right away
   if any of them names an adapter that does not exist.
2. `task.run` checks the agent's harness starts (see "Harness health" below),
   then opens a run, mints a callback token for it, and asks the runtime for a
   session in the scope's directory.
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
  looks like. A harness that never started at all is caught before this, by
  the probe below; an acknowledgement timeout also makes the next dispatch to
  that harness probe it again rather than trust an earlier answer.
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

### Harness health (#131)

A harness binary can stop starting without anything changing in Factory: on
2026-09-25 a freshly installed codex hung in `_dyld_start` on `codex
--version`, and every task handed to it sat in `dispatching` for three minutes
and then failed as an `ack_timeout` that blamed the agent. So before a
dispatch, the daemon checks the harness starts.

Factory also disables each built-in harness's own launch-time update check for
both task runs and standing agents: codex gets `-c` with
`check_for_update_on_startup=false`, Claude Code gets `DISABLE_AUTOUPDATER=1`,
pi gets `PI_SKIP_VERSION_CHECK=1`, and OpenCode gets
`OPENCODE_DISABLE_AUTOUPDATE=true`. These are ephemeral adapter defaults;
Factory does not rewrite a person's harness configuration, and scope-declared
arguments still follow the codex default so an explicit declaration can
override it. Scheduled host upgrades, including the active-run guard,
serialization, post-upgrade checks, and alerting, are separate work tracked in
[#173](https://github.com/ingo-eichhorst/factory/issues/173).

- **The probe is declared, not run, by the adapter.** `Agent::health_probe()`
  (a default method on the Agent seam, `None` unless overridden) names a
  command; every built-in harness answers `<harness> --version`, `shell` and
  plugins declare none. The daemon runs it -- its own process group, no stdin,
  `SIGKILL`ed with its group at the timeout, in a task of its own so a panic is
  a logged `JoinError` -- so a probe that hangs or panics cannot stall or take
  down the daemon.
- **Cached per binary, one probe at a time.** The program is resolved on the
  daemon's `PATH` (a name with a `/` is used as it is), and the answer is kept
  per resolved path: a healthy one for `cache_seconds`, an unhealthy one for
  `retry_seconds`. Five dispatches in one tick probe once.
- **A harness that does not answer, or exits non-zero, blocks the task before
  a run exists.** No run row, no session, nothing failed: the task goes to
  `blocked` with a reason naming the binary and the repair, e.g. ``codex does
  not start: `/opt/homebrew/bin/codex --version` did not answer in 10s. A
  person can repair it with `scripts/repair-harness codex` ``, and a
  `harness_unhealthy` journal entry. Other tasks for it are held the same way,
  not failed one by one. It is one `harness_unhealthy` exception per binary in
  the Line "needs a human" list, and a row under `HARNESSES` in `factory
  infra` (and on the L1 Infrastructure page), with every task it holds.
- **Held tasks are released on their own.** At most every `retry_seconds` the
  daemon looks at every task `blocked` with no run whose newest journal entry
  is a hold from the last seven days, probes its harness once, and releases
  the ones that answer: dispatched with the trigger they were held with, or --
  for a scheduled task held past its slot -- set back to `pending` for the
  scheduler to fire, so it never starts twice. The journal is the
  whole record of a hold, so a restart loses nothing. A person's own `task
  run` never trusts a cached failure -- it probes again, since that is what
  they do right after repairing it.
- **Repair is a script a person runs.** `scripts/repair-harness <name>`
  resolves the binary to its Homebrew cask folder, verifies the code signature
  and the expected developer team (OpenAI `2DC432GLL2` for codex, Anthropic
  `Q6L2SF6YDW` for Claude Code) and refuses on any mismatch, copies the folder
  beside itself without extended attributes, checks the copy answers
  `--version`, and swaps it in by rename, keeping the old folder as
  `<version>.stuck`. It never deletes anything. It overrides a decision macOS
  is holding, so the daemon runs it only when the owner sets `auto_repair:
  true` with a `repair_script` -- once per unhealthy stretch, bounded, the
  signature check applying all the same. v1 supports cask installs only:
  Claude Code's own installer, npm packages and ad-hoc signed formulas are
  refused with the reason.

```yaml
daemon:
  harness_health:
    enabled: true            # the default
    timeout_seconds: 10      # how long `--version` may take
    cache_seconds: 180       # how long a healthy answer is trusted
    retry_seconds: 60        # how often an unhealthy one is probed again
    repair_script: /Users/me/factory/scripts/repair-harness   # named in the reason
    auto_repair: false       # opt-in: run it without a person
```

The probe runs with the daemon's own environment. A daemon started by launchd
sees launchd's `PATH`, not a login shell's, so a harness a pane can find but
the daemon cannot is reported as not found -- give the service the same `PATH`
the panes have.

### Continuing after an infrastructure failure (#178)

`factory task run --continue <id>` (also `task.run`'s `continue` field on the
socket and HTTP interfaces) resumes a task's newest run instead of starting a
fresh one. It is refused outright unless that run is terminal and ended on an
infrastructure failure -- `FailKind::AckTimeout`, `RunTimeout`, `SessionGone`,
or `DispatchFailed` after a session had already come up -- never on an
ordinary retry, a workflow rework, or a feedback round, which all keep
today's fresh-session behaviour unchanged. Everything below that gate is a
fallback, never an error: a `--continue` that cannot actually resume still
dispatches, exactly as a plain `task run` would, with a `continue_fallback`
journal entry naming the one reason it fell back.

- **Resume capability is an adapter declaration.** `Agent::resume_spec(&self,
  session_id: &str) -> Option<ResumeSpec>` sits beside `health_probe` on the
  Agent seam: declared, never run, `None` by default and for every
  out-of-process plugin (the plugin protocol does not change). `claude-code`
  answers `--resume <id>`, `codex` answers the subcommand `resume <id>`;
  `pi`, `opencode` and `shell` declare none, which sends `--continue` straight
  to the fresh-session fallback for them. `ResumeSpec::args` is prepended to
  the launch, ahead of anything `launch_spec` or a scope's own declared args
  add, so a subcommand like codex's stays first.
- **Which session.** The previous run's newest usage snapshot entry whose
  `adapter` matches this run's, or -- failing that -- the session id a
  `turn-ended` hook payload named (Claude Code's `Stop`/`StopFailure` always
  carries one), kept on the run. Never "the latest session in this
  directory."
- **Which workspace.** A task with a worktree of its own reuses the previous
  run's exact directory and branch, once `resolve_continue` confirms it is
  still one of the scope's registered worktrees (`git worktree list
  --porcelain`) -- no fresh `git worktree add`, and no bench reset, which
  would wipe the very work being resumed.
- **Falls back to fresh, journaled with the reason, when:** the task's agent
  or adapter changed since the previous run; no session id was recorded; the
  adapter declares no resume; the worktree is gone; or the runtime does not
  confirm the previous session is gone (`AgentRuntime::status`, asked at
  `--continue` time) -- Factory never runs two processes on one conversation,
  so an unconfirmed answer refuses rather than risks it.
- **New run, new token, and a short prompt.** The resumed turn's prompt is a
  brief continue note plus the new run's reporting contract, not the task
  replayed in full -- the resumed conversation already has the original
  instructions. The contract itself gains one sentence, only on a run that
  actually resumed: any report command earlier in the conversation's history
  belongs to the run that just ended and is void. A report with that stale
  token is refused with "a newer run of task `<id>` exists; use the latest
  reporting commands" rather than the generic wrong-token message, in both
  `Engine::caller_for` and `Engine::check_run_token` -- the token itself is
  never kept, in the clear or otherwise: `Run::spent_token_sha256` and
  `Run::superseded_token_sha256s` hold only `run::token_digest`'s SHA-256, so
  the daemon can recognise that a rejected token *used to be* this task's
  without anything that could authorize a request ever being persisted.
- **Usage.** A resumed session's baseline falls back to the previous run's
  own `RunEnd` reading when this run's `Dispatch` snapshot does not yet show
  that session -- see `usage::run_usage_with_prior`.
- **Manual gate.** The owner runs one real resume each with `codex` and
  `claude` on a throwaway instance before trusting this in production; the
  automated tests exercise every fallback with stub adapters and runtimes,
  never a real harness login.

Left out of this first slice (tracked on
[#178](https://github.com/ingo-eichhorst/factory/issues/178)): resume as the
default for workflow feedback rounds, a "what changed since" summary in the
resumed prompt, `pi`'s own session-file resume, and any worktree lifetime or
cleanup policy.

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
  harness_health:              # check a harness starts before a dispatch (#131)
    enabled: true
    timeout_seconds: 10

infrastructure:              # optional: the AI accounts behind the agents, and backups
  providers:
    - name: claude-max
      vendor: anthropic
      kind: subscription       # subscription | api-key
      plan: Max 20x            # free text, optional
      harnesses: [claude-code] # default binding for every agent on these
  backup:                    # see "Backup" below
    destination: /Volumes/Backup/factory
    schedule: { cron: "0 3 * * *", timezone: Europe/Berlin }

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

### Dashboard configuration (#159)

The dashboard's layout is a `dashboard:` block, inherited down the scope
tree exactly the way `roles:` is -- the root's own top-level `dashboard:`,
then each nested scope's `scope.dashboard`, from the top of the tree down to
the scope being asked about:

```yaml
# root .factory/config.yaml, top level (like `roles:`)
dashboard:
  tiles:
    - { metric: throughput_week, size: s }
    - { metric: compliance.cra, size: s }
    - { view: throughput, size: l }

# a nested scope: projects/demo/.factory/config.yaml
scope:
  id: 8fc86b67-aeba-4935-a623-d75f59d77acd
  name: demo
  dashboard:                    # replaces the inherited layout whole, here and below
    tiles:
      - { view: kpis, size: s }
      - { metric: unit_cost, size: m }
```

A tile is exactly one of `metric` (any registry `MetricId` -- see "Goals"
above for the registry, including a bound family like `compliance.cra`) or
`view` (one of a fixed catalogue: `kpis`, `throughput`, `on_the_line`,
`production_year`, `by_scope`, `agent_hours_by_scope`,
`agent_hours_by_agent`, `occupancy_strip`, `inbox`, `compliance`, `cost`),
plus a `size` of `s`, `m`, `l` or `xl`. `kpis` is today's KPI row. No tile
takes a query, a filter or a `group_by` -- design §8's rule against a query
language holds here exactly as it does for metrics and policy checks; a
metric family's own bound segment (`compliance.cra`) is the only
parameter a tile ever carries.

**Resolution**, in one place (`Config::dashboard_for_scope`, the same chain
`roles_for_scope` walks by `Scope.path`, never by name or up the tree): the
nearest `dashboard:` block wins **whole**, never merged tile-by-tile with
what an ancestor declared -- a layout is always one list a person can read
top to bottom. With no `dashboard:` block anywhere in the chain, the
dashboard renders its built-in default (today's page, unchanged; the list
itself lives in `ui/js/dashboard-model.js`, not in this config).
**Removing the block reveals whatever it was overriding** -- the config file
already is the reset, so `Request::DashboardReset` (below) does nothing more
than remove it.

**Validation, at load**, in the same style an unknown role is refused: a
`dashboard:` block with no tiles is refused outright
(`` dashboard needs at least one tile; remove the `dashboard:` block to inherit ``)
-- a layout
that draws nothing is not a smaller valid layout, the same rule an empty
workflow or an empty quality attribute list already follows, and the fix
is named in the message. An unknown metric id (or a family named unbound,
like bare `compliance` with no framework) is a config error naming the
block and the tile (`invalid request: scope "demo"
dashboard.tiles[2].metric "compliance" is not a known metric`); a tile
naming both `metric` and `view`, or neither, the same way. A metric the
registry knows but cannot compute yet is **not** an error -- the tile is
valid and shows its own reason, exactly as an unavailable metric already
does elsewhere. `view` and `size` are closed enums, so an unrecognised
value is refused by the parser itself, the file and the bad value both
named, the same as an unrecognised `lifetime:` or `sandbox:` elsewhere in
this file. The instance root writes its dashboard only in its top-level
`dashboard:`; a `scope.dashboard` block in the root's own config is
refused at startup, and a `dashboard:` block written beside a nested
scope's `scope:` block (where that file never reads it) is refused with
the file named, the same two mistakes `roles:` already guards against.

**Reading it**: `Request::Dashboard { scope }` (wire op `"dashboard"`) answers
`{ tiles: [...] | null, source: "<scope name>" | null }` -- `tiles: null`
means "use the built-in default", paired always with `source: null`;
otherwise `source` names the scope whose block answered (the root's own
configured scope name, for its top-level block, or the instance name when
the root never opted itself into being a scope at all). Never a magic
string like `"root"` or `"default"`: a scope can be named either of those,
so `source` is always either `null` or a real name a caller could look up.
`scope` left out resolves the instance root's own; an unknown scope name
is refused (404 over HTTP), unlike `roles_for`'s tolerant fallback to the
built-in roles for a scope that has since gone -- a dashboard request
names a place to show, and a place that resolves to nothing has none to
show. `GET /api/dashboard?scope=demo` or `factory dashboard --scope demo`
read the same thing.

**Writing it** (`#160`, phase 5): `Request::DashboardSet { scope, tiles }`
saves `scope`'s own layout whole, and `Request::DashboardReset { scope }`
removes it, revealing whatever it was overriding -- both answer with the
same `Payload::Dashboard { tiles, source }` a follow-up read would. Unlike
the read, `scope` is required on both: writing means naming which scope's
own block this is, the same reason `Request::RoleDefine`/`AgentConfigure`
require it rather than falling back to the caller's own. The instance
root's own configured scope writes the top-level `dashboard:`; any other
scope writes `scope.dashboard` in its own config -- the same split, and the
same one-write-path-under-a-lock discipline, `role.rs`'s layer writes
already follow. Validated the same way loading does (`Config::validate`,
whole, before a byte is written), so an empty `tiles`, an unknown metric or
a bad size is refused with the block and the tile named and nothing
written; `Request::DashboardReset` is refused with a clear message when
`scope` writes no block of its own to remove. Both need `dashboard.edit` in
`scope` -- an ordinary grant (`foreman` gets it through `Grant::ALL`,
`worker` and `triager` do not), checked with the target scope as the
subject and reach the same way `agent.configure` is: `own` reach may not
edit a dashboard at all, `scope` reach stops at the caller's own scope.
`Event::DashboardChanged { scope }` is published on either, beside
`Event::RolesChanged`. HTTP: `PUT /api/dashboard?scope=` (body `{tiles}`)
and `DELETE /api/dashboard?scope=`, on the same route the read answers.
The web UI's Dashboard > Customise (`ui/js/dashboard.js`, catalogue and
list edits in `ui/js/dashboard-editor-model.js`) is the one place either is
called from today; no CLI, the same choice role writes already made.

The dashboard page itself (`ui/js/dashboard.js`) fetches this alongside
`/api/production` on load and on a scope change, never on a window change
-- a layout does not depend on how far back the history cards look -- and
falls back to its own built-in default on a failed fetch, so a slow or
unreachable daemon never draws a blank page. #162 (phase 3) gave every
registry `metric` id and the rest of #150's view catalogue --
`agent_hours_by_scope`/`agent_hours_by_agent`, `occupancy_strip`, `inbox`,
`compliance`, `cost` -- their own renderer, so a placeholder now only draws
for a `view` id outside the vocabulary, which the parser's own closed enum
should already have refused before a layout reaches this page at all. Those
tiles read four more shared answers -- `/api/metrics`, `/api/occupancy`,
`/api/operations`, `/api/policy`, `/api/costs` -- each fetched at most once
per render cycle and only when the resolved layout actually names a tile
that needs it, so the built-in default (which names none of them) starts no
new request. `/api/metrics` (`ids=` collected from every `metric` tile in one
request, never one per tile) and `/api/costs` (a fixed `group_by=scope`,
never an arbitrary parameter) follow the dashboard's own scope and window,
the same as `/api/production`. `/api/occupancy` carries no `scope` on the
wire at all (`Request::Occupancy`); its tiles follow the window (`minutes=`)
and narrow the answer to the selected subtree client-side with `inScope`,
the same as the occupancy chart itself -- and since the endpoint clamps a
window past thirty days, an agent-hours tile's own qualifier says the span
it actually got, not the one asked for. `/api/policy` follows the scope only,
no window (a compliance rollup is a live snapshot, not a trailing sum). The
`inbox` tile reuses the Inbox nav view's own unscoped fetch and model rather
than a second copy, narrowing what it shows to the selected scope
client-side the same way, so a scope change never needs a second request.

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

The provider cards also show the latest 5-hour and weekly rate-limit windows,
when each resets, freshness and attribution quality, a snapshot-derived
trend, and the active agents and run instances bound to the account. The
readings of every run bound to the account form one timeline, so the trend
spans a handoff from one run to the next. A reading is dated by its sample
time: one older than fifteen minutes, or one whose runtime did not say when
it sampled, is marked stale. Its `direct` or `apportioned` badge is the
attribution of the interval that reading closed, fixed when it was sampled
rather than read off who is running now. Missing, stale, or unattributable
readings say so rather than rendering as zero. The same
snapshots supply per-run plan share and `factory cost --by provider`; Factory
records token and API-equivalent cost measurements only when the configured
agent runtime supplies them.

The host half of the page is read live on every request, from `sysctl` and
`libc` rather than a subprocess per field: model, chip, cores, memory, OS,
uptime, load and the root filesystem. Any of those that cannot be read is
`null`, never a failed request, and only macOS answers all of them.

### Backup

`.factory/` holds the only copy of the company's operating history -- the
database, the knowledge vault, the policies, goals, scenarios, quality
profiles and VEX judgments -- and git tracks none of it. The root config
names where a copy goes, and the daemon takes one on a schedule:

```yaml
# root .factory/config.yaml
infrastructure:
  backup:
    destination: /Volumes/Backup/factory     # an external disk, NAS mount or synced folder
    schedule: { cron: "0 3 * * *", timezone: Europe/Berlin }
    keep: { daily: 7, weekly: 4, monthly: 6 } # grandfather-father-son
    include_logs: false                       # also .factory/guides/ and .factory/logs/
    encrypt_to: age1...                       # optional (#152): encrypt every snapshot to this recipient
```

A snapshot is one `factory-backup-<instance>-<utc>.tar.zst` holding:

- `.factory/factory.sqlite`, copied with `VACUUM INTO` on a connection of its
  own -- one read transaction, so the copy is consistent while the daemon
  writes, and never a file copy, which WAL would make torn -- then checked with
  `PRAGMA integrity_check` before anything is archived;
- the root `.factory/config.yaml` and every registered scope's own;
- `.factory/{knowledge,datasets,policies,goals,scenarios,quality,vex}/`, whole;
- a `manifest.json`, written last: instance, daemon version (there is no
  build commit compiled in, so none is claimed), the database's
  `user_version`, tables and integrity result, and the path, size and sha256
  of every other file.

Never in it: `.factory/secrets.yaml` or anything under a `secrets/`
directory (not even opened), `.env` files, symbolic links (not followed --
each is named in the manifest instead), the `-wal`/`-shm` files, the socket,
`.factory/worktrees/`, and scope source code, which is backed up by pushing
it to its git remote. Each file is read once and hashed and archived from the
same bytes, so the manifest cannot disagree with the archive. The archive is
written as a hidden `.partial` and renamed into place only when complete and
synced.

**Encryption (`#152`).** `infrastructure.backup.encrypt_to` names a single
native X25519 recipient (`age1…`, from `age-keygen` or an equivalent); an SSH
or plugin recipient (`age1yubikey1…`, `ssh-ed25519 …`) is refused when the
daemon starts, not silently downgraded to plaintext. With it set, the
tar/zstd stream is written straight through an
[`age`](https://age-encryption.org) stream encryptor into the same hidden
`.partial` file, so no plaintext byte ever reaches the destination; the
archive is named `factory-backup-<instance>-<utc>.tar.zst.age` instead of
`...tar.zst`, and a destination holding both is one retention history.
Whether a given archive is encrypted is read from its own bytes (the age
format's magic header), never from its name or the live config, so a renamed
file or a config changed after the fact is never misreported. The daemon
never stores the recipient's matching identity -- only an owner, supplying
one with `--identity <file>` on `verify` or `restore`, can decrypt a
snapshot; a config asking for encryption gets it, but nothing here can prove
that encrypted snapshot restores without a person doing so by hand.

    factory backup [status]          the hero: age against the schedule, destination, last verify, warnings
    factory backup list              every snapshot, verified or not, and the rule that keeps it
    factory backup run               take one now, then apply retention
    factory backup verify [<name>] [--identity <file>]
                                      prove one would restore; an encrypted snapshot needs --identity; exits
                                      non-zero on a failed check
    factory backup restore <name> --into <new-root> [--identity <file>]
                                      verify, then restore into a new root

`GET /api/backup`, `POST /api/backup/run` and
`POST /api/backup/verify?snapshot=` are the same three over HTTP, and the
**L1 › Backup** page draws them. `run` and `verify` need `backup.run`, which
only an agent in the root scope may hold, like `policy.attest`; reading is
open to every agent. The HTTP verify endpoint never accepts an identity --
decrypting an encrypted snapshot is CLI-only and owner-only, the same as
restore, whatever grants a role holds.

**Restore** is CLI-only and owner-only; there is deliberately no HTTP or UI
endpoint and no role grant for it. It runs the same archive-path, manifest,
checksum, database-schema and authored-content checks as `verify`, staging the
exact checked files in a temporary sibling of `<new-root>`. Only after every
required check passes does one rename make the root visible. `<new-root>` must
not exist or must be empty, and may never be the running instance. A corrupt,
incomplete, unsafe or database-incompatible archive leaves neither a partial
root nor a staging directory. The command prints the exact
`factory-daemon --root <new-root> run` and `factory --root <new-root> status`
commands for a switch-over, but never stops a daemon, switches roots or starts
the restored instance itself. Both plaintext and encrypted archives are
supported; an encrypted one needs `--identity <file>` holding the one native
age identity that matches its recipient, or restore is refused before
anything is staged.

**Verify** unpacks a snapshot into a temporary directory -- never over the
instance -- refusing any entry that would land outside it, then checks every
sha256 against the manifest (a file missing, changed or unlisted fails it),
runs `integrity_check` on the database copy and compares its schema version,
loads the root config, and loads every authored-content directory with the
loader the daemon uses: policies and drafts, goals, scenarios, quality
profiles, CycloneDX VEX judgments, datasets and the knowledge index. A file
one of those loaders cannot parse is a warning, not a failure: the checksums
have already proved it is byte for byte what was backed up, so it is broken
in the live instance too. Every verification is recorded; the page's "last
verified" only counts snapshots still in the destination. An encrypted
snapshot (`#152`) decrypts first, given `--identity <file>`; a `decrypt`
check names the identity's public recipient. Without an identity, verifying
an encrypted snapshot is refused outright -- `snapshot X is encrypted; run
factory backup verify X --identity <file>` -- and nothing is recorded: that
is a fact about the request, not a verdict on the archive. A wrong identity
still runs and is recorded, failing only the `decrypt` check.

**The job.** Once a minute the daemon looks whether the schedule's next slot
after the later of the last attempt and the newest archive has passed, so a
daemon that was down at 03:00 takes that night's backup as soon as it is
back, once. **Retention** runs only after a backup succeeded, only on file
names that are this instance's archives -- nothing else in a shared folder is
ever listed or deleted -- and never deletes the newest. Each rule keeps the
newest snapshot of each of its most recent days, ISO weeks or months (in the
schedule's timezone) that have one.

**Verification drills (`#156`).** `infrastructure.backup.verify_schedule`
(`{ cron, timezone }`, the same shape as `schedule`) is a second, independent
schedule for `factory backup verify` itself, so a restore is proven on a
cadence, not only when somebody remembers to run one:

```yaml
infrastructure:
  backup:
    destination: /Volumes/Backup/factory
    schedule: { cron: "0 3 * * *", timezone: Europe/Berlin }
    verify_schedule: { cron: "0 4 * * 0", timezone: Europe/Berlin } # weekly drill
```

The same job loop runs it, right after the backup check: a drill is due at
the verification schedule's next slot after the newest verification recorded
of *any* trigger -- a manual `factory backup verify` satisfies it too -- or
after the daemon booted if nothing has ever been verified, so a daemon down
across several slots drills exactly once when it returns, the same catch-up
the backup itself gets. When a backup and a drill are due on the same tick,
the backup runs first and the drill waits for a later tick, so it always
verifies the fresh snapshot rather than racing it. The drill calls the same
`backup_verify` a person would, unnamed (the newest snapshot) and with no
identity, so it takes the same `backup_busy` exclusion and records the same
`backup_verified` event and row, `by: "schedule"` -- pass or fail feeds
`last_verified` and the policy facts and metrics below exactly as a manual
verify does. Busy or no snapshot yet record nothing, so the slot stays due
and the next tick tries again; a corrupted archive is recorded `ok: false`,
same as a manual verify's own failure. An encrypted newest snapshot
(`#152`) is never drilled -- the job never holds an identity -- so it is a
skip, not a failure: nothing is recorded, and the reason (`newest snapshot
is encrypted; verify it with --identity`) is logged once per due slot,
never once a minute for as long as it stays that way. `factory backup
status` prints the drill's own schedule and its next slot as `next drill`,
and the skip reason when one applies; the L1 › Backup page's hero shows the
same next to "Next backup". `verify_schedule` is optional and
`BackupConfig` stays `deny_unknown_fields`, so an existing config without it
is unchanged and an older daemon refuses the new key outright rather than
silently ignoring it. Out of scope: taking a backup as part of a drill,
auto-restore, and a backup-specific scenario path -- see "Signposts" below
for how a scenario reads backup age instead.

**Warnings are facts, not guesses:** no backup configured; the last attempt
failed, with its reason; the destination missing (only its last component is
ever created -- a missing parent is most likely an unmounted disk, and
creating `/Volumes/Backup/factory` on the system disk would be the backup that
looks fine and is not); the destination on the **same device** as the
instance (same `st_dev`), which is a copy, not a backup; the newest backup
stale (one slot missed, plus two hours' grace) or overdue (two); no schedule;
no snapshot in the destination verified, or the last verification failed; a
scope's repository ahead of its upstream, with no remote, or with no upstream
at all; and Time Machine not configured. A destination inside the instance's
own `.factory/` is refused. The code and Time Machine warnings hold whether
or not a backup is even configured -- source code is backed up by pushing
it, not by this snapshot -- so they still follow the "no backup configured"
warning rather than being skipped by it.

**Code and Time Machine (`#155`).** Scope source code is backed up by
pushing it to its git remote, never by the snapshot above, so the page says
what `git` itself reports for each registered scope's repository: its
current branch's remote and how many commits are not on it, **as of the
last fetch** -- this never fetches, pushes or configures anything. Two
scopes whose directories are the same repository (a nested scope sharing
the root's checkout) are probed once and share one row's worth of fact.
Every state is explicit rather than guessed: `no_directory`,
`not_a_repository`, `no_commits`, `detached_head`, `no_remote`, `no_upstream`,
`tracked` (with the remote, the upstream and the exact count ahead) or
`inspection_failed` when a `git` probe itself failed or timed out. A remote
URL is only ever shown redacted -- `user:password@`/`token@` userinfo is
stripped from an `http(s)` URL before it reaches the wire; a
scp-like `git@host:owner/repo.git` or an `ssh://` URL carries no such
userinfo and is shown as is. On macOS, `tmutil destinationinfo` is read the
same read-only way and reported `configured` (naming each destination),
`not_configured`, or `unavailable` when `tmutil` could not be asked; every
other platform reads `unsupported`. Unknown, failed or unsupported states are
shown as unknown -- never claimed as a problem or as fine.
`factory backup status` prints a `CODE` block and a `TIME MACHINE` line
alongside the snapshot status; the L1 › Backup page draws the same as a
"Code" table and a Time Machine fact, regardless of whether a backup is
configured at all.

Every backup and verification is an event -- `backup_completed`,
`backup_failed`, `backup_verified` -- and a row in an append-only
`backup_events` table. A failure is never a crash: a full disk or an
unmounted volume is a `backup_failed` with the reason, a warning on the page
and a line in the log.

**Policy facts and metrics (`#154`).** One `BackupFact` -- `configured`, the
newest snapshot's timestamp, and `recent`/`offsite`/`verified`, each an
`Option<bool>` -- is derived from the same captured state as the page above,
off `Engine::backup_fact(now)`, but never the repository or Time Machine
probes: a policy report or a metric call never spawns `git` or `tmutil`. No
backup configured reads `false` on all three; a destination missing or
unmounted reads indeterminate on all three, since even "no" would be a
guess about an archive nobody can currently list. The "Policies" section
above documents the three `daemon` facts this backs
(`backup_recent`/`backup_offsite`/`backup_verified`) and the "Goals"
section the two registry metrics (`backup_age_hours`/
`backup_verified_age_days`) it also backs. `encrypt_to` (`#152`) needs no
special case here: an encrypted newest snapshot counts toward
`backup_verified` only once an owner has actually verified it with its
identity, exactly as a plaintext one does.

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
for only the task's own lines, none of its runs'), `GET /api/runs/{id}/output`,
`GET /api/runs/{id}/usage`, `GET /api/tasks/{id}/usage`,
`GET /api/costs?group_by=&from=&to=&scope=`, `GET /api/agents`,
`GET /api/agent-runtime`, `GET /api/environment`, `GET /api/infrastructure`,
`GET /api/backup`, `POST /api/backup/run`, `POST /api/backup/verify?snapshot=`,
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
L5 Quality attributes tab under `GET /api/quality?scope=` and a scenario's
remediation task under `POST /api/quality/remediate` (see "Quality
attributes" above),
L4 Line tab under `GET /api/operations?scope=&window=`, skipping a
schedule's next slot under `POST /api/tasks/{id}/skip-next` and answering
a blocked run under `POST /api/runs/{id}/answer` (see "Line" above),
the L1 Operations tab under `GET /api/environments?scope=`, a deployment's
start and end under `POST /api/deployments` and
`POST /api/deployments/{id}/finish`, and `POST /api/releases` (see
"Operations" above),
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

**Dashboard** is a resolved list of tiles (`GET /api/dashboard?scope=`, `#159`;
the built-in default is five KPI tiles, a by-scope table and an inbox). The
five default tiles still read the same `state.tasks`/`state.scopes` every
other view already holds, nothing fetched specially for them; a registry
`metric` tile or one of #150's newer view tiles (agent hours, occupancy
strip, compliance, cost -- `#162`) reads `/api/metrics`, `/api/occupancy`,
`/api/policy` or `/api/costs` instead, one shared request per endpoint per
render cycle, fetched only when the resolved layout actually names a tile
that needs it. The inbox lives inside the dashboard rather
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

The L0 kernel now owns every live fact schema and its nested vocabulary
(#193 phase 2), including conformance evidence. Existing core paths re-export
these types. Providers, wildcard authorization, intake deduplication and
conformance evaluation remain outside L0. The kernel has no Factory crate
dependency. The first phase-3 batch adds typed pull ports: producer-owned
providers assemble policy evidence, and metrics read backup, conformance
and environment figures through the same channel. Line reads its capacity
configuration through a port. Queries preserve selective names and windows;
responses can contain only the selected fact or a collection of it. No
status table or fact log is introduced. The remaining signpost reader move,
Below bounds, crate splitting and strict command ladder are still ahead.

## Layout

    crates/factory-kernel    L0: pure shared vocabulary and every live fact schema (Level/Fact, nested statuses, grants and evidence); no other factory-* dependency
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
    ui/js/{sandboxes,secrets,dependencies}.js                    L2's three tabs
    ui/js/dependencies-model.js                                  Dependencies' pure shaping logic
    ui/js/{benchmarks,knowledge}.js                              L5's two tabs
    ui/js/knowledge-graph.js                                     the knowledge graph's pure layout, filter and tail logic
    ui/js/{backup,backup-model}.js                               the L1 Backup tab and its pure shaping logic
    ui/js/{environments,environments-model}.js                   the L1 Operations tab (#185) and its pure shaping logic
    ui/js/{doctor,doctor-model}.js                               the L1 Doctor dependency view and its pure shaping logic
    ui/vendor/three.min.js     vendored so the site's lit render works offline
    examples/plugins         a worked example of an out-of-process adapter
