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
target/debug/factory-daemon --root /tmp/demo-factory init
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
scopes:
  - name: demo
    path: projects/demo
    agents:
      - name: watcher
        harness: pi
        lifetime: permanent     # started with the daemon, restarted if it dies
      - name: scratch
        harness: shell
        lifetime: temporary     # startable, but nothing starts it on its own
      - name: reviewer
        harness: claude-code
        lifetime: task          # not standing: offered for tasks in this scope
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

Every agent has a **role**, and a task names a **concrete agent**, not a
harness. `assistant` and `scratch` are different agents even when both are pi.

```yaml
scopes:
  - name: demo
    path: projects/demo
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

```sh
factory agents                       # scopes, their agents, and what each is doing
factory agent start demo watcher
factory agent stop demo/watcher      # stays stopped until someone asks again
factory agent output demo/watcher
factory agent input demo/watcher --text "how is it going?" --key enter
```

Each standing agent also carries the command to get into its terminal yourself —
`herdr --session factory agent attach factory-demo-watcher`. A shell session has
no named agent to attach to, so Factory says so instead of printing a command
that would fail.

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

## How a task actually runs

1. `task.create` resolves the scope, agent, and runtime — from the request, then
   the scope's declaration, then the instance defaults — and refuses right away
   if any of them names an adapter that does not exist.
2. `task.run` opens a run, mints a callback token for it, and asks the runtime
   for a session in the scope's directory.
3. The agent adapter produces the prompt. It carries the task, the working
   directory, and the reporting contract — the exact commands the agent is to
   run. The same values are in the session's environment as `FACTORY_TASK_ID`,
   `FACTORY_TASK_TOKEN`, `FACTORY_SOCKET`, and `FACTORY_BIN`.
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

`.factory/` at the instance root is the whole configuration surface. Nothing of
Factory's is ever written inside a scope.

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

scopes:
  - name: demo
    path: projects/demo
    agent: pi                # this scope's default, overriding the instance's

  - name: upstream
    path: projects/upstream
    task_store: file-store   # and this scope's tasks live somewhere else
```

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

The engine a scope uses is shown on the agents page, and is not editable there
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
the `factory` binary, the callback token, and `reporting_contract` — the exact
wording the built-in agents use. Paste it rather than rewriting it.

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
and `POST /api/rpc` for the raw envelope. `GET /ws` is the event stream: a
snapshot of every task first, then one message per event.

Adding an interface — mcp, or anything else — means translating to that
envelope, not inventing a second API.

## The web UI

Two pages. **Tasks** is the list; clicking one opens it in a modal with its
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

**Agents** has two views of the same thing. **Occupancy** is a chart: one row
per agent, grouped by scope, drawn against a shared clock. Three layers, kept
deliberately apart because they are different kinds of evidence:

- a **solid block** is a run — Factory started it and the agent reported back;
- a **dashed block** past the now line is a schedule's next firing, drawn as
  wide as the median of that task's own finished runs, or as a marker when it
  has none to measure;
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
- **A scope's task engine is configuration, not a control.** The agents page
  says which engine a scope's tasks live in; changing it means editing
  `.factory/config.yaml` and restarting. A selector that rewrote the instance's
  configuration over an interface with no authentication is a different
  decision, and it has not been made.
- **First-run agent prompts.** An agent that has never seen a directory may ask
  a human to trust it before it will read the task. Factory cannot answer that
  for you. For a harness whose runtime reports through a lifecycle hook (`pi`,
  today), this now surfaces as a hook-reported `Blocked` run rather than
  silence, and is governed by `blocked_timeout_seconds` instead of the run
  timeout. For every other harness it is still exactly what it always was: the
  task times out and tells you where to look.
- **One workspace per task, closed on completion.** A task that never reaches a
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
    examples/plugins         a worked example of an out-of-process adapter
