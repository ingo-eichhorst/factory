# Slice 10 — operator guide

Backlog §10 (`docs/implementation-backlog.md`) is CLI automation and Claude
Code parity: "[r]eplace validated manual runbooks with an idempotent Factory
CLI and add the second adapter without changing domain behavior." This
document is that runbook.

It differs from `docs/slice-1-operator-guide.md`, `docs/slice-8-operator-guide.md`
and `docs/slice-9-operator-guide.md` in one way that matters more than
anything else below. Each of those guides said, in its own words, "there is
still no `factory` binary" and then drove the library crates through a small
test program written and discarded for the guide. **That sentence is retired.**
Every procedure here is a command you type.

Read `docs/slice-8-operator-guide.md`'s "What this slice delivers, and what it
does not" section first if you have not. It is the part that keeps a guide
like this one honest, and this document follows its shape.

## What this slice delivers, and what it does not

### Delivers

- **The `factory` binary** (`crates/factory-cli`), implementing the part of
  design §7 this slice carries: `init`, `start|stop|status|doctor`, `daemon
  run`, `scope`, `agent`, `task`, `context`.
- **A daemon** (`crates/factory-daemon`) — ADR 0014's "Factory runs as a
  long-running daemon; the daemon is the sole owner of core state
  mutations." It owns the database, holds the installation lock ADR 0012
  deferred, serves a line-delimited JSON request/response protocol over a
  Unix socket, and runs the observe loop.
- **A second adapter** (`crates/factory-adapter/src/claude.rs`) — Claude Code,
  driving Herdr for terminal actions and Irrlicht for state, joined on
  `launcher.herdr_pane_id` exactly as ADR 0017 requires.
- **`factory doctor`** (`crates/factory-doctor`) — six read-only checks in one
  pass, the only component outside the daemon that opens the database, and
  the only one that works when the daemon is down.
- **An agent reporting its own result** — `factory task done|fail|block`, run
  by the agent inside its own session. Design §3 lists "capturing the final
  response when the harness exposes it" among adapter duties but never says
  how a finished agent tells Factory so. This is the answer, and it is a push
  over the same client transport as every other command.

### Does not

- **`factory scope add` is refused, not stubbed.** It returns
  `internal.scope_add_unsupported` and names the manual remedy. See
  "Registering a scope by hand" below for why.
- **`secret`, `knowledge`, `memory` and `agent list` do not exist** — station
  13 and station 12 respectively. They are absent, not stubbed: a stub that
  exits non-zero is a command that exists, and an operator reads it as broken
  rather than as unbuilt. (`schedule` was in this list. Station 11 shipped it;
  see the slice-11 guide.)
- **`opencode` is configurable but not driveable.** `factory start --harness
  opencode` is refused at startup with "no adapter", rather than starting a
  daemon that fails on the first session.
- **Factory installs no `launchd` job**, and after station 11 it never will:
  its dispatcher is a thread the daemon owns (ADR 0021 decision 2). `factory
  doctor`'s check 6 still asks `launchctl` about
  `com.business-factory.scheduler`, but that label belongs to the pre-Factory
  Python prototype, `scripts/factory_tasks.py dispatch --deliver`, which writes
  to the same `.factory/factory.sqlite`. On this machine it is **loaded**.
  Nothing here has cut over yet, so that is a fact check 6 reports rather than
  a fault — until Factory's own dispatcher also ticks against this database,
  at which point two dispatchers are writing one file and check 6 says so.
- **A Claude Code session that reaches `disconnected` is returned to service
  by a human, never by its own adapter.** This is deliberate; see "Why Claude
  Code needs a human" below.
- **`factory_recovery::reconnect`'s three restart classes are not wired into
  the daemon.** Startup runs `restore::reconcile` only. Slice 9's procedures
  2 through 5 remain library calls, exactly as that guide documents them.

## The command surface, and what each command actually calls

Every row below was checked name-by-name against the tree. The middle column
is the operation name the CLI sends over the socket
(`crates/factory-daemon/src/handler.rs`); the right column is the domain
function that operation calls. `factory doctor` and `factory init` have no
operation because neither goes through the daemon.

| Command | Operation | Domain function |
|---|---|---|
| `factory init` | — (local) | `factory_config`, `factory_store::Store::open_at` |
| `factory start` | — (spawns `daemon run`) | — |
| `factory daemon run` | — (is the daemon) | `Daemon::start` → `build_handler` → `observe::spawn` → `serve` |
| `factory stop` | — (signal + bounded wait) | — |
| `factory status` | `daemon.status` | `ops::status::daemon_status` |
| `factory doctor` | — (read-only, no daemon) | `factory_doctor::diagnose` |
| `factory scope add` | `scope.add` | refused — `ops::scope::add` |
| `factory scope reconcile` | `scope.reconcile` | `factory_registry::{resolve, reconcile, apply}` |
| `factory scope list` | `scope.list` | `ops::scope::list` |
| `factory agent start` | `agent.start` | `factory_session::begin_start`, `Adapter::start`, `factory_session::mark_running` |
| `factory agent stop` | `agent.stop` | `factory_session::{interrupt, stop}`, `Adapter::{interrupt, stop}` |
| `factory agent status` | `agent.status` | `factory_session::{show, list, leases_of_session}` |
| `factory agent attach` | `agent.attach_command` | `Adapter::attach_command`, then `exec` in the client |
| `factory task send` | `task.send` | `factory_delegation::queue::{queue_from_human, queue_from_session}`, `factory_task::assign::assign`, `factory_task::deliver::deliver` |
| `factory task send --wait` | `task.send` then `task.wait` | `ops::task::wait` |
| `factory task cancel` | `task.cancel` | `factory_task::create::cancel` |
| `factory task done` | `task.done` | `factory_task::complete::done`, `factory_session::on_task_terminal` |
| `factory task fail` | `task.fail` | `factory_task::complete::fail` |
| `factory task block` | `task.block` | `factory_task::complete::blocked` |
| `factory task resume` | `task.resume` | `factory_task::deliver::authorise_resume` |
| `factory task list` | `task.list` | `factory_task::create::list` |
| `factory task show` | `task.show` | `factory_task::create::{show, delegation_chain_of}` |
| `factory context show` | `context.show` | `factory_context::compile` |

`task.wait` is a **query**, not a command, and that is load-bearing: waiting
is a read. Its deadline expiring changes nothing about the task — the CLI
exits `7` and names the task id so you can ask again with `factory task show`.

### Exit codes

`crates/factory-cli/src/exit.rs` is the one home for these.

| Code | Meaning | Your next move |
|---|---|---|
| `0` | Did what it was asked | — |
| `1` | Something else failed | Read the message |
| `3` | The daemon is not running | `factory start --root <root>` |
| `4` | The daemon rejected the request | The message carries the daemon's own error code |
| `5` | `doctor` ran and found something | Read the report |
| `6` | `doctor` could not open the database | Diagnosis never ran; check the root |
| `7` | `task send --wait` passed its deadline | `factory task show --task-id <id>` — the task is untouched |
| `8` | `daemon run` could not bind or lock | Usually another daemon; the message names its pid |
| `9` | `stop` signalled but the daemon still answered | Check the pid; consider `kill -9` |

## Procedure 1 — create an instance

```sh
factory init --root /path/to/instance --name my-instance
```

Idempotent. Running it twice on the same root is safe. It writes
`.factory/config.yaml` and `.factory/factory.sqlite`, and nothing outside a
`.factory/` directory.

Without `--root`, `init` defaults to the **current directory** — the one
command that does, because its root need not exist yet. Every other command
resolves the root from `--root`, then `FACTORY_ROOT`, then by walking up from
the current directory looking for a `.factory/`.

### The socket path limit, which you will meet before you expect to

The daemon binds `<root>/.factory/factory.sock`. A Unix domain socket path is
limited by `SUN_LEN`, and on macOS the measured limit is **103 bytes: 103
binds, 104 fails.** Linux allows more.

An instance root nested a few directories deep inside a temp directory
reaches this without looking long. `factory daemon run` checks the length
**before it touches anything on disk**, so an unbindable path can never first
delete a live daemon's socket, and the error names the path, its length, and
the limit rather than the kernel's own `path must be shorter than SUN_LEN`.

If you hit it, put the instance root somewhere shorter. There is no flag to
move the socket: one home for the path (`factory_daemon::socket_path`) is
worth more than the flexibility.

## Procedure 2 — the daemon lifecycle

```sh
factory start --root <root> --harness pi      # or --harness claude-code
factory status --root <root>
factory stop --root <root>
```

`factory start` spawns `factory daemon run` in the background and appends its
output to `<root>/.factory/daemon.log`. `factory daemon run` in the foreground
is the same daemon and is what the drills use.

Three things happen at startup, in this order:

1. **The installation lock** — an exclusive `flock` on
   `<root>/.factory/factory.lock`, taken *before* the socket is bound. This is
   ADR 0012's deferred lock, real now that something long-lived exists to hold
   it. A second daemon on the same root fails with exit `8` and a message
   naming the first one's pid.
2. **The socket** — `<root>/.factory/factory.sock`, bound only after the lock
   is held. The ordering is what makes a stale socket file safe to remove: the
   lock, not the socket file, is what proves nobody is serving.
3. **`restore::reconcile`** — ADR 0019's restore pass, run once as the handler
   is built, before the first request is served.

Then the observe loop starts, polling every adapter-visible session on a fixed
five-second interval (`OBSERVE_INTERVAL` in
`crates/factory-cli/src/commands/daemon.rs`).

`factory stop` signals the daemon and waits, bounded. It is idempotent —
stopping an already-stopped daemon is `0`, not an error. A daemon on its way
down may accept a connection from the listener backlog and close it without
answering; that is its own condition (`ClientError::ClosedWithoutResponse`),
matched by type, and it does not print an error on a successful stop.

## Procedure 3 — registering a scope by hand

`factory scope add` is refused. The reason is ADR 0009: `config.yaml` may
carry keys this build does not know, and a round-trip that preserves them
needs a YAML *writer*, which nothing in this tree has. Writing it with a
serializer that emits only the keys the current structs define would silently
delete an operator's own comments and any key a newer build added.

So the remedy is manual, and the command says so:

```sh
$EDITOR <root>/.factory/config.yaml       # add the scope
factory scope reconcile --root <root>     # see what would change
factory scope reconcile --root <root> --apply
```

`reconcile` without `--apply` reports drift and changes nothing. With
`--apply` it applies what ADR 0016 says is safe to apply automatically, and
still reports — never applies — the variants ADR 0016 marks "a human must
look."

## Procedure 4 — sessions

```sh
factory agent start  --root <root> --scope worker --agent-name builder
factory agent status --root <root> --scope worker
factory agent attach --root <root> --session <id>
factory agent stop   --root <root> --session <id>
```

**`--scope` takes a name or an id.** Design §7 addresses scopes by name in
every example it gives, so the CLI resolves a name through `scope.list`
(`crates/factory-cli/src/scope_ref.rs`). An argument that parses as a UUID is
treated as an id; anything else is a name. An unknown name lists the scopes
that do exist rather than saying only "not found."

`factory agent attach` is the one command that does not merely print what the
daemon answered. The daemon cannot hand you a terminal from inside a worker
thread, so it returns the argv that would attach, and the **client execs it**.

## Procedure 5 — tasks, and how one is closed

```sh
factory task send --root <root> --scope worker --prompt "..."
factory task send --root <root> --scope worker --prompt "..." --wait --timeout 300
factory task show --root <root> --task-id <id>
```

Delivery follows design §5 unchanged: the attempt is journalled **before** the
terminal is written, and nothing resends automatically after an ambiguous
outcome. Two refinements landed in this slice.

**A refusal is not a delivery.** `Adapter::send` prechecks the session's
harness state and refuses to submit into a session that is already working
(`AdapterError::SessionBusy`). The daemon maps that one error to
`PromptWriteError::refused`, which records `DeliveryOutcome::Refused`, and
recovery's `ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL` predicate excludes it. A
task whose only attempt was refused stays `queued`, because nothing was sent.
Before this, such a task went to `blocked: interrupted` and sent an operator
to `task resume` for a prompt that had never reached a terminal.

**The task id is rendered once, by `deliver`.** Design §5's line belongs to
delivery, not to an adapter, and the decisive case is
`OperatorPromptWriter` — a human pasting a prompt by hand, which never passes
through an adapter at all. An adapter-owned prefix would have dropped the id
from that path entirely; when both adapters *also* prefixed, a real Pi pane
showed it twice.

### Closing a task

The agent working the task closes it, from inside its own session:

```sh
factory task done  --root <root> --task-id <id> --summary "..."
factory task fail  --root <root> --task-id <id> --summary "..."
factory task block --root <root> --task-id <id> --reason clarification
```

The task id is in the prompt the agent received. Nothing about `TaskSignal`
changed and no harness state closes a task — nothing has to, because the agent
says so itself.

When such a task carries a delegation chain, the delegating session is told
through the same journal-then-write path as any other delivery, carrying
nothing but the task id and its outcome. That is **not** a message primitive
and must not become one: the task record remains the only durable unit of work
exchanged (design §2.4, ADR 0010 decision 2). A caller that would rather block
than be told uses `task send --wait`.

### Resuming after a restart

```sh
factory task resume --root <root> --task-id <id>
```

ADR 0019's middle row: a task that was `queued` when a delivery attempt may
have reached the terminal is restored to `blocked: interrupted`, and only a
human authorises one further delivery.

## Procedure 6 — diagnosis

```sh
factory doctor --root <root>
```

Read-only, in one pass, and **it works when the daemon is down** — which is
the condition you run it to diagnose. It opens the database through ADR 0018's
`Store::open_read_only`, so it reports the schema version rather than
migrating to it.

Six checks:

1. **Configuration** — `config.yaml` loads and validates, for every registered
   scope. Missing or unreadable counts.
2. **Database** — `PRAGMA integrity_check`, plus the schema version compared
   against this build's: `SchemaBehindBuild` means an update was installed and
   nothing has opened the database read-write since; `SchemaAheadOfBuild`
   means an older build is looking at a database a newer one migrated.
3. **Registry drift** — the whole `factory_registry::Drift` report, unfiltered.
   ADR 0016 draws the "a human must look" line itself; filtering here would
   silently drop the variants that line names.
4. **Orphaned leases** — a held `workspace_leases` row whose session is no
   longer in a lease-holding state.
5. **The pane audit**, both directions — a live session whose pane is gone
   from Herdr, and a pane still alive for a session the database calls dead.
6. **The scheduler** — two questions, not one. Station 11 replaced this
   check's second half; ADR 0021 decision 10 has the rules and why each one
   is what it is.

   - **Factory's own dispatcher**, read from `dispatcher_state`. No row means
     no dispatcher has ever ticked against this database, which is ordinary on
     a fresh instance and is **not** a finding. A row older than five minutes
     means one ran and stopped, which is. Below schema 6 the table does not
     exist, and the check reports `CheckSkipped` rather than failing the whole
     report.
   - **The foreign scheduler**, from `launchctl list
     com.business-factory.scheduler`. That label belongs to the pre-Factory
     Python prototype, not to Factory: Factory's dispatcher is a thread the
     daemon owns and installs no `launchd` job. **Not loaded is the desired
     end state after the cut-over and is not a finding.** A loaded job beside a
     *fresh* Factory tick is two dispatchers against one database, which is
     the finding.

   On this machine one **is** loaded, and Factory has never ticked here, so
   check 6 reports it as a fact and nothing more. That is the documented state
   of an instance that has not cut over yet.

   `last_fired` is gone. It was always `None`, because neither `launchctl
   list` nor `launchctl print` exposes a last-fired time and `LastExitStatus`
   is an exit code, not a timestamp. `last_tick_at` replaces it and answers a
   different question: when Factory's own dispatcher last ran.

It always reports the backup summary too (count, total size, oldest and
newest under `.factory/backups/`, per ADR 0019 decision 5). Purely
informational: Factory deletes nothing there and no size threshold is itself
a finding.

**It reports and never repairs.** Recovery keeps slice 9's explicit review,
resume and replacement actions, so there are not two ways to change the same
state. A finding means exit `5`, which makes the command usable from a check.
A database it cannot open at all is exit `6` — no report to read, only a
reason diagnosis did not run.

## Why Claude Code needs a human

Both adapters pass the same contract tests for start, send, observe,
interrupt and stop. What differs is what their observations are allowed to
*decide*.

Measured on 2026-09-09, and this is the risk clause of backlog §10 landing
exactly where it was aimed:

- `herdr agent explain` on a live **Pi** pane answers
  `screen_detection_skip_reason: full_lifecycle_hook_authority`. Herdr is
  *told* Pi's state. That is what ADR 0017 requires before an observation may
  be called authoritative.
- The same command on a live **Claude Code** pane answers
  `rule: live_prompt_box (region=prompt_box_body priority=950)`, `evidence:
  "❯\n"`. Herdr reads Claude's screen.

So the Claude adapter takes its state from Irrlicht, as ADR 0011 always said
it would — and Irrlicht supplies the join key (`launcher.herdr_pane_id`,
present for 5 of 5 live `claude-code` sessions) but not authority: every one
reports `confidence: "medium"` and `last_event: "transcript_activity"`, which
is state inferred from a transcript.

The Claude adapter therefore returns `Confidence::Degraded` and never
`Authoritative`. `factory_recovery::evidence::may_promote_from_disconnected`
requires `Authoritative`, so **a Claude Code session that reaches
`disconnected` is returned to service by a human.** Weakening that rule to
admit `Degraded` would have made at-most-once delivery rest on a heuristic for
one of the two harnesses, which is the trade backlog §10's risk clause
forbids.

Neither adapter joins on working directory. Four Claude sessions can share one
directory — measured, not assumed — so the pane is the only join key that
identifies a session.

## Where this is proven

`./check.sh` — clippy at `-D warnings`, `rustfmt`, and **556 passing tests**
as of this slice's second commit.

Two drills in `crates/factory-e2e/tests/` run inside that number and need
nothing live:

- `daemon_process_drill.rs` — ADR 0014's own open item, "a daemon that has
  never been killed and observed to come back is not known to be restartable,"
  against a **real daemon process**. The parent test re-executes its own
  already-compiled test binary as the daemon subprocess, so it needs no new
  dependency and no `[[bin]]` target.
- `observe_loop_drill.rs` — the threaded runner around `reconcile_once`.
  Deterministic on purpose: the interval bounds how long the drill takes,
  never whether it passes, and "stopping is prompt" is proven with a 30-second
  interval and a 2-second join.

The live drills are `#[ignore]`d and need a running Herdr:

```sh
cargo test -p factory-e2e --test live_herdr -- --ignored --nocapture
FACTORY_E2E_ROOT=<root> FACTORY_E2E_PANE=<pane> \
  cargo test -p factory-e2e --test live_agent -- --ignored --nocapture
```

`check.sh` must not depend on a running Herdr. A suite that goes red because
somebody closed a terminal teaches an operator to ignore red.

## Running the whole chain live, once

This is the drill that found four defects the 556 tests could not. Run it
against a throwaway root, never against a real instance.

```sh
export FACTORY_ROOT=/tmp/f10          # keep it short: 103 bytes for the socket
factory init  --root "$FACTORY_ROOT" --name f10
$EDITOR "$FACTORY_ROOT"/.factory/config.yaml      # declare one scope
factory start --root "$FACTORY_ROOT" --harness pi
factory scope reconcile --root "$FACTORY_ROOT" --apply
factory agent start --root "$FACTORY_ROOT" --scope worker --agent-name worker
factory agent status --root "$FACTORY_ROOT" --scope worker
factory task send --root "$FACTORY_ROOT" --scope worker \
  --prompt 'Reply with exactly the token ST10C-831000 and nothing else.'
factory task show --root "$FACTORY_ROOT" --task-id <id>
factory stop --root "$FACTORY_ROOT"
```

Put a **unique token in the prompt, generated per run.** Asserting on a token
that could have come from an earlier run is not a proof. Check the pane
yourself: the task id must appear exactly **once**.

Clean up afterwards. Count Herdr's panes before you start and after you
finish — they must match. `herdr workspace close <id>` closes what `agent
start` created.

Two behaviours you will see that are correct, not defects:

- A `disconnected` session still counts against `max_sessions`. The harness
  process really is still alive; the error says so.
- After a daemon restart, a pre-existing session reconciles to `disconnected`.
  That is ADR 0019's rule, in production rather than in a drill.

## Open items this slice leaves

- `factory scope add` (above), and the YAML writer it needs.
- `factory_recovery::reconnect`'s restart classes are not wired into the
  daemon's observe loop or startup.
- `agent.stop` does not specially handle a task that is `queued` and assigned
  but not yet delivered to the stopping session — only a `running` task
  triggers `interrupt`.
- `InstallationLock` leaks its `RwLock` (`Box::leak`). Sound while
  `Daemon::start` is the only caller — verified: the CLI uses `rpc::ping`, not
  the lock, to test liveness. It must be restructured before anything
  acquires the lock in a loop.
