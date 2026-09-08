# Slice 9 — operator guide

Backlog §9 (`docs/implementation-backlog.md`) is reconciliation, restart
drills, and failure handling: "[m]ake recovery conservative and auditable
before unattended operation." Its scope line asks to "[r]un failure drills
for supervisor, Herdr, machine, harness, and harness-change events" — five
named events, covered by six procedures below because design §5's own
recovery table merges the middle two ("Herdr restart" and "Machine restart")
into a single acceptance criterion, exactly as `crates/factory-recovery/src/
reconnect.rs`'s own module docs merge them into one function. This document
is that runbook, in the same spirit as `docs/slice-1-operator-guide.md` and
`docs/slice-8-operator-guide.md`: read `docs/slice-8-operator-guide.md`'s own
"What this slice delivers, and what it does not" section first if you have
not — it is the part that keeps a guide like this one honest, and this
document follows its shape closely.

Two things are covered here beyond the five named events, both explicitly
asked for by this task:

- **The restore path** (`factory_recovery::restore::reconcile`) — ADR 0019's
  half of recovery that needs no live evidence at all, and the state every
  one of the five events below actually starts from in practice.
- **`factory_task::deliver::authorise_resume`** — new in this slice, and an
  operator has never seen it in a guide before: it is the "a human may
  resume" half of design §5's "a human may resume or create a replacement
  task," which had no mechanism until this slice built one.

## What this slice delivers, and what it does not

### Delivers

- **The two restart classes this slice's own agent built**, in
  `crates/factory-recovery/src/harness.rs`:
  - `harness_crashed(&mut Store, session_id, &Observation) -> Result<CrashOutcome, HarnessError>`
    — design §5's "Harness crash: mark session failed and current task
    blocked or failed," backed by a doc-comment evidence-to-outcome table and
    one test per row in `crates/factory-recovery/tests/harness_crash.rs`.
  - `harness_changed(&mut Store, scope_id, agent_name, &dyn Adapter) -> Result<Vec<HarnessChangeRecord>, HarnessError>`
    — design §5's "Harness change: stop the old session; queued tasks
    remain; running task requires review," backed by its own doc-comment
    table and `crates/factory-recovery/tests/harness_change.rs`.

  Both reuse `crate::evidence::may_promote_from_disconnected` — the one home
  of "only authoritative evidence may move a session" — rather than
  re-deriving it, and both reuse `crate::reconnect::give_up_on_disconnected_session`
  and `factory_session`'s own transition functions rather than writing a
  parallel state machine. Procedures 4 and 5 below are these two functions.

- **Three restart classes and the restore path, already built by earlier
  agents in this slice** — this guide documents them and runs them, it does
  not rebuild them: `factory_recovery::restore::reconcile`,
  `factory_recovery::reconnect::reconnect_after_supervisor_restart`,
  `factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart`,
  and the shared `factory_recovery::reconnect::give_up_on_disconnected_session`
  every "cannot positively reconnect" case in this slice routes through.
  Procedures 1 through 3 below are these.

- **`factory_task::deliver::authorise_resume`**, also already built earlier
  in this slice — "the first function in this crate to write the
  `blocked → queued` edge... nothing has used" until now, per its own doc
  comment. Procedure 6 below is this.

### Does not

- **There is still no `factory` binary.** Exactly as `docs/slice-1-operator-guide.md`
  and `docs/slice-8-operator-guide.md` say of their own slices: every
  procedure below drives the library crates directly, through a small test
  program written, run, and discarded for this guide. Nothing below is a
  preview of a CLI surface — `crates/factory-recovery/src/lib.rs`'s
  coordinator decision 1 is explicit that a `factory restore` (or any other)
  command is slice 10's to build.
- **No automated harness start, send, interrupt, or stop.** ADR 0017
  decision 5 holds unchanged through this slice: "the operator performs
  terminal actions; the adapter records." Concretely, `harness_changed`
  never spawns, signals, or messages any process — it only writes the
  database consequence of a harness change an operator has already made (or
  is about to make) by hand through Herdr. Procedure 5 says explicitly what
  an operator must do outside Factory before or alongside calling it.
- **No daemon, no loop, no scheduler.** Every function this guide calls takes
  `&mut factory_store::Store` and evidence supplied by the caller, and runs
  exactly once per call — `crates/factory-recovery/src/lib.rs`'s coordinator
  decision 1 again, and `harness.rs`'s own module docs restate it for the two
  functions this slice's agent added. Nothing here watches, waits, or
  retries.
- **No real Herdr anywhere in this guide.** Every `factory_adapter::Observation`
  below is a Rust value constructed directly in a test file, exactly as
  `crates/factory-recovery/tests/common/mod.rs::FakeAdapter` already does for
  the permanent suite — the same reasoning `docs/slice-8-operator-guide.md`'s
  Procedure 4 gives for not inventing a manual Herdr procedure it cannot
  verify: this task's own constraints forbid touching anything outside a
  fresh temp database, and an unverified procedure against a real harness
  would be worse than none.
- **No terminal attach**, unchanged from `docs/slice-8-operator-guide.md`'s
  own Procedure 4 — nothing in this slice adds a pane identity an operator
  could attach to.

## Getting a toolchain, and running the checks

`docs/slice-1-operator-guide.md`'s own "Getting a toolchain" and "Running the
checks" sections still apply unchanged — `rust-toolchain.toml` pins the same
`stable` channel, and `./check.sh` still runs `cargo fmt --all -- --check`,
then `cargo clippy --workspace --all-targets -- -D warnings`, then
`cargo test --workspace`. Every command in this document was actually run
with the toolchain put on `PATH` this way:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test -p factory-recovery --test operator_guide_scratch -- --nocapture --test-threads=1
```

`--test-threads=1` is there only so this document's captured transcript below
reads in the order the six procedures are written, not because any of these
functions have a concurrency requirement.

## The recovery functions, exactly

Signatures below are copied verbatim from `crates/factory-recovery/src/{restore,reconnect,harness}.rs`
and `crates/factory-task/src/deliver.rs` (doc comments trimmed for length —
read those files for the full reasoning behind each choice, including the
complete evidence-to-outcome tables `harness.rs`'s module docs give for the
two functions this slice's agent added).

```rust
// crates/factory-recovery/src/restore.rs — ADR 0019 decision 2, no live evidence.
pub fn reconcile(store: &mut factory_store::Store) -> Result<RestoreReport, RecoveryError>;

// crates/factory-recovery/src/reconnect.rs — presumes Herdr and harnesses alive.
pub fn reconnect_after_supervisor_restart(
    store: &mut Store,
    adapter: &dyn Adapter,
) -> Result<Vec<ReconnectRecord>, ReconnectError>;

// crates/factory-recovery/src/reconnect.rs — presumes the session gone unless proven otherwise.
pub fn reconnect_after_herdr_or_machine_restart(
    store: &mut Store,
    adapter: &dyn Adapter,
    max_sessions: u32,
    next_session_id: impl FnMut() -> uuid::Uuid,
) -> Result<Vec<ReconnectRecord>, ReconnectError>;

// crates/factory-recovery/src/reconnect.rs — the shared "give up" operation
// every "cannot positively reconnect" case above, and both functions below,
// route through.
pub fn give_up_on_disconnected_session(
    store: &mut Store,
    session_id: uuid::Uuid,
    reason: &str,
) -> Result<GiveUpOutcome, ReconnectError>;

// crates/factory-recovery/src/harness.rs — THIS slice's agent's own two functions.
pub fn harness_crashed(
    store: &mut Store,
    session_id: uuid::Uuid,
    observation: &Observation,
) -> Result<CrashOutcome, HarnessError>;

pub fn harness_changed(
    store: &mut Store,
    scope_id: uuid::Uuid,
    agent_name: &str,
    adapter: &dyn Adapter,
) -> Result<Vec<HarnessChangeRecord>, HarnessError>;

// crates/factory-task/src/deliver.rs — new this slice.
pub fn authorise_resume(store: &mut factory_store::Store, task_id: uuid::Uuid) -> Result<(), TaskError>;
```

### `harness_crashed`'s evidence-to-outcome table, condensed

(Full table and reasoning: `crates/factory-recovery/src/harness.rs`'s module
docs. `session_id` is presumed `running` when this is called — a session
Factory has already lost sight of is `reconnect.rs`'s exclusive territory,
not this function's.)

| Evidence | The task | Outcome |
|---|---|---|
| Authoritative, session dead | non-terminal | **Confirmed** — session → `failed` (lease released), task → `blocked: interrupted` |
| Authoritative, session dead | already `done`/`failed`/`cancelled` | **Confirmed** — session → `failed`, task left exactly where it was (design §5's "or failed") |
| Authoritative, session **alive** | — | **NotConfirmed** — the strongest evidence available contradicts the crash; nothing changes |
| Degraded / Unavailable | — | **Inconclusive** — session → `disconnected` (lease **retained**), task untouched |

### `harness_changed`'s evidence-to-outcome table, condensed

(Full table: same file. Applies once per currently lease-holding session of
`(scope_id, agent_name)`.)

| The session's task | Evidence on its pane | Outcome |
|---|---|---|
| one `running` | *not consulted* | **RequiresReview** — session → `disconnected` (lease **retained**), task → `blocked: interrupted` |
| none running, never reached `running` | *no pane exists to check* | **Retired** — session → `failed` |
| none running | authoritative + confirmed dead | **Retired** — session → `stopped` (or `failed`, above), lease released |
| none running | anything weaker, or "still alive" | **Inconclusive** — session → `disconnected`, lease retained |

Running work short-circuits evidence entirely in the first row, on purpose:
"a failed session releases its lease (ADR 0012 decision 5); a session that
merely needs review does not."

## Procedures 1–6, run together

Drop the following into a temporary file at
`crates/factory-recovery/tests/operator_guide_scratch.rs`, run it, read the
result, then discard it — exactly `docs/slice-1-operator-guide.md`'s own
"validate a config" procedure, applied here. This is the literal file this
guide's transcript was captured from. It uses `crates/factory-recovery/tests/common/mod.rs`
(`FakeAdapter`, `open_store`, the seeding and reading helpers) and a small
`harness_common` module written for this slice's own two new test files,
which seeds a session directly into `starting`/`running`/`disconnected`
(`crates/factory-recovery/tests/harness_common/mod.rs`) rather than the
hardcoded `disconnected` `common::seed_disconnected_session` produces.

```rust
mod common;
mod harness_common;

use common::*;
use factory_adapter::{Confidence, Observation, PaneId, TaskSignal};
use factory_recovery::harness::{harness_changed, harness_crashed};
use factory_recovery::reconnect::{
    reconnect_after_herdr_or_machine_restart, reconnect_after_supervisor_restart,
};
use factory_recovery::restore::reconcile;
use factory_store::Store;

fn observation(confidence: Confidence, session_alive: bool) -> Observation {
    Observation {
        pane: PaneId("wE:p1".to_string()),
        harness_state: "gone".to_string(),
        confidence,
        session_alive,
        task_signal: TaskSignal::NoChange,
        transcript_path: None,
        harness_session_id: None,
    }
}

/// A store rooted at `<tempdir>/.factory/factory.sqlite`, the shape
/// `factory_recovery::restore::reconcile` requires (`RecoveryError::
/// NotInDotFactory` otherwise).
fn open_dot_factory_store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open store under .factory");
    (dir, store)
}

#[test]
fn procedure_1_restore() {
    let (_dir, mut store) = open_dot_factory_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "acme", workspace.path());
    // A snapshot copied into place by hand: a `running` session (ADR 0019's
    // own example) with its task mid-flight and already delivered once.
    let session = uid(2);
    harness_common::seed_session(
        &mut store, session, scope, "pi", workspace.path(), "running", Some("wE:p1"),
    );
    let task = seed_task(&mut store, 3, scope, "running", None, Some(session));
    seed_delivery_attempt(&mut store, task, session);

    println!("[before restore] session={session} state=running  task={task} status=running");
    let report = reconcile(&mut store).expect("reconcile must succeed");
    println!(
        "[restore report] snapshot_user_version={} sessions_changed={} tasks_changed={} report_path={}",
        report.snapshot_user_version, report.sessions.len(), report.tasks.len(),
        report.report_path.display()
    );
    let report_contents = std::fs::read_to_string(&report.report_path).expect("read report file back");
    println!("[restore report, on disk] {} bytes, first line: {:?}",
        report_contents.len(), report_contents.lines().next());
    println!(
        "[after restore] session={session} state={}  task={task} status={:?}",
        session_state(&store, session), task_status(&store, task)
    );
    assert_eq!(session_state(&store, session), "disconnected");
    assert_eq!(task_status(&store, task), ("blocked".to_string(), Some("interrupted".to_string())));
    assert!(!report_contents.is_empty(), "the report file must actually contain the reconciliation it describes");
}

#[test]
fn procedure_2_supervisor_restart() {
    let (_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "acme", workspace.path());

    // Two `disconnected` sessions, exactly the row shape a supervisor
    // restart's own reconciliation would already have produced.
    let reconnects = seed_disconnected_session(
        &mut store, 2, scope, "pi", workspace.path(), Some("wE:p1"), None,
    );
    let gives_up_workspace = tempfile::tempdir().expect("tempdir");
    let gives_up = seed_disconnected_session(
        &mut store, 3, scope, "pi", gives_up_workspace.path(), Some("wE:p2"), None,
    );
    let gives_up_task = seed_task(&mut store, 4, scope, "running", None, Some(gives_up));

    let adapter = FakeAdapter::new()
        .with_observation("wE:p1", observation(Confidence::Authoritative, true))
        .with_observation("wE:p2", observation(Confidence::Degraded, false));

    let records = reconnect_after_supervisor_restart(&mut store, &adapter)
        .expect("reconnect_after_supervisor_restart must succeed");
    for r in &records {
        println!("[supervisor restart] session={} outcome={:?}", r.session_id, r.outcome);
    }
    println!(
        "[after] {reconnects} state={}   {gives_up} state={} task={:?}",
        session_state(&store, reconnects), session_state(&store, gives_up), task_status(&store, gives_up_task)
    );
    assert_eq!(session_state(&store, reconnects), "running");
    assert_eq!(session_state(&store, gives_up), "failed");
}

#[test]
fn procedure_3_herdr_or_machine_restart() {
    let (_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "acme", workspace.path());
    let existing_workspace = tempfile::tempdir().expect("tempdir");

    let recreated = seed_disconnected_session(
        &mut store, 2, scope, "pi", existing_workspace.path(), None, None,
    );
    // Unassigned: waiting for *any* idle session of this agent, not one this
    // specific disconnected session was ever handed — see
    // `tests/unrecoverable.rs::queued_task_with_a_prior_delivery_attempt_is_also_interrupted`
    // for the contrasting case where a queued task IS still assigned.
    let queued_task = seed_task(&mut store, 3, scope, "queued", None, None);

    let adapter = FakeAdapter::new();
    let mut next_id = 100u32;
    let records = reconnect_after_herdr_or_machine_restart(&mut store, &adapter, 4, || {
        next_id += 1;
        uid(next_id)
    })
    .expect("reconnect_after_herdr_or_machine_restart must succeed");
    let mut new_session_id = None;
    for r in &records {
        println!("[herdr/machine restart] session={} outcome={:?}", r.session_id, r.outcome);
        if let factory_recovery::reconnect::Outcome::Recreated { new_session_id: id, .. } = r.outcome {
            new_session_id = Some(id);
        }
    }
    let new_session_id = new_session_id.expect("one session must have been recreated");
    println!(
        "[after] old session state={}   new session {new_session_id} state={}   queued task status={:?}",
        session_state(&store, recreated), session_state(&store, new_session_id), task_status(&store, queued_task)
    );
    assert_eq!(session_state(&store, recreated), "failed");
    assert_eq!(session_state(&store, new_session_id), "starting");
    assert_eq!(task_status(&store, queued_task).0, "queued");
}

#[test]
fn procedure_4_harness_crash() {
    let (_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "acme", workspace.path());
    let session = uid(2);
    harness_common::seed_session(
        &mut store, session, scope, "pi", workspace.path(), "running", Some("wE:p1"),
    );
    let task = seed_task(&mut store, 3, scope, "running", None, Some(session));

    let confirmed = harness_crashed(&mut store, session, &observation(Confidence::Authoritative, false))
        .expect("harness_crashed must succeed");
    println!("[harness crash, confirmed] outcome={confirmed:?}");
    println!("[after] session={} task={:?}", session_state(&store, session), task_status(&store, task));
    assert_eq!(session_state(&store, session), "failed");

    // A second session, same crash notification, but only degraded evidence.
    let session2 = uid(4);
    harness_common::seed_session(
        &mut store, session2, scope, "pi", workspace.path(), "running", Some("wE:p3"),
    );
    let inconclusive = harness_crashed(&mut store, session2, &observation(Confidence::Degraded, false))
        .expect("harness_crashed must succeed");
    println!("[harness crash, degraded] outcome={inconclusive:?}");
    println!("[after] session={}", session_state(&store, session2));
    assert_eq!(session_state(&store, session2), "disconnected");
}

#[test]
fn procedure_5_harness_change() {
    let (_dir, mut store) = open_store();
    let workspace_a = tempfile::tempdir().expect("tempdir");
    let workspace_b = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "acme", workspace_a.path());

    // Confirmed dead: no work in flight, so this one retires cleanly.
    let idle_session = uid(2);
    harness_common::seed_session(
        &mut store, idle_session, scope, "pi", workspace_a.path(), "running", Some("wE:p9"),
    );
    // Running work: forces review regardless of what the same confirmed-dead
    // evidence would otherwise justify.
    let busy_session = uid(3);
    harness_common::seed_session(&mut store, busy_session, scope, "pi", workspace_b.path(), "running", None);
    let running_task = seed_task(&mut store, 4, scope, "running", None, Some(busy_session));
    let queued_task = seed_task(&mut store, 5, scope, "queued", None, Some(busy_session));

    let adapter = FakeAdapter::new()
        .with_observation("wE:p9", observation(Confidence::Authoritative, false));
    let records = harness_changed(&mut store, scope, "pi", &adapter).expect("harness_changed must succeed");
    for r in &records {
        println!("[harness change] session={} outcome={:?}", r.session_id, r.outcome);
    }
    println!(
        "[after] idle={}  busy={}  running_task={:?}  queued_task={:?}",
        session_state(&store, idle_session), session_state(&store, busy_session),
        task_status(&store, running_task), task_status(&store, queued_task)
    );
    assert_eq!(session_state(&store, idle_session), "stopped");
    assert!(every_lease_released(&store, idle_session));
    assert_eq!(session_state(&store, busy_session), "disconnected");
    assert!(!every_lease_released(&store, busy_session));
    assert_eq!(task_status(&store, queued_task).0, "queued");
}

#[test]
fn procedure_6_resume() {
    let (_dir, mut store) = open_store();
    let workspace = tempfile::tempdir().expect("tempdir");
    let scope = seed_scope(&mut store, 1, "acme", workspace.path());
    let session = uid(2);
    harness_common::seed_session(&mut store, session, scope, "pi", workspace.path(), "disconnected", None);
    let task = seed_task(&mut store, 3, scope, "blocked", Some("interrupted"), Some(session));
    seed_delivery_attempt(&mut store, task, session);

    let authorised_before: i64 = store.connection().query_row(
        "SELECT authorised_deliveries FROM tasks WHERE id = ?1", [task.to_string()], |row| row.get(0),
    ).expect("read authorised_deliveries");
    println!(
        "[before resume] task={task} status={:?} authorised_deliveries={authorised_before}",
        task_status(&store, task)
    );

    factory_task::deliver::authorise_resume(&mut store, task).expect("authorise_resume must succeed");

    let authorised_after: i64 = store.connection().query_row(
        "SELECT authorised_deliveries FROM tasks WHERE id = ?1", [task.to_string()], |row| row.get(0),
    ).expect("read authorised_deliveries");
    println!(
        "[after resume] task={task} status={:?} authorised_deliveries={authorised_after} assigned_session_id={:?}",
        task_status(&store, task), task_assigned_session_id(&store, task)
    );
    assert_eq!(task_status(&store, task), ("queued".to_string(), None));
    assert_eq!(authorised_before, 1);
    assert_eq!(authorised_after, 2);
    assert_eq!(task_assigned_session_id(&store, task), None);
}
```

Captured transcript (`cargo test -p factory-recovery --test operator_guide_scratch -- --nocapture --test-threads=1`), unedited:

```text
running 6 tests
test procedure_1_restore ... [before restore] session=00000000-0000-4000-8000-000000000002 state=running  task=00000000-0000-4000-8000-000000000003 status=running
[restore report] snapshot_user_version=5 sessions_changed=1 tasks_changed=1 report_path=/var/folders/7_/5tmjp7yd4qdfw8fs7tw1b3p80000gp/T/.tmpEmQLQn/.factory/restores/20260908T221342Z
[restore report, on disk] 403 bytes, first line: Some("Factory restore reconciliation (ADR 0019 decision 2)")
[after restore] session=00000000-0000-4000-8000-000000000002 state=disconnected  task=00000000-0000-4000-8000-000000000003 status=("blocked", Some("interrupted"))
ok
test procedure_2_supervisor_restart ... [supervisor restart] session=00000000-0000-4000-8000-000000000002 outcome=Reconnected
[supervisor restart] session=00000000-0000-4000-8000-000000000003 outcome=GivenUp(Interrupted { task_id: 00000000-0000-4000-8000-000000000004 })
[after] 00000000-0000-4000-8000-000000000002 state=running   00000000-0000-4000-8000-000000000003 state=failed task=("blocked", Some("interrupted"))
ok
test procedure_3_herdr_or_machine_restart ... [herdr/machine restart] session=00000000-0000-4000-8000-000000000002 outcome=Recreated { new_session_id: 00000000-0000-4000-8000-000000000065, outcome: Failed }
[after] old session state=failed   new session 00000000-0000-4000-8000-000000000065 state=starting   queued task status=("queued", None)
ok
test procedure_4_harness_crash ... [harness crash, confirmed] outcome=Confirmed(Interrupted { task_id: 00000000-0000-4000-8000-000000000003 })
[after] session=failed task=("blocked", Some("interrupted"))
[harness crash, degraded] outcome=Inconclusive
[after] session=disconnected
ok
test procedure_5_harness_change ... [harness change] session=00000000-0000-4000-8000-000000000002 outcome=Retired
[harness change] session=00000000-0000-4000-8000-000000000003 outcome=RequiresReview { task_id: 00000000-0000-4000-8000-000000000004 }
[after] idle=stopped  busy=disconnected  running_task=("blocked", Some("interrupted"))  queued_task=("queued", None)
ok
test procedure_6_resume ... [before resume] task=00000000-0000-4000-8000-000000000003 status=("blocked", Some("interrupted")) authorised_deliveries=1
[after resume] task=00000000-0000-4000-8000-000000000003 status=("queued", None) authorised_deliveries=2 assigned_session_id=None
ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

Read it against the six procedures.

### Procedure 1 — restore

**Operator observes:** a database file restored into place from a backup —
"an operator who copies a snapshot into place by hand" (ADR 0019 decision 1),
by any means outside Factory (a file copy, `VACUUM INTO`'s output moved back,
whatever the backup mechanism ADR 0012 decision 4 describes actually is).
There is no automated trigger for this: `reconcile` is called once,
explicitly, after the file is already in place.

**Factory claims afterwards:** every session that held a workspace lease
(`starting`, `running`, or `disconnected`) becomes `disconnected`, keeping
its lease — the snapshot's `running` session above became `disconnected`
without losing its `workspace_leases` row. Every task is decided by the
delivery journal, not status alone: the `running` task above, which also had
a `delivery_attempts` row, became `blocked: interrupted` — the report says so
(`tasks_changed=1`), and a plain-text copy lands under `.factory/restores/`
(`report_path` above) for later inspection — read back in the transcript
above (`[restore report, on disk]`), not merely asserted from the return
value: the file genuinely exists, is non-empty, and its first line names
what it is. `reconcile` also refuses to run at all against a `Store` not
rooted at a literal `.factory/` directory, which is why the scratch file's
own `open_dot_factory_store` uses `Store::open`, not `common::open_store`'s
`Store::open_at`.

**Operator must decide:** for the `blocked: interrupted` task, resume
(Procedure 6) or replace; for the now-`disconnected` session, which of
Procedures 2–5 applies to how this restart actually happened (was it a
supervisor restart, a Herdr restart, a machine restart, or is this session's
harness simply confirmed gone) — `reconcile` itself makes no such judgement,
by design (crate docs: "never asks Herdr anything... a snapshot may be hours
old, so a pane that exists is not evidence that *this* session exists").

**Exact call:** `factory_recovery::restore::reconcile(&mut store)`.

### Procedure 2 — Factory supervisor restart

**Operator observes:** the Factory daemon process itself restarted (a
deploy, a crash, a manual restart) — Herdr and every harness are presumed
still alive and unaffected.

**Factory claims afterwards:** for every `disconnected` session with a
recorded pane, `wE:p1`'s authoritative-but-*alive* reading promoted it
straight back to `running` (`Reconnected`) with no task consequence at all
— the escape hatch a supervisor restart is expected to use most of the time.
`wE:p2`'s only degraded evidence was not enough to trust, so that session
was given up on (`GivenUp`): `failed`, lease released, and its `running`
task rewritten to `blocked: interrupted`.

**Operator must decide:** nothing further for `Reconnected` sessions — they
are transparently recovered. For a `GivenUp` session's task, resume or
replace, same as Procedure 1.

**Exact call:** `factory_recovery::reconnect::reconnect_after_supervisor_restart(&mut store, &adapter)`.

### Procedure 3 — Herdr restart or machine restart

**Operator observes:** the terminal multiplexer restarted, or the whole
machine rebooted — every session is presumed **gone** unless proven
otherwise, the opposite presumption from Procedure 2.

**Factory claims afterwards:** with no pane recorded at all for the old
session, there was nothing to check — its recorded workspace still existed
on disk, so it was given up on and a **new** session was opened in the same
workspace, landing in `starting` with no pane recorded yet (`Recreated`) —
the transcript's own `[after]` line reads the new session's state back
directly (`state=starting`) rather than trusting the outcome's name alone.
Recording a real pane only happens once an operator actually launches a
harness there (ADR 0017 decision 5 again: "this slice records, it does not
launch"). The unassigned `queued` task was never inspected at all — its
status is exactly `queued` afterward, exactly as it was before, because
nothing in this function's recreation path reads or writes `tasks.status`
for a task that was never assigned to the session being replaced.

**Operator must decide:** physically start a harness in the recreated
session's workspace (a manual `herdr agent start` or equivalent, outside
Factory) so the next observation can promote it to `running`; the
unassigned `queued` task needs no decision — the ordinary assignment path
will pick it up once an idle session exists.

**Exact call:** `factory_recovery::reconnect::reconnect_after_herdr_or_machine_restart(&mut store, &adapter, max_sessions, next_session_id)`.

### Procedure 4 — harness crash

**Operator observes:** a live notification that **one** harness process
died — Herdr's lifecycle hook telling Factory a specific session's harness
exited, while every other session and Herdr itself are unaffected. This is
the one procedure in this guide that is not restart-shaped at all: nothing
about the supervisor, Herdr, or the machine changed.

**Factory claims afterwards, per the evidence table above:** the first
session's authoritative, session-dead reading was confirmed: `failed`
(lease released), its `running` task rewritten to `blocked: interrupted`
(`Confirmed(Interrupted { .. })`). The second session's only degraded
reading was not enough to conclude anything: `disconnected` (lease
**retained**), left for a later, stronger observation.

**Operator must decide:** for a `Confirmed` crash, resume or replace the
interrupted task, same as Procedures 1–2. For `Inconclusive`, there is
nothing to act on yet — the session is not lost (its lease says so), only
unproven; a later call to this same function with fresh evidence, or a
supervisor/Herdr restart drill's own reconciliation, is what eventually
resolves it.

**Exact call:** `factory_recovery::harness::harness_crashed(&mut store, session_id, &observation)`.

### Procedure 5 — harness change

**Operator observes:** their own deliberate decision to retire an agent's
current harness in favour of a different one (a version upgrade, switching
Pi for Claude Code) — the only procedure in this guide driven by an
operator's choice rather than a restart or a crash report. **Before, or
alongside, calling this function, the operator stops the old harness
process(es) by hand** — ADR 0017 decision 5 gives Factory no automated
`stop`, and this function does not pretend otherwise: it writes only the
database consequence of that decision.

**Factory claims afterwards:** both sessions saw the *same* authoritative,
confirmed-dead evidence on their recorded panes, and it produced two
deliberately different outcomes, which is the whole point of the "running
work short-circuits evidence" rule. The idle session (nothing running) was
retired cleanly: `stopped`, lease released (`Retired`) — the transcript's
`[after]` line confirms `idle=stopped` and its lease fully released, not
merely the outcome's name. The busy session's `running` task forced
`RequiresReview` regardless of that same strong evidence: `disconnected`
(lease **retained**, never `failed` or `stopped`), its task rewritten to
`blocked: interrupted`. Its sibling `queued` task, seeded on the very same
session, was never inspected at all and reads back exactly `queued`. (A
session with *no* pane recorded, or only weaker evidence, lands
`Inconclusive` instead of `Retired` — `disconnected`, lease retained,
nothing confirmed either way; see
`tests/harness_change.rs::a_session_with_no_recorded_pane_is_inconclusive_not_stopped`
and `::no_running_task_and_inconclusive_evidence_keeps_the_lease` for that
row, not re-demonstrated here since this transcript's own idle session
already has a pane and strong evidence to show the *other* two rows
side by side.)

**Operator must decide:** for `RequiresReview`, resume or replace the task,
same as every other procedure — and, having reviewed it, may now manually
call `factory_session::stop` or `factory_session::fail` to actually release
that session's lease, which this function deliberately never does on its
own. For `Inconclusive`, the same follow-up Procedure 4 describes: confirm
by hand (or a later pass with real pane evidence) before considering the
lease free. `Retired` needs no further decision at all — the lease is
already free.

**Idempotence:** calling this function again for the same `(scope_id,
agent_name)` with nothing else changed must write nothing new — in
particular, a session it already flagged `RequiresReview` must not be
retired on a second call just because the same confirmed-dead evidence is
still on hand for its pane. This is not automatic: `disconnected` is one of
the *inputs* this function's own session query accepts, so a naive
re-implementation would see "no running task" the second time (the task is
now `blocked`, not `running`) and retire it out from under an unreviewed
task — exactly ADR 0012 decision 5's hazard. See
`tests/harness_change.rs::calling_it_twice_does_not_retire_a_session_still_awaiting_review`,
not reproduced in this transcript since it needs two calls in one test
rather than a single narrated pass.

**Exact call:** `factory_recovery::harness::harness_changed(&mut store, scope_id, agent_name, &adapter)`.

### Procedure 6 — resume

**Operator observes:** a `blocked` task from any of the five procedures
above, which they have reviewed and judged safe to send one more prompt to,
rather than replace outright.

**Factory claims afterwards:** the task moved to `queued`
(`authorised_deliveries` incremented from 1 to 2, `assigned_session_id`
cleared, `blocked_reason` cleared) — "re-assignment is all it re-enters"
(`crate::valid_targets`'s own `blocked → queued` doc comment). This does
**not** deliver anything by itself: `deliver()`'s own guard is what actually
spends the authorisation this call just granted, the next time the ordinary
assignment path picks this task up.

**Operator must decide:** nothing further here — the ordinary assign/deliver
path (Slice 7, unchanged) now accepts exactly one more delivery for this
task, where it would otherwise have refused with "no remaining authorised
deliveries."

**Exact call:** `factory_task::deliver::authorise_resume(&mut store, task_id)`.

## Where this is proven

| Procedure | Proven by |
|---|---|
| Restore | `crates/factory-recovery/tests/restore.rs::reconcile_disconnects_every_lease_holding_session_and_keeps_its_lease`, `::reconcile_blocks_a_running_task_as_interrupted`, `::reconcile_blocks_a_queued_task_with_a_delivery_attempt_as_interrupted`, `::reconcile_leaves_a_queued_task_with_no_attempt_queued_but_clears_its_assignment`, `::reconcile_leaves_already_settled_tasks_completely_untouched`, `::reconcile_is_idempotent_a_second_run_changes_nothing`, `::reconcile_writes_a_report_under_dot_factory_naming_the_user_version`, `::reconcile_refuses_to_write_a_report_when_the_database_is_not_under_dot_factory` |
| Factory supervisor restart | `crates/factory-recovery/tests/supervisor_restart.rs::authoritative_matching_observation_promotes_to_running`, `::degraded_observation_gives_up_rather_than_promoting`, `::unavailable_observation_gives_up_rather_than_promoting`, `::authoritative_but_mismatched_harness_session_id_gives_up`, `::no_recorded_pane_is_left_untouched`, `::first_authoritative_reconnection_records_the_observed_harness_session_id`, `::an_already_recorded_identity_is_never_overwritten`, `::reconnecting_never_delivers_a_prompt`, `::adapter_error_on_one_session_does_not_block_another`, `::rerunning_after_promotion_is_a_no_op`; and `crates/factory-recovery/tests/unrecoverable.rs::ambiguous_evidence_via_supervisor_restart_reaches_give_up_with_history_intact` |
| Herdr restart / machine restart | `crates/factory-recovery/tests/herdr_or_machine_restart.rs::queued_tasks_remain_queued_through_recreation`, `::pane_recorded_but_non_authoritative_observation_still_recreates`, `::nonexistent_workspace_is_not_recreated`, `::live_evidence_of_the_same_session_blocks_recreation_and_promotes_instead`, `::rerunning_after_recreation_creates_no_second_session`, `::authoritative_but_reused_pane_recreates_rather_than_promotes`, `::existing_workspace_with_no_live_evidence_is_recreated` |
| The shared give-up operation | `crates/factory-recovery/tests/unrecoverable.rs::unrecoverable_session_with_no_attached_task_just_fails`, `::queued_task_with_a_prior_delivery_attempt_is_also_interrupted`, `::unrecoverable_session_fails_and_interrupts_its_non_terminal_task_while_keeping_delivery_history` |
| Harness crash | `crates/factory-recovery/tests/harness_crash.rs::authoritative_confirmed_crash_fails_session_and_interrupts_its_running_task`, `::authoritative_confirmed_crash_leaves_an_already_terminal_task_alone`, `::authoritative_but_alive_is_not_confirmed_a_crash`, `::degraded_evidence_is_inconclusive_and_keeps_the_lease`, `::unavailable_evidence_is_also_inconclusive`, `::a_second_crash_notification_for_the_same_session_is_a_caller_error` |
| Harness change | `crates/factory-recovery/tests/harness_change.rs::a_running_task_requires_review_and_keeps_the_lease_regardless_of_evidence`, `::a_queued_task_with_no_delivery_attempt_stays_queued_because_nothing_was_sent`, `::a_queued_task_with_a_prior_delivery_attempt_requires_review_like_running_work`, `::no_running_task_and_confirmed_dead_evidence_stops_the_session_cleanly`, `::no_running_task_and_inconclusive_evidence_keeps_the_lease`, `::no_running_task_and_still_alive_evidence_keeps_the_lease_too`, `::a_session_with_no_recorded_pane_is_inconclusive_not_stopped`, `::a_session_that_never_confirmed_running_fails_rather_than_stops`, `::only_sessions_of_the_named_agent_are_affected`, `::calling_it_twice_does_not_retire_a_session_still_awaiting_review` |
| Resume | `crates/factory-task/tests/deliver.rs::authorise_resume_of_a_nonexistent_task_is_not_found`, `::authorise_resume_of_a_terminal_task_is_refused`, `::authorise_resume_of_a_queued_task_is_refused`, `::authorise_resume_of_a_running_task_is_refused`, `::authorise_resume_moves_a_blocked_task_to_queued_and_grants_one_more_delivery`, `::a_task_can_be_delivered_again_after_authorise_resume_grants_a_second_delivery` |
| Real Herdr, read-only | `crates/factory-e2e/tests/live_herdr.rs::a_live_pi_pane_is_observed_and_reconnected` |
| Real Herdr, a real agent doing the work | `crates/factory-e2e/tests/live_agent.rs::a_real_agent_answers_a_factory_task` |
| The whole chain, offline | `crates/factory-e2e/tests/full_chain.rs`, `tests/delegation_tree.rs`, `tests/restart_drill.rs` |

## Running the two live drills

The last three rows above were added after this guide was first written, when
the drills existed to fill them. The two `live_*` drills are `#[ignore]`d:
`./check.sh` must not go red because a terminal was closed, since a suite that
does that teaches an operator to ignore red.

`live_herdr` is read-only against Herdr — it runs `agent list`, `agent get`
and `agent explain`, starts no pane and sends no prompt. It needs only a
running Herdr with at least one live `pi` pane:

```text
cargo test -p factory-e2e --test live_herdr -- --ignored --nocapture
```

`live_agent` writes a real prompt into a real terminal, so it needs preparing
first. Build a throwaway instance (an `AGENTS.md` per scope and a
`.factory/config.yaml`, as `crates/factory-e2e/tests/common/mod.rs` builds
one), then:

```text
herdr workspace create --cwd <instance>/worker --label factory-e2e --no-focus
herdr agent start e2e-worker --kind pi --pane <pane id from the line above>

FACTORY_E2E_ROOT=<instance> FACTORY_E2E_PANE=<pane id>   cargo test -p factory-e2e --test live_agent -- --ignored --nocapture
```

Close the workspace afterwards with `herdr workspace close <workspace id>`.

**Two traps this drill fell into before it was trustworthy**, both worth
knowing before writing any test that talks to a live agent. `herdr agent wait
--until idle` matches the `idle` the agent was in *before* the prompt, so the
prompt is submitted with `--wait --until working` and the wait that follows
cannot match the stale state. And asserting on a fixed answer word passes on
the *previous* run's reply still in the scrollback, so each run asks for a
token derived from its own task id. The first version of this drill went green
in 0.06 seconds without the agent having answered anything.

Every row above except the three live ones is part of the permanent suite
`./check.sh` already runs on every change; none of it depends on the scratch
file this document tells you to discard, which is not part of the crate's
test tree (see this task's own constraint: no real `.factory/factory.sqlite`
was opened anywhere in producing this document — every store above is a
fresh temp database created for exactly one procedure).
