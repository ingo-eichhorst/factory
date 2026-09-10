# ADR 0021: Task templates, runs, and the cron dispatcher

- Status: Accepted
- Date: 2026-09-10
- Owners: Business Factory
- Supersedes: `agent-task-scheduler-design.md`'s `tasks`-as-template naming
  (decision 1) and its `launchd` wake-up (decision 2)

## Context

Backlog §11 asks for durable task runs, an append-only audit log, structured
decisions, cron schedules, and the version-1 hooks of design §12 — "without
introducing a separate workflow engine." It names four things to resolve
before any of it is built:

1. whether a verification verdict may transition a run's state in version 1,
   or may only annotate it;
2. which harness adapters can report token counts at all, before cost metrics
   are treated as a required field;
3. who may legally create a verification run, given that design §6 forbids a
   scope from targeting itself;
4. whether `launchd` installation is production automation yet.

There is a fifth that the backlog does not name because it only becomes
visible once stations 7 through 10 exist. `agent-task-scheduler-design.md` was
written on 2026-08-07, before any of them. In its data model, `tasks` is *the
template* and `task_runs` is the execution. In the tree today, `tasks` is the
execution: it carries `assigned_session_id`, `authorised_deliveries`,
`delivery_attempts`, `task_delegation_chain`, the one-running-task-per-session
index, and every recovery rule ADR 0019 wrote. Two documents use the same word
for different objects, and a migration cannot be written until that is settled.

## Decision

### 1. The existing `tasks` row *is* the run. Templates sit above it.

Design §11 settles this in its own second sentence: task templates and runs are
"an extension of the existing `Task` primitive, not a new parallel workflow or
messaging system." So station 11 adds `task_templates` above `tasks` and gives
`tasks` the columns that make it a run — it does not add a `task_runs` table.

The alternative is worse than merely redundant. A separate `task_runs` table
would need its own assignment, its own delivery journal, its own
at-most-once rule, its own delegation chain and its own restore transitions —
a second copy of everything stations 7 through 10 built and drilled, differing
from the first in ways nobody would notice until a restart.

`agent-task-scheduler-design.md`'s naming is therefore superseded, not
followed. Its *contract* — a schedule only creates a run, never executes a free
prompt; a run without a target stays queued for central assignment; a busy or
stopped target loses nothing — is adopted unchanged.

Its **state vocabulary** is superseded for the same reason. That document lists
run states as `queued | assigned | running | blocked | done | failed |
cancelled`. Factory has no `assigned` status and is not to grow one:
assignment is `tasks.assigned_session_id`, and a task that has been assigned
but not yet delivered is still `queued`. Backlog §11's acceptance criterion 1
asks that an agent can "assign" a run, and it can — by writing that column and
an `assigned` **event**. Adding `'assigned'` to the status CHECK would unpick
`tasks_one_running_per_session`, whose whole reasoning is that `running` is the
one status that must name a session.

### 2. The dispatcher is a thread the daemon owns, not a `launchd` job.

ADR 0014 makes the daemon the sole owner of core state mutations. A `launchd`
job firing every minute could only be one of two things: a second process
mutating the database, which ADR 0014 forbids outright, or a client that wakes
up sixty times an hour to ask a daemon that is already running to do something
it could have done itself. `factory_daemon::observe::spawn` is the existing
precedent for exactly this shape, and the dispatcher is the same shape with a
different period.

This leaves `launchd` with one job Factory might one day want — starting the
*daemon* at login — which is a different concern from scheduling, is an explicit
macOS system change, and stays gated behind the explicit release
`agent-task-scheduler-design.md` requires in its closing line. Station 11 does
not install one.

**Consequence for `factory doctor`.** Backlog §10 asked doctor to report
"whether the scheduler job is loaded and when it last fired." Today check 6 can
answer only the first half, from `launchctl`. With the dispatcher inside the
daemon, the second half becomes answerable for the first time: the dispatcher's
own last tick, recorded in the database. Check 6 gains that.

It does **not** lose the `launchctl` half. This ADR's first draft said it
should, on the assumption that the label would never be loaded. That assumption
was wrong on this machine, and decision 10 is what measuring it produced.

### 3. Cron idempotency is a partial unique index, not dispatcher code.

`tasks` gains a nullable `schedule_id` and a nullable `fired_for_minute` — the
firing instant quantised to the minute, rendered in the schedule's own
timezone — under

```sql
CREATE UNIQUE INDEX tasks_one_run_per_schedule_minute
    ON tasks (schedule_id, fired_for_minute)
    WHERE schedule_id IS NOT NULL AND fired_for_minute IS NOT NULL;
```

This crate already enforces its two hardest invariants this way rather than
trusting application code to check first —
`sessions_one_live_lease_per_workspace` and `tasks_one_running_per_session` —
and both of those doc comments give the reason this one adopts.

The run and `schedules.last_fired_at` are written in **one transaction**. On
the autumn-back duplicate the insert is rejected, and a `last_fired_at` updated
outside that transaction would record a firing that did not happen — a
schedule that looks like it ran and did not is worse than one that looks like
it has never run.

That has a shape consequence worth stating, because it took two attempts to
get right. `schedules` is owned by `factory_task::schedule` and `tasks` by
`factory_task::create`, while the dispatcher lives in `factory-daemon` — so
the stamp cannot be a `&mut Store` function. `schedule::mark_fired` therefore
takes the caller's own `&Transaction`.

That alone was not enough. A dispatcher assembling the fire from
`create_from_template` (which commits) plus a second transaction that tagged
the row and stamped the schedule still left a window: a crash between the two
commits strands a run with `triggered_by = 'manual'` and no `schedule_id`,
which `tasks_one_run_per_schedule_minute` never indexes and no later tick can
find, so the schedule fires again for the same minute. A pre-check read cannot
close it, because the orphan carries nothing to find it by.

The whole fire is therefore one function in `factory-task`,
`create::create_from_schedule`: insert the run with its schedule and minute,
write the `created` event, stamp `last_fired_at`, one transaction. Detecting
the duplicate moved with it, and improved by moving — `factory-task` has
`rusqlite` as a direct dependency and matches
`ErrorCode::ConstraintViolation` against the index by name, where the daemon
could only have compared error text against a message that the same INSERT
also produces for a CHECK or foreign-key failure.

It matters most for the acceptance criterion that says "including after
dispatcher restarts." A dispatcher that remembers in memory which minutes it
has already fired is correct until it is killed mid-minute. An index is correct
because the second insert cannot happen, which is a property of the database
rather than of the dispatcher's lifetime.

Measured before writing this ADR, against SQLite 3 as bundled: a partial unique
index over two nullable columns rejects the duplicate (`UNIQUE constraint
failed: tasks.schedule_id, tasks.fired_for_minute`) and leaves every row with
NULLs unaffected.

`schedules` therefore stores `last_fired_at`, which is a fact, and does **not**
store a next-run time, which is a derivation. The next run is computed from the
cron expression when someone asks. A stored next-run column would be a second
home for the cron evaluator's answer, and it goes stale the moment a schedule
is edited, a timezone's rules change, or the process is down across it.

### 3a. Firing is a *match* on the local minute, and the next-run preview uses
the same predicate.

Two ways to run a cron schedule look equivalent and are not. The dispatcher can
ask "does this expression match the minute I am in," or it can ask a library
"when is the next occurrence after now." They disagree across a daylight-saving
transition, and the disagreement is silent.

Measured against `croner` 4.0.0 and `chrono-tz`, Europe/Berlin:

| Case | Local time | What matching does | What `find_next_occurrence` does |
|---|---|---|---|
| Spring forward, 2027-03-28 | 02:30 never occurs | never fires that day | **fires at 03:00** |
| Autumn back, 2027-10-31 | 02:30 occurs twice | one local minute string, so the unique index admits one run | two occurrences |

Factory matches. A schedule written for 02:30 means 02:30, and a clock that
skips 02:30 skips the run; silently moving it to 03:00 is the scheduler making
a decision the operator did not write down. The autumn case is why
`fired_for_minute` is the **local** minute string: both instants render
`2027-10-31T02:30`, so `tasks_one_run_per_schedule_minute` admits exactly one
run without the dispatcher having to know that the hour repeated. That was
measured, not assumed.

The consequence binds `factory schedule list`. Its "next run" column is
computed by stepping forward minute by minute with **the same matching
predicate the dispatcher uses**, not by asking the library for its next
occurrence. Otherwise the listing would promise a 03:00 run on the one morning
of the year that no run happens, which is worse than showing nothing: it is a
wrong answer that looks like an answer.

The walk is bounded, and the bound is chosen so that `None` means one thing
only: **this expression can never fire.** This ADR first said 400 days, on the
reasoning that nothing needs longer. That was wrong, and measuring it is what
found it: `0 0 29 2 *` fires on 29 February, and from 2027-01-01 its next match
is 2028-02-29, **424 days** out. At 400 days the preview answered `None` for an
ordinary working schedule three years in every four, and `factory schedule
list` would have shown a blank next-run column with no way to tell that from a
broken rule. The bound is 1500 days, a little over four years.

Measured on this machine, release build, Europe/Berlin:

| Expression | Steps | Time | Answer |
|---|---|---|---|
| `0 3 * * *` (daily) | 900 | 0.02 ms | next day, 03:00 |
| `0 4 1 1 *` (yearly) | 162,300 | 3.0 ms | 2027-01-01 04:00 |
| `0 0 29 2 *` (leap day) | 609,840 | 11.0 ms | 2028-02-29 00:00 |
| `0 0 30 2 *` (impossible) | 2,160,000 | 39.0 ms | `None` |

The worst case is the expression that can never fire, and it costs 39 ms to say
so — paid only by an operator who wrote a date that does not exist.

Stepping this way also shows what the dispatcher must expect on the autumn
morning. Both instants of the repeated 02:30 match, an hour apart, so the
dispatcher genuinely tries to fire twice and the second insert is rejected by
`tasks_one_run_per_schedule_minute`. **A unique-constraint violation on that
index is therefore an ordinary "already fired for this minute" outcome, not an
error to log or surface.** A dispatcher that treated it as a failure would
report a fault once a year, at 02:30, to nobody.

### 4. A verification verdict annotates. It never transitions.

Design §11 states it as a rule about the record — a verification verdict "never
replaces or overwrites that worker's own completion record" — and §12.1 states
it as a rule about the station: "the gate itself remains a human operation."
Version 1 therefore writes a `verification` row into `task_events` and touches
nothing else. No status changes, no rework is created, no yield is computed.

One invariant does bind, and it is the whole content of §12.1's
"independent": **the verdict's author must not belong to the scope under
inspection.**

Scope, not session. §12.1 says so in as many words — "a worker still cannot
raise a verification run against another session of *its own scope*. Allowing
sibling scopes does not change this: the inspecting session would have to live
in a different scope" — and design §6's "a scope may not target itself" is the
rule it is deriving from. A session-level guard would permit session B of scope
X to verify session A of scope X, which is precisely the case §12.1 rules out.

It also has a hole that the scope-level guard does not. `assigned_session_id`
is NULL for a task no session ever ran: one a human closed, one that was
cancelled while queued, one whose only delivery attempt was refused. A guard
comparing against it passes trivially on every such task. `target_scope_id` is
NOT NULL by schema, so the scope comparison has no such case.

Concretely: a verdict whose author is a session is refused when that session's
scope is the run's `target_scope_id`. A verdict with no author session is a
human's, and is allowed. This is a cross-table rule, so it is a code guard
rather than a CHECK, and it is one of this station's catching mutations.

### 4a. A refusal is an event, because station 10 decided it is not a delivery.

`Adapter::send` declines to submit into a session that is already working, and
station 10 made that refusal record `DeliveryOutcome::Refused` rather than a
delivery: nothing reached a terminal, so the task stays `queued`.

The audit log has to be able to say that. `task_events.event_type` therefore
carries `refused` alongside `delivered`. Without it, decision 3's "every
material transition writes exactly one event" would force one of two wrong
answers: no event, leaving a task sitting `queued` with nothing in the log
explaining why, or a `delivered` event for a prompt that was never sent. The
second is the confusion station 10 removed one layer up, reintroduced in the
place an operator goes to find out what happened.

### 4b. What the event vocabulary answers, and the two entries added while
building it.

`task_events.event_type` is not a copy of `tasks.status`. It answers a
different question — *what happened to this run* — and two entries were added
after the first wiring pass showed the vocabulary was short.

**`delivered` covers a `Failed` attempt as well as a `Sent` one.** The cut the
type makes is "did anything reach the agent," and station 10 already drew that
line once: `ATTEMPT_MAY_HAVE_REACHED_THE_TERMINAL` groups `Sent` and `Failed`
together and excludes only `Refused`, because a write that errored ambiguously
may well have landed. The event vocabulary uses the same cut rather than
inventing a second one, and the payload carries the outcome so a reader is
never told a failed attempt succeeded.

**`resumed` was missing, and its absence was the sharpest kind.**
`authorise_resume` moves `blocked → queued`, which design §11 counts as a
material status transition, and the first wiring pass correctly wrote no event
for it because no type fitted. That left the audit log silent about the one
transition an operator is most likely to go looking for later: a human
authorising a further delivery after a restart. The gap was the vocabulary's,
not the caller's, and it is fixed in the vocabulary.

There is deliberately **no `cancel_requested`**. Asking a running task to stop
records `tasks.cancel_requested_at` and changes no status, and the schema's own
comment on that column says why: it "is not a status change by itself."

**An event says that a result was reported. It does not repeat the result.**
The first wiring pass copied `result_summary` into the `done` and `failed`
payloads. `tasks.result_summary` is one join away by `task_id`, a summary may
be 16 KiB of agent-authored free text, and `task_events` is append-only and is
never corrected or deleted — so the copy would outlive any later edit of the
original and buy a reader nothing. The payload records that a result exists.

### 5. Nobody may create an agent-initiated verification run in version 1, and
that needs no exception to design §6.

§6 forbids a scope from targeting itself, and §12.1 observes that allowing
sibling scopes does not settle it either, since the inspecting session would
still have to live outside the scope under inspection. The question the backlog
asks is who gets an exception.

The answer is that no exception is needed, because nothing exists that would
use one. A verification verdict in version 1 is an event recorded against an
already-finished run — by a human, or by a session of a parent scope that
already may target the child. Neither needs a new rule. An exception written
now would be a rule with no caller, which this project has been burned by
before: slice 9 shipped `authorise_resume` with no caller and it stayed
unreachable until station 10 built one.

When something agent-initiated does exist, it will arrive with a concrete
caller, and the exception can be written against that caller rather than
against a guess about it.

### 6. Cost is one adapter method with two implementations, and `None` is a
valid answer.

Measured on 2026-09-10 against this machine's live sessions, because backlog
§11 asks to "confirm which harness adapters can report token counts at all
before treating cost metrics as a required field":

| Harness | Source | Shape | Join |
|---|---|---|---|
| Pi | its own session transcript, whose path Herdr reports as `agent_session.value` | **per message**: `usage {input, output, cacheRead, cacheWrite, reasoning, totalTokens}` and `model` | the pane, via `herdr agent get` |
| Claude Code | Irrlicht `metrics` | **cumulative per session**: `cum_input_tokens`, `cum_output_tokens`, `model_name` | `launcher.herdr_pane_id` |
| opencode | — | `metrics: null` | — |

Neither joins on working directory; ADR 0017's rule holds for cost exactly as
it holds for state.

Two consequences follow from the shapes differing.

**The columns are nullable and absent must read as absent.** Two of four live
Pi sessions reported no metrics at all, and opencode reports none by
construction. The acceptance criterion "remains valid when the adapter reports
none of them" is the ordinary case, not the edge.

**A baseline sample taken at delivery is itself a durable column.** A
cumulative source can only answer "what did *this run* cost" as a difference
between two samples, and a difference held in memory does not survive the
daemon restart this project drills for. So the sample taken when a task is
delivered is written to the run, in the adapter's own shape, and the terminal
sample subtracts from it. Without that column the figure is silently wrong
after every restart — which is the failure mode this project keeps finding and
keeps deciding to make structurally impossible instead.

### 7. `metrics.total_tokens` is context occupancy, not usage, and never lands
in a token-count column.

Measured across all 8 live sessions carrying the three fields, with no
exceptions:

```
total_tokens == context_window × context_utilization_percentage / 100
```

It is how full the context window is *right now*. It falls when a session
compacts. Storing it in a column documented as "tokens this run used" would be
the same class of defect as writing a placeholder process id into a column
documented to hold a UUID — a number of the right type, in the wrong column,
that nothing downstream can detect.

Only `cum_input_tokens` and `cum_output_tokens` are cumulative and therefore
subtractable. Those are the token counts design §12.6 asks for.

The occupancy figure is not discarded, because it answers a different and
useful question: a run that failed at 96 % context failed differently from one
that failed at 30 %, and tokens alone cannot tell them apart. It is stored as
context utilisation, under its own name, nullable and per-adapter.

### 8. No currency figure is stored.

Irrlicht reports `estimated_cost_usd` and Pi's transcript reports a per-message
`cost`. They are not the same kind of number: one is an estimate against
Irrlicht's price table, the other is the harness's own billing figure. Putting
both in one column would produce a number whose meaning depends on which
adapter wrote it, which is decision 7's defect wearing a different hat.

Model identifier and token counts are stored. Money is derived outside Factory,
where the price table has one owner.

### 9. No `artifacts` table.

`agent-task-scheduler-design.md` lists one. Station 7 already decided this
question the other way and its reasoning still holds: `tasks.result_artifact_paths`
is a JSON array because "this crate does not own artifact-path semantics."
Adding a child table now would create a second home for artifact paths, with
nothing to say which one a reader should trust.

### 10. Factory's table names stand. The prototype dispatcher is what station 11
replaces, and the cut-over is an operator's decision, not a migration's.

`docs/task-store-migration-plan.md` recorded a collision on 2026-09-08 and
deferred it here in as many words: "Resolving it belongs to slice 11, which
owns the shared task audit, and the choice is between renaming one side's table
and giving the Factory schema its own database file."

That plan measured **one** name in common, `tasks`. Migration 6 makes it
**four**. Measured on 2026-09-10 by reading
`scripts/factory_tasks.py`'s own `migrate()`, the Python prototype creates
`tasks`, `task_runs`, `task_events`, `task_decisions`, `artifacts`, `schedules`
and `factory_schema_migrations` in `<company root>/.factory/factory.sqlite` —
the same file design §4 pins Factory to. Three of those (`task_events`,
`task_decisions`, `schedules`) are migration 6's, and every one differs in its
key column, read from the prototype's own DDL rather than inferred:

| Table | Prototype | Migration 6 |
|---|---|---|
| `task_events` | `run_id` → `task_runs(id)`; `actor TEXT NOT NULL`; `payload_json NOT NULL CHECK (json_valid(...))`; no `event_type` CHECK | `task_id` → `tasks(id)`; `author_session_id` nullable → `sessions(id)`; `payload` nullable; `event_type` CHECKed to twelve values |
| `task_decisions` | `run_id` → `task_runs(id)`; `actor NOT NULL`; carries `sources` | `task_id` → `tasks(id)`; `author_session_id` nullable; no `sources` |
| `schedules` | `task_id` → `tasks(id)`, where its `tasks` *is* the template; `last_fired_minute`; `created_by NOT NULL` | `template_id` → `task_templates(id)`; `last_fired_at` |

The keys differ because the models do. The prototype has a separate `task_runs`
table and hangs everything off it; Factory does not (decision 1), so the same
rows hang off `tasks`. The `actor` difference is the same disagreement seen from
the other end: Factory's author column is a nullable foreign key to `sessions`,
where NULL means a human or the daemon, and the prototype requires a free-text
string.

There is a fifth divergence that is worse than any table shape.
`factory_schema_migrations` is the prototype's own migration bookkeeping, in the
same file as Factory's `PRAGMA user_version`. Two systems each believe they know
what version that database is, and neither can see the other's answer.

And the prototype is not dormant. `launchctl list com.business-factory.scheduler`
on this machine returns a loaded job, `LastExitStatus = 0`, running
`python3 scripts/factory_tasks.py dispatch --deliver`. It creates runs and
writes events today.

**Factory keeps its names.** Renaming Factory's tables would bend a schema to
fit a prototype that station 11 exists to replace, and a separate database file
would contradict design §4's one operational store. Neither option is
improved by the fact that the prototype holds live data: that data has to move
either way.

**The loud failure is kept, deliberately.** Opening the live company root fails
at migration 1 today with `table tasks already exists`, and will fail the same
way at migration 6. `docs/task-store-migration-plan.md` already argues this is
the good failure mode, "without touching data," and the alternative — a Factory
schema that quietly adopted whichever `tasks` it found — would mix two
vocabularies of `status` in one column. Nothing here weakens it.

**The cut-over is an explicit operator procedure and station 11 does not
perform it.** Stopping a live dispatcher and moving its data is a system
change, not a side effect of installing a build. The slice-11 operator guide
carries the procedure; `docs/task-store-migration-plan.md`'s own steps 1
through 4 are its first half.

**Consequence for `factory doctor`.** Check 6 reports two things, not one, and
they answer different questions:

1. The Factory dispatcher's own last tick, read from `dispatcher_state`. No row
   means no dispatcher has ever ticked against this database, which is ordinary
   on a fresh instance. A row whose timestamp is old means one ran and stopped,
   which is a finding.
2. Whether a job is loaded under `com.business-factory.scheduler`. **A loaded
   foreign scheduler while Factory's own daemon is dispatching is two
   dispatchers against one database**, which is precisely what ADR 0014
   forbids. That is a finding in its own right, and it is the check that would
   have caught this situation rather than leaving it to be read out of a
   `launchctl` listing by hand.

The earlier plan to retire the `launchctl` check outright was wrong, and it was
wrong because of an assumption rather than a measurement: this document's first
draft said the label "will never be loaded." It is loaded now.

Four further rules follow from those two halves. They are written down here
because none of them is obvious from the two paragraphs above, and a reader who
finds a green `factory doctor` next to a loaded prototype needs to know that is
the intended answer rather than a defect.

**A label that is not loaded is no longer a finding.** `SchedulerNotLoaded` is
deleted, not kept. Decision 2 makes Factory's dispatcher a daemon thread, so
"nothing is loaded under that label" is the *desired* end state after the
cut-over. A doctor that kept reporting it would be demanding that the retired
prototype stay installed forever.

**A loaded label beside a Factory dispatcher that has never ticked is not a
finding either.** That is the documented state of an instance that has not cut
over yet — every instance, today. It is reported as a fact in
`SchedulerStatus`, and nothing more. A check that goes red on every instance
before its cut-over teaches an operator to ignore check 6, which costs more
than it finds.

**A stale tick is reported as stale, never also as two dispatchers.** A
dispatcher that has stopped is not a second writer. Reporting both would name
one fault twice and describe the second one wrongly.

**Staleness is compared on the magnitude of the age, not its sign.** A
clock-skewed or hand-edited row can carry a timestamp in the future, and
treating "in the future" as fresh would mask a dead dispatcher for as long as
that row stands. The signed age is still carried in the finding, so a negative
number reads as what it is instead of as an implausible staleness.

**The `dispatcher_state` read is gated on the schema version.** Below migration
6 the table does not exist, which is the real shape of a machine still running
the prototype: it keeps its own `factory_schema_migrations` table, so
`PRAGMA user_version` reads 0 there. An ungated read would fail, and the
failure would propagate out of `diagnose` as an error — doctor reporting
*nothing at all* on exactly the instance an operator most needs it for. The
gate reports `CheckSkipped` instead, which is the pattern checks 3 and 5
already use for the same reason.

### 11. `factory schedule create` names the template it creates.

Design §7's example is `factory schedule create irrlicht --task "…" --cron
"0 9 * * 1-5" --tz Europe/Berlin`. It passes a prompt and nothing else.

Decision 1 puts a named template under every schedule, and `task_templates.name`
is `NOT NULL` with a `UNIQUE` index, so that example has nothing to name the
template it implies. The design predates templates being a named entity; it is
not wrong about the shape of the command, only silent about a field that did
not exist when it was written.

So `create` takes exactly one of two forms, and refuses both-or-neither:

- `--template <name>` uses an existing template, and refuses one whose state is
  not `open`.
- `--task "<prompt>" --name <template-name>` creates the template and the
  schedule **in one transaction**.

The alternative — deriving a template name from the prompt — was rejected. A
derived name is either not unique, which the index refuses at a moment nobody
is watching, or it is unique because it carries a timestamp or an id, which
makes `factory schedule list` unreadable. Neither is better than asking the
operator for a word.

The transaction is not decoration. Without it a failed schedule insert leaves a
named template behind, and the operator's next attempt fails on the unique
index with an error about a template they did not think they had created.

**`--template` refuses a template whose state is not `open`,** and the refusal
names the state. A `paused` or `closed` template is one an operator has taken
out of service, and a new schedule against it would be a rule written to be
ignored.

**The dispatcher does not yet honour that same state, and that is a defect.**
`dispatch::try_fire` reads the template's scope and prompt and never its
`state`, so a schedule created while a template was `open` keeps firing every
minute after the template is paused. The gate above and the dispatcher must
agree, or "paused" means one thing at creation and nothing afterwards. Both
halves belong to the same rule, so the dispatcher's half is on station 11's
closeout list rather than left to be discovered.

Skipping a paused template is not a failure and must not be logged as one: the
operator asked for it. It needs its own outcome, distinct from
`FireOutcome::Failed`, and `factory schedule list` is where an operator should
see that a schedule is enabled while its template is not.

## Consequences

- Migration 6 is additive: new tables, plus `ALTER TABLE tasks ADD COLUMN` for
  every new run field. No table rebuild, unlike migrations 2 and 3. Measured
  before writing this ADR: SQLite accepts `ADD COLUMN ... REFERENCES` when the
  default is NULL, and accepts `ADD COLUMN ... NOT NULL DEFAULT ... CHECK
  (...)` with the CHECK enforced on the next write.
- `factory doctor`'s check 6 gains a second half rather than being retired;
  decision 10 says why, and why the first draft of this ADR got that wrong.
  `docs/slice-10-operator-guide.md`'s Procedure 6 describes the old check and
  says the label reports nothing today, which is false on this machine. Both
  need the matching amendment, and it is on station 11's closeout list so that
  it is corrected deliberately rather than found stale during station 12's
  acceptance, which is how the slice 9 guide's table went stale.
- Cron parsing needs a five-field expression evaluated in a named IANA
  timezone. `croner` and `chrono-tz` are added for it, measured first: `croner`
  accepts a bare five-field expression (no seconds field to fake), enforces the
  weekday filter, handles step values, and rejects malformed input with a
  message worth showing an operator ("Pattern must have between 5 and 7
  fields"). Only its *matching* predicate is used; decision 3a says why its
  next-occurrence search is not.
- Design §12.1's inspection gate, §12.2's rework decision, §12.5's projections
  and `factory stats`, and §12.4's material paths remain unbuilt. Station 11
  carries durable fields and event types only, exactly as design §12 says
  version 1 does.
