# Slice 8 — operator guide

Backlog §8 (`docs/implementation-backlog.md`) is delegation policy and
multi-session manual rollout: one trust rule (design §6), the delegation
chain that makes peer delegation acyclic in practice, two independent
sessions of one agent, two agents of one scope running concurrently, and —
its closing acceptance criterion — "[t]he operator can inspect session,
task, lease, and compiled-context records without relying on terminal
scrollback." Its own Scope line also asks to "establish operator runbooks for
human targeting, parent-to-child delegation, task review, and terminal
attach." This document is that runbook, in the same spirit as
`docs/slice-1-operator-guide.md`: read that guide's own "What this slice
delivers, and what it does not" section first if you have not — it is the
part that keeps a guide like this one honest, and this document follows its
shape closely.

## What this slice delivers, and what it does not

### Delivers

- **The one version-1 delegation rule**, as two entry points in
  `factory-delegation`: `factory_delegation::queue::queue_from_human` (a
  human may target any *registered* scope) and
  `factory_delegation::queue::queue_from_session` (an agent session may
  target a registered descendant or sibling of its own scope, never an
  ancestor, never itself, and never a scope already in the task's own
  delegation chain). Both call through to `factory_task::create::create`,
  which they leave untouched — this slice's own crate documentation
  (`crates/factory-delegation/src/lib.rs`) is the authoritative record of the
  six decisions the rule was built on.
- **The delegation chain, persisted with the task in the same transaction**
  (`task_delegation_chain`, read back oldest-position-first by
  `factory_task::create::delegation_chain_of`) — so a two-step cycle
  (`A → B → A`) or a longer one is refusable after a restart, not only while
  running.
- **Two demonstrations that concurrency does not corrupt anything**: two
  sessions of one scope running separate tasks without sharing a workspace
  lease or a task, and two agents of one scope running concurrently — both
  already proven, not new machinery this task adds, by
  `crates/factory-task/tests/concurrency.rs`'s
  `two_sessions_of_one_scope_run_separate_tasks_concurrently` and
  `two_agents_of_one_scope_run_concurrently`.
- **Backlog §8's closing acceptance criterion.** Before this task,
  `factory_task::create::{list, show, delegation_chain_of}` already read
  tasks back, and `factory_context::compile` already compiled context — but
  nothing read sessions or their workspace-lease history. This task adds,
  to `factory-session` (`crates/factory-session/src/lib.rs`):
  - `Session` and `factory_session::list(&Store) -> Vec<Session>` — every
    session, oldest first (id, scope, agent name, state, workspace path,
    timestamps);
  - `factory_session::show(&Store, id) -> Session` — one session by id;
  - `WorkspaceLease` and
    `factory_session::leases_of_session(&Store, session_id) -> Vec<WorkspaceLease>`
    — the full `workspace_leases` journal for one session (`acquired_at`,
    `released_at`, `release_reason`), oldest acquisition first, released
    leases included.

  All three are read paths: they take `&Store` and go through
  `Store::connection()`, never `Store::transaction()` — see
  `Store::connection`'s own doc comment in `crates/factory-store/src/lib.rs`
  for why a read must not take the write lock under WAL.

### Does not

- **There is still no `factory` binary.** Exactly as `docs/slice-1-operator-guide.md`
  says of Slice 1: every procedure below drives the library crates directly,
  through a small test program you write, run, and discard. Nothing below
  should be read as a preview of a CLI surface.
- **No automated start, send, interrupt, or stop.** Backlog §5 is explicit
  that "an operator performs terminal actions" for Pi's own lifecycle, and
  `factory-adapter`'s `Adapter` trait (`crates/factory-adapter/src/lib.rs`)
  exposes exactly two methods, `observe` and `runtime_version` — its own doc
  comment says so directly: "Slice 5 implements only `observe`. Starting,
  sending, interrupting, and stopping remain documented operator
  procedures... Slice 10 automates what this slice proves." This task does
  not change that.
- **No terminal attach at all.** See Procedure 4 below for the specific
  evidence; this is deliberately stated plainly rather than worked around.
- **`target_agent_name` is still an open gap**, exactly as backlog §8's own
  "Left open by slice 8" note records: a task aimed at a scope that runs more
  than one agent (the `assistant` scope runs `assistant` and
  `assistant-chat`) cannot record which agent it was meant for once
  assignment picks one — resolving this needs a migration this task is
  forbidden from adding, and it is out of scope for the same reason it was
  out of scope when backlog §8 was first delivered.
- **This is cooperative policy, not hard security isolation** — design §6,
  verbatim: "[a]ll agents run under one trusted macOS account and may
  technically inspect files or invoke local tools outside their scope." The
  refusals demonstrated in Procedure 2 are Factory declining to *queue* a
  task on a caller's behalf; they are not a sandbox, and nothing stops an
  agent from doing the disallowed thing by hand outside Factory's own API.

## Getting a toolchain, and running the checks

`docs/slice-1-operator-guide.md`'s own "Getting a toolchain" and "Running the
checks" sections still apply unchanged — `rust-toolchain.toml` pins the same
`stable` channel, and `./check.sh` still runs `cargo fmt --all -- --check`,
then `cargo clippy --workspace --all-targets -- -D warnings`, then
`cargo test --workspace`, stopping at the first failure. Every command in
this document was actually run with the toolchain put on `PATH` this way:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test -p <crate> --test <file> -- --nocapture
```

`./check.sh` sources `~/.cargo/env` internally, so it needs no such prefix;
this form is for driving `cargo` directly, one crate at a time, the way every
procedure below does.

## The new readers, exactly

Every field and signature below is copied verbatim from
`crates/factory-session/src/lib.rs` (doc comments trimmed for length — read
that file for the full reasoning behind each choice):

```rust
pub struct Session {
    pub id: uuid::Uuid,
    pub scope_id: uuid::Uuid,
    pub agent_name: String,
    pub state: SessionState,
    pub workspace_path: String,
    pub created_at: String,
    pub updated_at: String,
}

pub fn list(store: &factory_store::Store) -> Result<Vec<Session>, SessionError>;
pub fn show(store: &factory_store::Store, id: uuid::Uuid) -> Result<Session, SessionError>;

pub struct WorkspaceLease {
    pub id: i64,
    pub canonical_workspace_path: String,
    pub acquired_at: String,
    pub released_at: Option<String>,
    pub release_reason: Option<String>,
}

pub fn leases_of_session(
    store: &factory_store::Store,
    session_id: uuid::Uuid,
) -> Result<Vec<WorkspaceLease>, SessionError>;
```

`list` orders oldest-created first (`ORDER BY created_at, id` — the `id`
tiebreaker matters because `created_at` has second resolution and two
sessions started in the same wall-clock second do tie on it).
`leases_of_session` orders oldest-acquired first (`ORDER BY id`, the lease
journal's own autoincrement key) and performs no existence check against
`sessions` — a `session_id` naming no session, or no lease rows, both
legitimately produce an empty `Vec`, mirroring
`factory_task::create::delegation_chain_of`'s own stance on an unknown
`task_id`.

## Procedure 1 — human targeting

Design §6: "[a] human may target any registered scope." Nothing more is
required of the caller than the target scope's id and a prompt;
`queue_from_human` resolves no session and needs none, because the sender is
not a scope at all (`Task::sender_scope_id` is `None` for a human-queued
task).

**Runnable:** see "Procedures 1–3, run together" below — the
`[human targeting]` line of that transcript is this procedure end to end.

## Procedure 2 — parent-to-child delegation, and two refusals

Design §6: "[a]n agent may target a registered descendant or sibling scope,
never an ancestor and never itself. A task carries its delegation chain, and
a scope already in that chain cannot be targeted again." The sender's scope
is never taken as a caller-supplied argument — `queue_from_session` takes a
*session id* and resolves `sessions.scope_id` from that row itself
(`factory_delegation`'s crate docs, decision 1: "a caller that may name its
own sender scope has no rule left to break").

An operator who has only ever seen the success path will not recognise a
refusal under pressure, so this procedure demonstrates both shapes
`factory_delegation::rule::DelegationError` can take here:

- `NotEligible { sender, target, reason }` — the target is the sender's own
  ancestor, itself, or unrelated (a nephew or cousin, reachable only through
  its parent).
- `AlreadyInChain { target }` — the target already appears in the task's own
  delegation chain, refused before any row is written, regardless of who is
  asking.

**Runnable:** see "Procedures 1–3, run together" below — the
`[parent -> child]` line is the success case; both `[parent -> child, refused]`
lines are the two refusals above, in order.

## Procedure 3 — task review

Status, result, and the delegation chain (`factory_task::create::{show,
delegation_chain_of}`), plus session and lease inspection
(`factory_session::{list, show, leases_of_session}`, added by this task).

**Runnable:** the `[task review]` lines of "Procedures 1–3, run together"
below cover status, result, and the chain, plus `factory_session::{list,
show}`. That section's own sessions carry no lease history (see its
explanation of why), so `leases_of_session` is covered separately by
"The lease journal," and compiled context by "Compiled-context inspection."

## Procedures 1–3, run together

`factory-delegation`'s own `Cargo.toml` already depends on `factory-session`,
`factory-store`, `factory-task`, `rusqlite`, and `uuid`, but deliberately not
on `factory-paths` — `factory-delegation/tests/queue.rs`'s own module docs
explain why: its rule only ever reads `sessions.scope_id` and
`tasks.(assigned_session_id, status)`, neither of which needs a real
workspace lease, so its whole test suite seeds session rows directly with
raw SQL rather than calling `factory_session::begin_start` (which requires a
resolved `factory_paths::CanonicalPath`, a dependency this crate has no other
reason to take). Procedures 1 through 3 below follow that same precedent,
which is why they demonstrate `factory_session::{list, show}` but not
`leases_of_session` — a directly-seeded session row carries no
`workspace_leases` row to read. `leases_of_session` is verified separately,
end to end through the real `begin_start → mark_running → stop` lease
lifecycle, in "The lease journal" section below.

Drop the following into a temporary file at
`crates/factory-delegation/tests/operator_guide_scratch.rs`, run it, read the
result, then discard it — exactly `docs/slice-1-operator-guide.md`'s own
"validate a config" procedure, applied here. This is the literal file this
guide's transcript was captured from:

```rust
use factory_store::Store;
use uuid::Uuid;

fn uid(seed: u32) -> Uuid {
    Uuid::parse_str(&format!("00000000-0000-4000-8000-{seed:012x}")).expect("valid uuid")
}

fn seed_scope(store: &mut Store, seed: u32, name: &str, parent_id: Option<Uuid>) -> Uuid {
    let id = uid(seed);
    let path = format!("/instance/scope-{seed}");
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO scopes (id, name, declared_path, canonical_path, parent_id) \
         VALUES (?1, ?2, ?3, ?3, ?4)",
        (id.to_string(), name, path, parent_id.map(|p| p.to_string())),
    )
    .expect("insert scope");
    tx.commit().expect("commit");
    id
}

/// Insert a `running` session row directly, exactly the way
/// `factory-delegation/tests/queue.rs::seed_session` already does — this
/// crate has no dependency on `factory-paths`, so a real
/// `factory_session::begin_start` (which requires a resolved
/// `CanonicalPath`) is not available here; a row inserted directly is
/// exactly as valid an input to `factory_delegation::queue` and to
/// `factory_session::{list, show}` as one `begin_start` would have produced.
/// It carries no `workspace_leases` row, which is why this file's task-review
/// section demonstrates `factory_session::{list, show}` but not
/// `leases_of_session` — that reader is instead verified end-to-end,
/// through the real `begin_start` → `mark_running` → `stop` lease lifecycle,
/// by `crates/factory-session/tests/read.rs`.
fn seed_session(store: &mut Store, seed: u32, scope_id: Uuid, agent_name: &str) -> Uuid {
    let id = uid(seed);
    let tx = store.transaction().expect("begin");
    tx.execute(
        "INSERT INTO sessions (id, scope_id, agent_name, workspace_path, state) \
         VALUES (?1, ?2, ?3, ?4, 'running')",
        (
            id.to_string(),
            scope_id.to_string(),
            agent_name,
            format!("/instance/workspace-{seed}"),
        ),
    )
    .expect("insert session");
    tx.commit().expect("commit");
    id
}

#[test]
fn operator_guide_full_workflow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut store = Store::open(dir.path()).expect("open");

    // A two-scope tree: `company` (root) and its registered child `billing`.
    let company = seed_scope(&mut store, 1, "company", None);
    let billing = seed_scope(&mut store, 2, "billing", Some(company));

    // ---- Procedure 1: human targeting -----------------------------------
    let onboarding_task = factory_delegation::queue::queue_from_human(
        &mut store,
        uid(100),
        company,
        None,
        None,
        "set up the quarter's board deck",
    )
    .expect("a human may queue to any registered scope");
    println!("[human targeting] queued task {onboarding_task} to scope `company` ({company})");

    // Give `company` a running session so it can delegate as an agent.
    let company_session = seed_session(&mut store, 10, company, "coordinator");
    println!(
        "[setup] session {company_session} is running for scope `company`, agent `coordinator`"
    );

    // ---- Procedure 2a: parent-to-child delegation (success) -------------
    let billing_task = factory_delegation::queue::queue_from_session(
        &mut store,
        uid(200),
        company_session,
        billing,
        None,
        None,
        "close the books for the quarter",
    )
    .expect("company (parent) may queue directly to billing (its child)");
    println!("[parent -> child] queued task {billing_task} to scope `billing` ({billing})");

    // ---- Procedure 3: task review -----------------------------------------
    let task = factory_task::create::show(&store, billing_task).expect("show");
    println!(
        "[task review] task {} status={} target_scope_id={}",
        task.id, task.status, task.target_scope_id
    );
    assert_eq!(task.status, factory_task::TaskStatus::Queued);
    assert_eq!(task.target_scope_id, billing);
    assert_eq!(task.sender_scope_id, Some(company));

    let chain = factory_task::create::delegation_chain_of(&store, billing_task).expect("chain");
    println!("[task review] delegation chain: {chain:?}");
    assert_eq!(chain, vec![company, billing]);

    // A second task, carried to completion, to show the *result* half of
    // "status, result, and the delegation chain" — `billing_task` above is
    // left `queued` because later steps still need it for the sibling-hop
    // scenario. `complete::done` only accepts `running`/`blocked` →
    // `done` (design §5 step 5), so the task is first assigned and marked
    // running directly — the same fixture shape
    // `factory-delegation/tests/queue.rs::mark_task_running` uses, standing
    // in for the `assign`/`deliver` steps this guide does not exercise.
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET status = 'running', assigned_session_id = ?2 WHERE id = ?1",
            (onboarding_task.to_string(), company_session.to_string()),
        )
        .expect("mark onboarding_task running");
        tx.commit().expect("commit");
    }
    factory_task::complete::done(
        &mut store,
        onboarding_task,
        Some("Board deck drafted; sent to `billing` for the numbers."),
        None,
    )
    .expect("mark the onboarding task done with a result");
    let onboarding = factory_task::create::show(&store, onboarding_task).expect("show");
    println!(
        "[task review] task {} status={} result_summary={:?}",
        onboarding.id, onboarding.status, onboarding.result_summary
    );
    assert_eq!(onboarding.status, factory_task::TaskStatus::Done);
    assert_eq!(
        onboarding.result_summary.as_deref(),
        Some("Board deck drafted; sent to `billing` for the numbers.")
    );

    let sessions = factory_session::list(&store).expect("list sessions");
    for s in &sessions {
        println!(
            "[task review] session {} scope={} agent={} state={} workspace={}",
            s.id, s.scope_id, s.agent_name, s.state, s.workspace_path
        );
    }
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, company_session);

    let session = factory_session::show(&store, company_session).expect("show session");
    assert_eq!(session.state, factory_session::SessionState::Running);

    // ---- Procedure 2b: parent-to-child delegation (a refusal) ------------
    // `billing` now has a running session of its own; escalating back to its
    // own ancestor `company` must be refused.
    let billing_session = seed_session(&mut store, 11, billing, "closer");

    let refusal = factory_delegation::queue::queue_from_session(
        &mut store,
        uid(201),
        billing_session,
        company,
        None,
        None,
        "escalate back to my own parent",
    )
    .expect_err("billing must not be able to target its own ancestor, company");
    println!("[parent -> child, refused] {refusal}");
    match &refusal {
        factory_delegation::rule::DelegationError::NotEligible { sender, target, .. } => {
            assert_eq!(*sender, billing);
            assert_eq!(*target, company);
        }
        other => panic!("expected NotEligible, got {other:?}"),
    }

    // ---- Procedure 2c: a second kind of refusal — AlreadyInChain ---------
    // `billing` hands the same task onward to a sibling, `finance` (also a
    // child of `company`), which then tries to hand it straight back to
    // `billing` — a two-step cycle the chain-membership gate must refuse.
    let finance = seed_scope(&mut store, 3, "finance", Some(company));
    let finance_session = seed_session(&mut store, 12, finance, "closer");

    // `billing_session` must actually be running `billing_task` for
    // `queue_from_session` to inherit its chain (`[company, billing]`)
    // rather than treating `billing_session` as idle (which would start a
    // fresh `[billing, ...]` chain and defeat this scenario) — mirrors
    // `factory-delegation/tests/queue.rs::mark_task_running`.
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET status = 'running', assigned_session_id = ?2 WHERE id = ?1",
            (billing_task.to_string(), billing_session.to_string()),
        )
        .expect("mark billing_task running");
        tx.commit().expect("commit");
    }

    // billing (currently running `billing_task`) hands off to its sibling
    // finance — a legal sibling hop, extending the chain to [company,
    // billing, finance].
    let handoff_task = factory_delegation::queue::queue_from_session(
        &mut store,
        uid(202),
        billing_session,
        finance,
        None,
        None,
        "reconcile with finance",
    )
    .expect("billing -> finance is a legal sibling hop");
    let handoff_chain =
        factory_task::create::delegation_chain_of(&store, handoff_task).expect("chain");
    assert_eq!(handoff_chain, vec![company, billing, finance]);

    // finance must run that task before it can delegate from it.
    {
        let tx = store.transaction().expect("begin");
        tx.execute(
            "UPDATE tasks SET status = 'running', assigned_session_id = ?2 WHERE id = ?1",
            (handoff_task.to_string(), finance_session.to_string()),
        )
        .expect("mark handoff task running");
        tx.commit().expect("commit");
    }

    let cycle_refusal = factory_delegation::queue::queue_from_session(
        &mut store,
        uid(203),
        finance_session,
        billing,
        None,
        None,
        "hand it back to billing",
    )
    .expect_err("billing already appears in this task's chain");
    println!("[parent -> child, refused] {cycle_refusal}");
    match &cycle_refusal {
        factory_delegation::rule::DelegationError::AlreadyInChain { target } => {
            assert_eq!(*target, billing);
        }
        other => panic!("expected AlreadyInChain, got {other:?}"),
    }
}
```

Run it:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test -p factory-delegation --test operator_guide_scratch -- --nocapture
```

The captured transcript, verbatim:

```text
running 1 test
[human targeting] queued task 00000000-0000-4000-8000-000000000064 to scope `company` (00000000-0000-4000-8000-000000000001)
[setup] session 00000000-0000-4000-8000-00000000000a is running for scope `company`, agent `coordinator`
[parent -> child] queued task 00000000-0000-4000-8000-0000000000c8 to scope `billing` (00000000-0000-4000-8000-000000000002)
[task review] task 00000000-0000-4000-8000-0000000000c8 status=queued target_scope_id=00000000-0000-4000-8000-000000000002
[task review] delegation chain: [00000000-0000-4000-8000-000000000001, 00000000-0000-4000-8000-000000000002]
[task review] task 00000000-0000-4000-8000-000000000064 status=done result_summary=Some("Board deck drafted; sent to `billing` for the numbers.")
[task review] session 00000000-0000-4000-8000-00000000000a scope=00000000-0000-4000-8000-000000000001 agent=coordinator state=running workspace=/instance/workspace-10
[parent -> child, refused] scope 00000000-0000-4000-8000-000000000002 may not target scope 00000000-0000-4000-8000-000000000001 — 00000000-0000-4000-8000-000000000001 is its own ancestor of 00000000-0000-4000-8000-000000000002
  help: design §6: an agent may target only a registered descendant or sibling scope; a nephew or cousin is reached through its parent
[parent -> child, refused] scope 00000000-0000-4000-8000-000000000002 already appears in this task's delegation chain
  help: design §6: a scope already in the chain cannot be targeted again — refused before any row is written
test operator_guide_full_workflow ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

Read it against the three procedures:

- **Human targeting**: `queue_from_human` needed only `company`'s id and a
  prompt — no session, no scope of its own to check kinship against.
- **Parent-to-child delegation**: `company_session` (running in scope
  `company`) queued directly to `billing`, a registered child — the chain
  became `[company, billing]`. The two refusals that follow are the ones an
  operator must be able to recognise: `billing` targeting its own ancestor
  `company` comes back `NotEligible`, naming both scopes and the reason
  ("its own ancestor of"); `finance` (having legally received the task from
  `billing`, extending the chain to `[company, billing, finance]`) handing it
  straight back to `billing` comes back `AlreadyInChain`, naming the scope
  already in the chain. Both refusals leave the message's own `help:` line
  pointing at design §6, and neither writes a `tasks` row — `queue_from_human`
  and `queue_from_session` are `factory_delegation::queue`'s only writers,
  and both call `rule::check` before either one ever reaches
  `factory_task::create::create` (see `crates/factory-delegation/src/queue.rs`'s
  own module docs).
- **Task review**: `factory_task::create::show` returned `billing_task`'s
  `status` (`queued`) and `target_scope_id`; `delegation_chain_of` returned
  its chain; a second task (`onboarding_task`) was carried to `done` with a
  `result_summary` to show the *result* half of review, since `billing_task`
  itself was needed `queued` for the later sibling-hop scenario.
  `factory_session::list` and `factory_session::show` — the readers this
  task adds — returned the one live session, typed (`state` is a
  `SessionState`, not a bare string).

One refusal this file does not demonstrate, because `queue_from_human`'s own
existence check makes it identical in shape to the agent-sender case: a human
targeting an id that names no registered scope at all comes back
`DelegationError::Registry(RegistryError::UnknownScope { id })` — "the word
[registered] is load-bearing," per `factory_delegation`'s own crate docs. This
is already proven by the permanent, `./check.sh`-run suite at
`crates/factory-delegation/tests/queue.rs::human_to_unregistered_scope_is_refused`
and its agent-sender twin `scope_to_unregistered_target_is_refused`, so it is
cited here rather than re-demonstrated.

## The lease journal

`leases_of_session` needs a session that actually went through
`factory_session::begin_start`, which needs `factory-paths`' `CanonicalPath`
— not available inside `factory-delegation`'s test target (see above), so
this is verified separately, permanently, in
`crates/factory-session/tests/read.rs`. Run it:

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test -p factory-session --test read
```

```text
running 7 tests
test show_of_a_nonexistent_session_is_not_found ... ok
test leases_of_session_for_a_session_with_no_lease_rows_is_empty ... ok
test list_breaks_a_created_at_tie_by_id ... ok
test leases_of_session_never_returns_another_sessions_lease ... ok
test list_returns_every_session_oldest_first ... ok
test show_returns_the_named_session_not_the_first_one ... ok
test leases_of_session_returns_a_released_lease_not_just_open_ones ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

(`cargo test` runs a crate's `#[test]` functions in parallel by default, so the
order these lines print in is not itself meaningful — only which tests appear
and whether each says `ok`.)

The test that matters most for "not just whether one is held now" is
`leases_of_session_returns_a_released_lease_not_just_open_ones`: it starts a
session, marks it running, then `stop`s it (releasing the lease with a
reason), then calls `leases_of_session` and asserts the one lease row still
comes back — `released_at` populated, `release_reason` equal to the string
`stop` was given. An operator reading a session whose `state` (from `show`)
is already `stopped` or `failed` can still see *when* it acquired its
workspace, *when* it let go, and *why* — the whole point of a journal, not a
flag.

## Compiled-context inspection

Backlog §8's closing criterion also names "compiled-context records."
`factory_context::compile` (`crates/factory-context/src/lib.rs`) already
existed before this task — it is a pure function (ADR 0013: opens files for
reading, creates nothing) that takes the scopes root-to-leaf, the agent
definition, and an optional task prompt, and returns a `CompiledContext`
whose `sources: Vec<SourceReport>` is exactly what `factory context show`
would eventually render: one entry per contributing source, each with a
`label`, an optional `path`, and the number of bytes it contributed.

The real registration fixture (`fixtures/registration/`) already exercises
this end to end, permanently, in
`crates/factory-context/tests/fixture.rs::the_registration_fixture_compiles_as_a_two_scope_chain`.
To see the `SourceReport` account an operator would actually read, drop this
into a temporary file at `crates/factory-context/tests/operator_guide_scratch.rs`,
run it, and discard it:

```rust
use factory_context::{ScopeContext, compile};
use std::path::PathBuf;

fn fixture_root() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/registration"
    ))
}

#[test]
fn print_the_source_report_for_the_registration_fixture() {
    let root = fixture_root();
    let scopes = vec![
        ScopeContext {
            scope_name: "fixture-co".to_string(),
            context_file: root.join("AGENTS.md"),
        },
        ScopeContext {
            scope_name: "example-project".to_string(),
            context_file: root.join("projects/example-project/AGENTS.md"),
        },
    ];
    let agent = factory_config::Agent {
        name: "example-project".to_string(),
        harness: factory_config::Harness::Pi,
        max_sessions: 1,
        lifetime: factory_config::Lifetime::Permanent,
    };

    let compiled = compile(&scopes, &agent, Some("Review the open pull request."))
        .expect("the fixture's AGENTS.md files are readable");

    for source in &compiled.sources {
        println!(
            "[context show] {} ({} bytes){}",
            source.label,
            source.bytes,
            source
                .path
                .as_ref()
                .map(|p| format!(" — {}", p.display()))
                .unwrap_or_default()
        );
    }
    println!(
        "[context show] total compiled text: {} bytes",
        compiled.text.len()
    );

    assert_eq!(
        compiled.sources.iter().map(|s| s.bytes).sum::<usize>(),
        compiled.text.len(),
        "sources must account for every byte of the compiled text"
    );
}
```

```sh
PATH="$HOME/.cargo/bin:$PATH" cargo test -p factory-context --test operator_guide_scratch -- --nocapture
```

```text
running 1 test
[context show] Context: fixture-co (1648 bytes) — /Users/factory/business-factory/projects/factory/crates/factory-context/../../fixtures/registration/AGENTS.md
[context show] Context: example-project (1893 bytes) — /Users/factory/business-factory/projects/factory/crates/factory-context/../../fixtures/registration/projects/example-project/AGENTS.md
[context show] Agent definition (250 bytes)
[context show] Task (198 bytes)
[context show] total compiled text: 3989 bytes
test print_the_source_report_for_the_registration_fixture ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The `path` column is only present for the two `AGENTS.md` sources — the
agent definition and the task prompt are not files, matching
`SourceReport::path`'s own doc comment ("`None` for sections that are not
files"). The final assertion — every source's `bytes` sums to exactly
`compiled.text.len()` — is `compile`'s own accounting guarantee, not
something this scratch file adds: `SourceReport::bytes`'s doc comment states
it counts "the whole rendered section," specifically so this reconciliation
is exact rather than approximate.

## Procedure 4 — terminal attach: not available in this slice

Backlog line 661 (§10's acceptance criteria) states plainly that "attach
reaches the native Pi or Claude Code terminal" — as a **Slice 10**
acceptance criterion, not Slice 8's. Measured against the actual code and
schema rather than assumed, four things confirm it is not available yet, and
this guide states that plainly instead of inventing a workaround:

1. **No pane identity is persisted anywhere.** `crates/factory-store/src/schema.rs`
   has no column recording a Herdr pane id on `sessions` or anywhere else —
   `grep -n pane crates/factory-store/src/schema.rs` returns nothing. Design
   §3 names the pane as "the one identifier Factory chose and recorded" for
   *observation* purposes (`crates/factory-adapter/src/lib.rs`'s module
   docs), but nothing durable joins a `sessions` row to the pane an operator
   would need to attach to.
2. **The adapter contract has no attach method.** `factory_adapter::Adapter`
   (`crates/factory-adapter/src/lib.rs`) declares exactly `observe` and
   `runtime_version`. There is no `start`, `send`, `interrupt`, `stop`, or
   `attach` — the trait's own doc comment: "Slice 5 implements only
   `observe`... Slice 10 automates what this slice proves."
3. **Design §3's own architecture diagram assigns "terminal attachment and
   rendering" to Herdr, not to Factory** — "Herdr owns: persistent PTYs;
   workspaces, tabs, and panes; terminal attachment and rendering; transport
   of input and output." Factory's own boundary in that diagram stops at
   "starting, stopping, observing, and reconciling agents."
4. **Backlog §5 says an operator performs terminal actions by hand today**:
   "Persist only observations in the database; an operator performs terminal
   actions." No slice between 5 and this one changes that, and no
   `docs/slice-5-operator-guide.md` (or any other written runbook for
   manually reaching a Pi or Claude Code pane) exists anywhere in this
   repository as of this task.

Given all four, this guide does not offer a manual "use Herdr directly"
procedure either: doing so accurately would require a real Herdr
installation and live Pi/Claude Code sessions to actually run and verify,
which this task's constraints (no touching anything outside a fresh temp
database, and no live-instance access at all) rule out — and an unverified
procedure is worse than none, per this task's own acceptance standard. The
honest statement is the one above: attach is Slice 10's acceptance
criterion, not delivered, and nothing in this slice's code, schema, or
documentation lets an operator reach a running agent's terminal through
Factory.

## Where this is proven

| Procedure | Proven by |
|---|---|
| Human targeting | `crates/factory-delegation/tests/queue.rs::human_can_queue_to_any_registered_scope`, `::human_to_unregistered_scope_is_refused` |
| Parent-to-child delegation | `crates/factory-delegation/tests/queue.rs::parent_can_queue_directly_to_child`, `::child_to_parent_is_refused`, `::scope_can_target_its_sibling`, `::scope_to_nephew_is_refused`, `::scope_to_cousin_is_refused`, `::two_step_cycle_is_refused`, `::three_step_cycle_is_refused` |
| Task review — task and chain | `crates/factory-task/tests/create.rs::list_returns_every_task_oldest_first`, `::show_of_a_nonexistent_task_is_not_found`, `::delegation_chain_of_round_trips_a_three_scope_chain_in_position_order` |
| Task review — sessions | `crates/factory-session/tests/read.rs::list_returns_every_session_oldest_first`, `::list_breaks_a_created_at_tie_by_id`, `::show_returns_the_named_session_not_the_first_one`, `::show_of_a_nonexistent_session_is_not_found` |
| Task review — lease journal | `crates/factory-session/tests/read.rs::leases_of_session_returns_a_released_lease_not_just_open_ones`, `::leases_of_session_never_returns_another_sessions_lease`, `::leases_of_session_for_a_session_with_no_lease_rows_is_empty` |
| Task review — compiled context | `crates/factory-context/tests/fixture.rs::the_registration_fixture_compiles_as_a_two_scope_chain` |
| Concurrency (delivered, not new) | `crates/factory-task/tests/concurrency.rs::two_sessions_of_one_scope_run_separate_tasks_concurrently`, `::two_agents_of_one_scope_run_concurrently` |
| Terminal attach | Not proven — not implemented; see Procedure 4 |

Every row above except "Terminal attach" is part of the permanent suite
`./check.sh` already runs on every change; none of it depends on a scratch
file this document tells you to discard.
