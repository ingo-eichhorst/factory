# Slice 11 — operator guide

Station 11 gives Factory a scheduler of its own.

Until now a cron rule lived in `scripts/factory_tasks.py`, woken by a
`launchd` job, writing to the same `.factory/factory.sqlite` that Factory
uses. That prototype still runs on this machine. This station builds the
replacement, and the last procedure here is how you swap one for the other.

Read `docs/slice-10-operator-guide.md` first if you have not. Everything it
says about the daemon, the socket, exit codes, and registering a scope by
hand is still true, and is not repeated here.

The decisions behind this station are in
`docs/adr/0021-task-templates-runs-and-the-cron-dispatcher.md`. Where this
guide says "because", that document says why at length.

## What this station delivers, and what it does not

### Delivers

- **A cron dispatcher inside the daemon.** A thread, not a `launchd` job. It
  starts with the daemon and stops with it.
- **`factory schedule create|list|enable|disable`.** A schedule is its own
  thing, with its own lifecycle, separate from `task send`.
- **Task templates.** A schedule never carries a prompt of its own. It names
  a template, and the template is what a run is made from.
- **An append-only audit trail per run**: `task_events` for every material
  transition, `task_decisions` for a decision with its rationale.
- **What a run cost**, when its adapter can say: the model, the input and
  output token counts, the duration, and how full the context was.
- **A second half to `factory doctor`'s check 6**, which can now answer
  "has anything dispatched against this database lately".

### Does not

- **No `launchd` job is installed, and none ever will be.** The dispatcher is
  a daemon thread. If you see a job under `com.business-factory.scheduler`, it
  is the prototype, not Factory.
- **No currency figure is stored.** Tokens and a model name are facts an
  adapter reports. A price is a rate table that changes without notice, and a
  stale one in a database reads like a measurement.
- **No `artifacts` table.** A result may name paths; nothing here copies file
  content into SQLite.
- **No automatic inspection gate, and no automatic rework.** A verification
  verdict is recorded and annotates a run. It never changes the run's status,
  and nothing decides on its own that a run must be redone.
- **No agent may commission a verification run.** Design §6 forbids a scope
  from targeting itself, and version 1 adds no exception to it.
- **The cut-over is not automatic.** Installing this build does not stop the
  prototype, does not move its data, and does not unload its `launchd` job.
  Procedure 7 is a thing you do, deliberately, by hand.

## The command surface

```text
factory schedule create <scope> (--template <name> | --task "<prompt>" --name <template-name>)
                                --cron "<expr>" --tz <IANA-name>
                                [--agent <agent-name>] [--acceptance "<criteria>"]
factory schedule list
factory schedule enable <schedule-id>
factory schedule disable <schedule-id>

factory task assign   --task-id <id> --agent-name <name>
factory task progress --task-id <id> --note "<text>"
factory task decision --task-id <id> --decision "<what>" --rationale "<why>"
                                     [--alternatives "<…>"] [--consequences "<…>"]
factory task verify   --task-id <id> --verdict "<verdict>" [--note "<text>"]
```

Every one of these is a client of the daemon, like every other command except
`doctor` (ADR 0014). With no daemon running they fail with the same message
`task send` gives you, and the same exit code.

The daemon operations behind them are `schedule.create`, `schedule.enable` and
`schedule.disable` (commands) and `schedule.list` (a query). They are listed
in the operation table in `crates/factory-daemon/src/lib.rs`, with every other
operation.

## Procedure 1 — create a schedule

A schedule needs a template. You can point at one that exists, or make one in
the same breath.

```sh
# Make the template and the schedule together.
factory schedule create irrlicht \
  --task "Analyse the release blockers and write a short report." \
  --name release-blockers \
  --cron "0 9 * * 1-5" \
  --tz Europe/Berlin \
  --agent lead

# Point a second schedule at the template that now exists.
factory schedule create irrlicht \
  --template release-blockers \
  --cron "0 17 * * 5" \
  --tz Europe/Berlin
```

Exactly one of the two forms. Both together, or neither, is refused — and it
is refused by the daemon, not by the argument parser, so a client speaking
JSON to the socket is held to the same rule.

**Why `--name` exists when design §7's example has no such flag.** A schedule
names a template, and a template's name is unique across the instance. The
design's example predates templates being a named thing. Deriving a name from
the prompt was considered and rejected: a derived name is either not unique,
which fails at a moment nobody is watching, or it is unique because it carries
a timestamp, which makes `schedule list` unreadable.

**The two together are one transaction.** If the schedule cannot be written,
the template is not left behind. Without that, your next attempt would fail on
the unique index, complaining about a template you did not think you had
created.

### What is checked, and by whom

- The **cron expression** is five fields, and the **timezone** is an IANA name
  such as `Europe/Berlin` — not an offset, which cannot express a rule that
  survives a daylight-saving change.
- Both are checked by `factory_task::schedule::validate`, in the daemon, and
  nowhere else. The CLI sends what you typed. A bad expression comes back as
  `validation.invalid_cron` and names the expression; a bad timezone as
  `validation.invalid_timezone` and names the zone.
- `--template` is refused when the template's state is not `open`, and the
  message names the state.

## Procedure 2 — see what is scheduled, and turn one off

```sh
factory schedule list
factory schedule disable <schedule-id>
factory schedule enable <schedule-id>
```

`list` shows each schedule's template, the cron expression and timezone
**exactly as you typed them**, whether it is enabled, when it last fired, and
when it will fire next.

The next run is computed with the same predicate the dispatcher uses to decide
that a schedule is due. There is deliberately no `next_run_at` column: a
stored one would be a second answer to a question the cron expression already
answers, and the two would drift.

`next_run` is a timestamp or nothing. When it is nothing, a separate
`next_run_state` says why — `disabled`, `never`, or `unreadable`. This is on
purpose: a field that sometimes holds a date and sometimes holds the word
"disabled" cannot be read by anything without guessing.

`enable` and `disable` change one flag. They do not delete the schedule, do
not touch its history, and do not affect a run it has already created.

## Procedure 3 — writing a run's audit trail

Four commands write to a run's history. None of them is a status change.

```sh
factory task assign   --task-id <id> --agent-name lead
factory task progress --task-id <id> --note "waiting on the release branch"
factory task decision --task-id <id> \
  --decision "ship without the migration" \
  --rationale "the migration needs a maintenance window we do not have" \
  --alternatives "delay the release by a week" \
  --consequences "the migration lands in the next release"
factory task verify   --task-id <id> --verdict pass --note "spot-checked the report"
```

**`--rationale` is required and may not be empty.** Design §11 asks for
decisions one can follow afterwards. A decision with no reason is a log line.

**A verdict annotates and never transitions.** It does not change the run's
status, does not create a rework, and does not commission an inspection.

**There is no `--session` flag on `verify`, and that is deliberate.** Factory
has no caller identity: a request names the scope a command *concerns*, not
the scope that issued it, and nothing tells the daemon which session ran a
command. An optional flag would be self-declared — an agent could omit it or
name someone else's session — and it would look like enforcement while being
none. The independence rule (a verdict may not be authored from inside the
scope under inspection) is enforced everywhere an author session *is* known,
and version 1 records verdicts as a human operation.

`assign` reports no change when it could not assign. A busy agent, or one with
no idle session, leaves the run where it was.

## Procedure 4 — what the dispatcher does, every twenty seconds

The dispatcher starts with the daemon and does three things each tick.

**1. It asks which enabled schedules match this minute.** Matching is a
predicate over the current local minute in the schedule's own timezone. It is
not "is now past the next run time", and `last_fired_at` is never read as an
input to the decision.

**2. It creates a run for each one that matches**, from that schedule's
template, in one transaction that also writes the run's `created` event and
stamps `last_fired_at`.

At most one run exists for a schedule and a local minute. That is a partial
unique index in the schema, not dispatcher code, so it holds across a
dispatcher restart, across two dispatchers, and across a hand-written `INSERT`.
On the autumn clock change the same local minute genuinely comes round twice,
an hour apart; the second attempt is an ordinary "already fired", not a fault.

**3. It records the tick**, whether or not anything fired. That is what lets
`factory doctor` tell "nothing was due" from "nothing is running".

### Why twenty seconds and not sixty

A schedule fires on a *match* against the current minute. A minute the loop
never samples is a fire lost for good — no later tick goes back for it. A
minute sampled three times costs two cheap "already fired" answers, which the
unique index makes free and correct.

Sixty seconds is the one wrong answer: any drift makes the loop land twice in
one minute and skip the next. The observe loop holds the store lock across
calls to Herdr and Irrlicht, so a dispatcher tick really can be delayed by
seconds. Twenty leaves room for that.

### What happens after a run is created

If the template names a target agent, the dispatcher attempts delivery: it
looks for an idle session for that agent, and writes the prompt into it.

If it cannot, **the run stays queued**. It is never marked failed and never
discarded. That covers all of:

- the template names no agent — design §11's run "queued for the central agent
  to assign";
- the agent has no idle session;
- the session refuses the prompt because it is busy — recorded as a `refused`
  event, which is deliberately *not* a delivery;
- the configuration does not describe the agent the template names.

The last one is the only one that names itself in the log, because it is the
only one no later tick can resolve on its own: nothing revalidates a template
against the configuration after the template is created, so a schedule can
fire forever against an agent that no longer exists. See Procedure 5.

### A template that is paused

A schedule whose template is `paused` or `closed` creates no run at all, and
this is not reported as a failure — you asked for it. It starts firing again
when the template is `open` again. Nothing is queued in the meantime; the
minutes that passed are gone.

## Procedure 5 — reading the dispatcher's log

The daemon's stderr goes to `<root>/.factory/daemon.log` when it was started
with `factory start`.

The dispatcher writes a line only when a schedule did **not** fire and should
have:

- a schedule whose cron expression or timezone cannot be read at all — which
  can only happen from a hand edit or a foreign restore, since `create`
  validates both;
- a schedule whose run could not be created;
- a template naming an agent this instance's configuration does not describe.

It says nothing when a schedule fires, and nothing when a schedule was already
fired for this minute. At three ticks a minute, "already fired" is the normal
answer for a healthy instance, and logging it would bury everything else.

**A broken schedule repeats its line on every tick, forever.** That is
deliberate. Telling "still broken" from "broken again" needs state the loop
does not keep, and a row nobody has fixed is exactly the thing an operator
should keep seeing.

## Procedure 6 — diagnosis

```sh
factory doctor --root <root>
```

Check 6 now answers two questions that are easy to confuse.

**Has Factory's own dispatcher run lately?** Read from `dispatcher_state`.

| What doctor finds | What it means | A finding? |
|---|---|---|
| No row | Nothing has ever dispatched against this database. Ordinary on a fresh instance. | No |
| A row, less than five minutes old | A dispatcher is running. | No |
| A row, older than five minutes | One ran and stopped. | **Yes** |
| A timestamp in the future | This database's clock and this machine's disagree. | **Yes** |
| Schema below 6 | The table does not exist yet. Reported as a skipped check, so the rest of the report still arrives. | Reported, not a fault |

Staleness is compared on the size of the gap, not its direction, so a
future-dated row cannot hide a dead dispatcher.

**Is anything else dispatching against this database?** Read from `launchctl
list com.business-factory.scheduler`.

| Label loaded | Factory's tick | What doctor says |
|---|---|---|
| No | any | Nothing. Not loaded is the correct end state after the cut-over. |
| Yes | never ticked | Nothing. This is every instance before its cut-over, including this machine today. |
| Yes | stale | The stale finding, only. A dispatcher that stopped is not a second writer. |
| Yes | fresh | **Two dispatchers against one database.** |

That last row is what ADR 0014 forbids, and it is the state Procedure 7 exists
to move you out of.

## Procedure 7 — the cut-over from the prototype

**Do not attempt this on the company root as an exercise.** Two things make it
a live operation: the prototype holds real data, and Factory's own migration 1
fails on that database outright with `table tasks already exists`. That
failure is the good one — a Factory that quietly adopted whichever `tasks`
table it found would mix two vocabularies of `status` in one column — but it
means the cut-over is a data move, not an install.

`docs/task-store-migration-plan.md` steps 1 through 4 are the first half of
this procedure. The order that matters:

1. **Stop the prototype first.** `launchctl remove com.business-factory.scheduler`.
   While it is loaded and Factory's daemon is running, two dispatchers are
   writing one file.
2. **Confirm it is gone.** `factory doctor` must stop reporting two
   dispatchers, or you have only stopped one of them.
3. **Move the data**, per the migration plan. Factory's table names stand; the
   prototype's are what change.
4. **Start the daemon and watch one tick land.** `factory doctor` should show
   a `last_tick_at` within the last minute.
5. **Create one schedule and watch it fire**, before you recreate all of them.

### The name `tasks` means opposite things in the two schemas

This is the most dangerous fact in the whole procedure, and it is easy to miss
because both sides look familiar. Read directly from
`scripts/factory_tasks.py` on 2026-09-10:

| Prototype table | What it holds | Factory table |
|---|---|---|
| `tasks` (`status` is `open`/`paused`/`closed`) | **the durable intent** — a template | `task_templates` (`state`, same three values) |
| `task_runs` (`status` is `queued`/`running`/…) | **one attempt** | `tasks` |
| `task_events` | keyed on the prototype's `tasks.id` | keyed on a *run* id |
| `task_decisions` | keyed on the prototype's `tasks.id` | keyed on a *run* id |
| `schedules` | a cron rule | `schedules` |
| `artifacts` | file references | *(none — ADR 0021 decision 9)* |

So the mapping is a **swap**, not a copy. `tasks` goes to `task_templates`,
and `task_runs` goes to `tasks`. A migration that moved `tasks` to `tasks`
would be putting template rows into the run table.

That particular mistake fails loudly rather than silently — `open` is not one
of Factory's run statuses and the CHECK constraint refuses it — but the two
event tables would not: both are keyed on a column called `task_id`, and in
the prototype that id is a template's, while in Factory it is a run's. Copied
across unchanged they would attach every event to the wrong thing, and nothing
would complain.

Migration 6 makes this a four-name collision, not the one
`docs/task-store-migration-plan.md` measured on 2026-09-08. There is a fifth
divergence with no name at all: the prototype tracks its migrations in a
`factory_schema_migrations` table, while Factory uses SQLite's own
`PRAGMA user_version`. A database carrying the prototype's schema therefore
reads as version 0 to Factory, which is why `factory doctor` skips the
dispatcher half of check 6 there rather than failing the whole report.

## What a run records now

Beyond what slice 10's guide describes, a run carries:

- **The template it came from, and the template version it executed.** That
  version is frozen at creation. Revising the template afterwards does not
  change what any existing run says it ran.
- **Its origin**: `manual`, or `cron` with the schedule and the local minute
  it fired for. A row cannot be half of each; the schema refuses it.
- **What it cost**, when the adapter can say: the model, input and output
  tokens, duration, and how full the context was. All of these are optional,
  and absent is the ordinary case — two of four live Pi sessions reported no
  metrics at all when this was measured, and opencode reports none by
  construction.
- **An append-only event for every material transition**, including a refusal,
  which is not a delivery.

**The token counts are a difference, not a reading.** A sample is taken at
delivery and stored on the run; the terminal sample subtracts from it. Both
adapters report counters that accumulate over a whole session, so without the
stored baseline the figure would be silently wrong after every daemon restart.

**Context occupancy is not a token count.** A harness reporting
`total_tokens` is describing how full the context window is, not what the run
consumed. It has its own column and never reaches a token count.

## Where this is proven

| Claim | Test |
|---|---|
| A real daemon process ticks the dispatcher | `crates/factory-cli/tests/cli.rs::a_real_daemon_process_writes_dispatcher_state` |
| A broken schedule names itself in the daemon's stderr | `crates/factory-cli/tests/cli.rs::a_schedule_with_an_unreadable_timezone_is_named_on_stderr` |
| One run per schedule per local minute, and the rest of the tick | `crates/factory-daemon/tests/dispatch.rs` |
| The schedule commands and their refusals | `crates/factory-daemon/tests/schedule.rs`, `crates/factory-task/tests/schedule.rs` |
| Cost is a difference, sampled before teardown | `crates/factory-daemon/tests/cost.rs` |
| Check 6's two halves and the schema gate | `crates/factory-doctor/tests/diagnose.rs` |
| The schema refuses what the code refuses | `crates/factory-store/tests/migration_6.rs` |
| A rework leaves the run it references untouched | `crates/factory-task/tests/rework.rs` |
| A verdict annotates and never transitions, and independence is by scope | `crates/factory-task/tests/verify.rs` |
| Each new command's envelope, and its refusals | `crates/factory-daemon/tests/ops_coverage.rs`, `crates/factory-cli/tests/cli.rs` |

Each of those has been mutated deliberately — the rule removed, the test
watched to die, the rule restored. A rule with no test that dies for it is not
enforced, whatever the code looks like.

## Open items this station leaves

- **The cut-over has not been performed**, and no live drill has run against a
  real Herdr pane on a throwaway root.
- **`check.sh` does not run `cargo doc`**, so a doc comment linking to a
  function that no longer exists is not an error. Measured on 2026-09-10:
  70 such errors across 8 of the 14 crates.
- **`factory scope add` still needs a YAML writer**, carried from slice 10.
- **`factory_recovery::reconnect`'s restart classes are still not wired into
  the daemon**, carried from slice 10.
- **A `paused` template's skipped minutes are not recorded anywhere.** Nothing
  says a schedule would have fired while its template was out of service.
