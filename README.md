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
| **Task store** | where tasks live — the CRUD contract | `sqlite` |
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

## How a task actually runs

1. `task.create` resolves the scope, agent, and runtime — from the request, then
   the scope's declaration, then the instance defaults — and refuses right away
   if any of them names an adapter that does not exist.
2. `task.run` mints a callback token, marks the task `dispatching`, and asks the
   runtime for a session in the scope's directory.
3. The agent adapter produces the prompt. It carries the task, the working
   directory, and the reporting contract — the exact commands the agent is to
   run. The same values are in the session's environment as `FACTORY_TASK_ID`,
   `FACTORY_TASK_TOKEN`, `FACTORY_SOCKET`, and `FACTORY_BIN`.
4. The agent runs `factory task report <id> --status running …`, then finishes
   with `done`, `failed`, or `blocked`.
5. On a terminal report the daemon keeps the last of the terminal output as a
   journal entry and closes the session.

**Status comes from the agent, not from the terminal.** A runtime can say
whether a session is alive; it cannot say whether the work is finished, and
anything that reads that from a terminal's appearance will be wrong sometimes.
Runtime status is used only to notice sessions that died.

Two timeouts catch the rest:

- `ack_timeout_seconds` (180 by default) — the agent is up but has not said a
  word. This is what an agent sitting on a first-run trust prompt or a login
  looks like.
- `task_timeout_seconds` (3600 by default) — it acknowledged and then went quiet.

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
      bind: 127.0.0.1:8787
  tick_seconds: 5
  ack_timeout_seconds: 180
  task_timeout_seconds: 3600
  default_agent: claude-code
  default_runtime: herdr

scopes:
  - name: demo
    path: projects/demo
    agent: pi                # this scope's default, overriding the instance's
```

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
`task.delete`, `task.append_entry`, `task.entries`, and `task.due`, which is
what it takes to back tasks with an issue tracker instead of the local database.

`examples/plugins/shell-plugin/` is a complete, working example in about eighty
lines of Python. Copy it.

A plugin may not take the name of an adapter that already exists; the registry
refuses the collision rather than silently shadowing a built-in.

## Interfaces

Both interfaces speak one request envelope, so they cannot drift apart. The
socket carries it directly, one JSON object per line:

```sh
echo '{"op":"task.list","params":{}}' | nc -U .factory/factory.sock
```

HTTP maps REST onto the same thing — `GET /api/tasks`, `POST /api/tasks`,
`POST /api/tasks/{id}/run`, `GET /api/tasks/{id}/entries`, and `POST /api/rpc`
for the raw envelope. `GET /ws` is the event stream: a snapshot of every task
first, then one message per event.

Adding an interface — mcp, or anything else — means translating to that
envelope, not inventing a second API.

## What this prototype does not do yet

- **Runtime and interface plugins.** The manifest accepts `kind: runtime` and
  `kind: interface`, and the daemon says plainly that it will not load them.
  The traits are there; the proxies are not.
- **Runs are not modelled.** A recurring task is one row that returns to
  `pending` after each firing, carrying its most recent result. Per-firing
  history lives in the journal rather than as its own record.
- **The socket is the security boundary.** It is `0600` in `.factory/`, and the
  callback token only stops one running agent from closing another's task by
  mistake. The HTTP interface has no authentication and binds to loopback.
- **First-run agent prompts.** An agent that has never seen a directory may ask
  a human to trust it before it will read the task. Factory cannot answer that
  for you — it will time the task out and tell you where to look.
- **One workspace per task, closed on completion.** A task that never reaches a
  terminal state leaves its session open on purpose, so it can be looked at.

## Layout

    crates/factory-core      domain, events, wire protocol, the four adapter traits
    crates/factory-plugins   built-in adapters, the plugin host, the registry
    crates/factory-daemon    engine, scheduler, interfaces, the binary
    crates/factory-cli       the `factory` binary
    ui/index.html            the web UI, compiled into the daemon
    examples/plugins         a worked example of an out-of-process adapter
