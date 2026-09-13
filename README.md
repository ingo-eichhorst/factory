# Factory

A daemon that hands tasks to coding agents and watches what happens.

A task is created — by a person, by a schedule, or by another agent. The daemon
opens a session in an agent runtime, starts an agent in it, and gives it the
task along with instructions for how to report back. The agent works, reports
progress, and closes the task. Everything is visible live over a CLI, an HTTP
API, and a web UI.

This is a prototype. It runs, and the parts that do not work yet say so.

## The four seams

Everything Factory can be pointed at something else is one of four traits, and
the daemon cannot tell a built-in implementation from a plugin:

| Adapter | What it decides | Ships with |
| --- | --- | --- |
| **Agent** | how a harness is started, and what a task sounds like to it | `claude-code`, `pi`, `codex`, `opencode`, `shell` |
| **Agent runtime** | where agents actually run | `herdr` |
| **Task store** | where tasks live — the CRUD contract, chosen per scope | `sqlite` |
| **Interface** | how the outside reaches the daemon | `cli` (unix socket), `http` (REST + WebSocket + UI) |

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

# Recurring work.
factory task create "morning sweep" -i "..." --schedule "0 9 * * 1-5"
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

The instance root's `knowledge/wiki/` is a wiki people and agents write by
hand — `SCHEMA.md` sets the rules, pages carry YAML frontmatter and
`[[links]]`. The L5 **Knowledge** tab reads it. Nothing else does yet, and
nothing writes to it.

`factory knowledge` and `GET /api/knowledge` rebuild the index from the files
on every call: no link table to keep in step, and nothing is ever written. A
`.md` file whose frontmatter parses and names a `title` is a note; every
other one is a page, listed but not a graph node. A `[[link]]` resolves
page-relative, then root-relative, then by a unique file name; more than one
match is reported rather than guessed at, and an unresolved or ambiguous
target is a gap, not an error.

**v1 indexes; it does not read.** No note's body ever reaches the browser or
the CLI — only titles, frontmatter fields, links, and findings. A `sources[]`
entry under `data/secrets/` is reported as a finding and nothing under it is
ever opened or even stat-ed; that call is decided from the string alone,
before any filesystem access. The knowledge base is company-wide: the scope
rail does not filter this tab.

## Benchmarks

**v1 declares and displays; it runs nothing and records no score.**
`factory bench` and `GET /api/benchmarks` group every agent Factory can
dispatch — every declared agent, plus the foreman `daemon.foreman` would
synthesize — into one configuration per distinct harness, full arguments, and
sandbox, and say which of what a real score would need is recorded today.

Today that is: the harness itself, and a model when a declaration's `args`
spells out `--model`, `--model=`, or `-m`. Harness version, tool surface,
context policy, and retry budget are recorded nowhere yet, so every
configuration comes back `pinned: false`. No argument value but the extracted
model ever reaches the payload — an `args` entry can be a secret, the same
rule the Secrets tab already lives by, so everything else is reduced to its
flag with the value elided.

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

Three timeouts catch the rest:

- `ack_timeout_seconds` (180 by default) — the agent is up but has not said a
  word. This is what an agent sitting on a first-run trust prompt or a login
  looks like.
- `task_timeout_seconds` (3600 by default) — it acknowledged and then went quiet.
- `blocked_timeout_seconds` (86400 by default) — a run a hook reported
  `Blocked` is exempt from the two above and given this much longer clock
  instead, counted from when the block began rather than when the run did, so
  a person has a real chance to see it and answer before the daemon gives up.

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
the `factory` binary, the callback token, `reporting_contract` — the exact
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
`GET /api/runs/{id}/entries`, `GET /api/runs/{id}/output`, `GET /api/agents`,
`GET /api/agent-runtime`, `GET /api/environment`, `GET /api/knowledge`,
`GET /api/benchmarks`, workflow CRUD under `/api/workflows`, workflow-run
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

## What this prototype does not do yet

- **Runtime and interface plugins.** The manifest accepts `kind: runtime` and
  `kind: interface`, and the daemon says plainly that it will not load them.
  The traits are there; the proxies are not.
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

    crates/factory-core      domain, events, wire protocol, the four adapter traits
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
    ui/vendor/three.min.js     vendored so the site's lit render works offline
    examples/plugins         a worked example of an out-of-process adapter
