# Version-1 Implementation Backlog

## Delivery approach

Build and exercise each slice first with documented operator commands and
fixtures. Keep domain state and boundary contracts independent of the command
line so the Factory CLI can replace manual procedures without changing the
model. No slice may make external changes (create worktrees, merge branches,
or alter harness-owned files) as part of normal operation.

## 1. Repository contract and configuration validation

**Objective:** Establish the safe, inspectable local contract for a company root
and a registered scope.

**Scope:** Define version-1 YAML schemas for `.factory/config.yaml`, defaults
(`max_sessions: 1`), UUID and harness validation, canonical-path utilities, and
human-readable validation errors. Document a manual registration fixture:
company-root config, child scope config, and `AGENTS.md` files.

**Dependencies:** None.

**Acceptance criteria:**
- Valid root and child configuration loads into a typed scope/agent model.
- Missing or malformed fields, unsupported versions, invalid UUIDs, duplicate
  IDs, and unsupported harnesses fail with the file and corrective action.
- Relative paths, `..`, and symlinked paths resolve to canonical absolute paths
  before comparison.
- Validation never writes or overwrites `AGENTS.md`, `CLAUDE.md`, `.pi/`, or
  `.claude/`.

**Decisions or risks to resolve first:** Select the implementation language,
YAML library, schema-evolution policy, and canonicalization behavior for paths
that do not yet exist.

## 2. Root SQLite store and idempotent initialization

**Objective:** Make the root database the durable source for runtime state
without yet starting agents.

**Scope:** Create transactional schema and migrations for registered scopes,
sessions, workspace leases, tasks, and delivery attempts. Implement manual
initialization and inspection procedures; retain scope config and `AGENTS.md`
as their own canonical sources.

**Dependencies:** Slice 1.

**Acceptance criteria:**
- Initialization creates `<company>/.factory/factory.sqlite` and can be run
  repeatedly without changing a valid existing configuration.
- The schema enforces unique scope IDs, canonical registered paths, session
  IDs, and one live lease per canonical workspace.
- A failed mutation rolls back completely; a second supervisor/mutator is
  rejected or serialized.
- Database file permissions are restrictive and backups can be performed using
  a documented, consistent SQLite procedure.

**Decisions or risks to resolve first:** Choose migration tooling, lock strategy,
backup retention/restore verification, and which session states count as a live
workspace lease.

## 3. Explicit scope registry and hierarchy reconciliation

**Objective:** Register scopes deliberately and derive parentage safely from
paths and stable IDs.

**Scope:** Add scope records from validated configs; list, move, and reconcile
known scopes with a bounded supplied scan root. Detect config UUID/path drift
and Git worktree copies rather than treating every config file as a scope.

**Dependencies:** Slices 1–2.

**Acceptance criteria:**
- A root and registered descendants can be added and listed with stable UUID,
  name, canonical path, and nearest registered ancestor.
- Moving a registered directory and reconciling updates its path while retaining
  its UUID.
- Duplicate IDs and path collisions are reported without partially updating the
  registry.
- A copied `.factory/config.yaml` in a Git worktree is ignored as a duplicate
  scope definition; normal operation never recursively discovers scopes.
- Symlink escapes cannot make a non-descendant appear to be a child.

**Decisions or risks to resolve first:** Define the operator confirmation and
conflict policy for a moved scope versus a copied scope, and the supported Git
worktree detection mechanism.

## 4. Deterministic context compiler and generated-file guardrails

**Objective:** Produce exactly the text each harness will receive while keeping
human-maintained context safe.

**Scope:** Compile root-to-leaf `AGENTS.md` files with source headings, then the
agent definition and task prompt. Add read-only context inspection. If
compatibility generation is included, confine it to `.factory/generated/` and
require an ownership marker before regeneration.

**Dependencies:** Slice 3.

**Acceptance criteria:**
- The same scope and task produce byte-stable ordered context: company,
  ancestors, current scope, agent definition, task prompt.
- Context inspection identifies every source file and reports missing readable
  context files clearly.
- Existing `AGENTS.md`, `CLAUDE.md`, `.pi/`, and `.claude/` are never modified.
- An existing non-Factory target under generated output stops generation with a
  conflict instead of being overwritten.

**Decisions or risks to resolve first:** Set context size/error policy and decide
whether version 1 needs any generated compatibility file at all; manual context
injection is sufficient for the first rollout.

## 5. Manual harness adapter proof for Pi

**Objective:** Validate one real interactive harness boundary before automating
terminal control.

**Scope:** Specify and manually exercise the adapter lifecycle
`start/send/observe/interrupt/stop` for Pi in a scope workspace. Record pane
identity, command, generated context handoff, readiness evidence, completion
markers, question/permission signals, and final-result capture rules. Persist
only observations in the database; an operator performs terminal actions.

**Dependencies:** Slices 2 and 4.

**Acceptance criteria:**
- An operator can start Pi in a recorded existing workspace with compiled
  context, deliver one task containing its UUID, observe its state, and record
  a result or blocking reason.
- The session and task records remain useful after terminal detachment.
- Pi-specific logic is isolated behind the adapter contract; no Pi concepts are
  stored in scope, session, or task domain records.
- A missing executable or failed launch produces an actionable failed-start
  record and leaves no leaked lease.

**Decisions or risks to resolve first:** Confirm Herdr's stable programmatic or
operator-visible pane identifiers and Pi's reliable readiness/completion and
permission-prompt signals. These may require conservative human confirmation.

## 6. Session lifecycle and safe workspace leasing

**Objective:** Safely run one or more instances of an agent in existing allowed
workspaces.

**Scope:** Implement session state transitions, lease acquisition/release,
`max_sessions`, and workspace validation for the scope directory, an
unregistered descendant, and an existing same-repository Git worktree. Support
manual start, stop, status, and attach procedures through the Pi proof adapter.

**Dependencies:** Slices 2–5.

**Acceptance criteria:**
- Starting records `starting` before launch and reaches `running` only after
  observed readiness; stopping releases the lease only after the process is no
  longer usable.
- A second live session cannot lease the same canonical path, including aliases
  through symlinks.
- Registered descendant scopes are rejected as another scope's workspace.
- Existing unregistered descendants and validated same-repository worktrees are
  accepted; nonexistent paths and foreign-repository worktrees are rejected.
- `max_sessions` prevents further starts without changing existing sessions.

**Decisions or risks to resolve first:** Define stale-lease/operator recovery
rules and the exact repository identity test (for example, common Git dir plus
canonical repository root).

## 7. Durable task queue and single-session delivery

**Objective:** Deliver one durable task safely to an idle session, with manual
confirmation where terminal observation is ambiguous.

**Scope:** Create/list/show/cancel task records, task-to-session assignment,
delivery-attempt journal, allowed state transitions, result summaries/artifact
paths, and workspace/session targeting. Use at-most-once delivery; retain a
manual operator handoff option until PTY input is automated.

**Dependencies:** Slice 6.

**Acceptance criteria:**
- Creating a task commits `queued` before any prompt is entered into a terminal.
- An untargeted task selects only an idle matching session; a targeted workspace
  reuses its session or starts one only within `max_sessions`.
- Each session has at most one running task, and busy agents leave excess tasks
  queued.
- The delivery attempt is recorded before input; every prompt contains the task
  UUID.
- Completion stores `done`/`failed` plus a result summary or artifact paths;
  questions and permissions become `blocked` with the specified reason.
- Cancelling queued work is immediate; cancelling running work requests
  cooperative interruption rather than force-stopping the session.

**Decisions or risks to resolve first:** Define task result size/storage limits,
operator authority for status changes, and how the adapter distinguishes a
question from normal harness output.

## 8. Delegation policy and multi-session manual rollout

**Objective:** Enforce the one version-1 trust rule while proving useful
parallel work.

**Scope:** Carry authenticated sender identity from a human or recorded sending
session; check target scope ancestry before queueing. Exercise two independent
sessions of one agent in distinct workspaces and establish operator runbooks for
human targeting, parent-to-child delegation, task review, and terminal attach.

**Dependencies:** Slices 3, 6–7.

**Acceptance criteria:**
- A human can queue work to any registered scope.
- A parent scope can queue work to a registered descendant.
- A session is rejected when targeting its own ancestor, a sibling, or any
  non-descendant; rejection leaves no task or delivery record.
- Two sessions for one scope can run separate tasks concurrently without shared
  workspace leases or prompt multiplexing.
- The operator can inspect session, task, lease, and compiled-context records
  without relying on terminal scrollback.

**Decisions or risks to resolve first:** Choose how sender identity is bound to
an interactive session and clearly document that this is cooperative policy,
not security isolation.

## 9. Reconciliation, restart drills, and failure handling

**Objective:** Make recovery conservative and auditable before unattended
operation.

**Scope:** Reconcile database records with Herdr panes and harness observations;
restore known sessions and leases where possible; provide explicit review,
resume, and replacement-task actions. Run failure drills for supervisor,
Herdr, machine, harness, and harness-change events.

**Dependencies:** Slices 5–8.

**Acceptance criteria:**
- After a supervisor restart, known panes are reconnected where possible and no
  task prompt is silently sent a second time.
- Ambiguous or unrecoverable running work becomes `blocked: interrupted` with
  a recorded delivery history and clear human next action.
- After Herdr or machine restart, sessions are recreated only in recorded,
  existing workspaces; queued tasks remain queued.
- A harness crash marks its session failed and its active task blocked or failed
  according to documented evidence.
- Changing a harness stops old sessions, retains queued tasks, and requires
  review of running work.

**Decisions or risks to resolve first:** Determine what Herdr can reliably
reconnect to after each restart class and whether task resume means an explicit
new delivery or a replacement task.

## 10. CLI automation and Claude Code parity

**Objective:** Replace validated manual runbooks with an idempotent Factory CLI
and add the second adapter without changing domain behavior.

**Scope:** Implement the specified `init`, supervisor, scope, agent, task, and
context command surface over prior services. Automate only adapter actions that
have passed manual proof; add Claude Code using the same adapter contract and
repeat the core lifecycle, task, and recovery tests.

**Dependencies:** Slices 1–9.

**Acceptance criteria:**
- Every listed version-1 command maps to existing domain operations and is safe
  to retry after an interrupted invocation.
- CLI output provides stable IDs, state, actionable errors, and exact context
  inspection; attach reaches the native Pi or Claude Code terminal.
- Pi and Claude Code pass the same contract tests for start, send, observe,
  interrupt, stop, failed start, and one-task-per-session behavior.
- Automated tests cover configuration, paths, transactions, leases, task state
  transitions, delegation, and reconciliation; integration tests use isolated
  fixture repositories and existing worktrees only.
- No browser app, scheduler, workflow engine, external database, worktree
  creation/merge/cleanup, or hard-isolation feature is introduced.

**Decisions or risks to resolve first:** Confirm Herdr automation APIs and test
fixtures for both harnesses. If either adapter cannot observe safely, retain its
manual-confirmation mode rather than weakening at-most-once delivery.

## 11. Shared task audit and cron dispatcher

**Objective:** Extend the root task store with durable task runs, decisions,
results, and recurring task creation without introducing a separate workflow
engine.

**Scope:** Add transactional SQLite tables for task templates, task runs,
append-only events, decisions, artifacts, and cron schedules. Expose the same
task tool to every Pi agent. A minute-level dispatcher creates idempotent cron
runs and attempts delivery only to an assigned idle agent; otherwise work stays
queued for central assignment.

**Dependencies:** Slices 2, 7, and 10.

**Acceptance criteria:**
- An agent can create, inspect, assign, progress, decide, block, and close a
  task run with durable IDs and audit history.
- A completed run stores an outcome; decisions store rationale and alternatives.
- One cron schedule creates no more than one run for the same local minute,
  including after dispatcher restarts.
- Unassigned, busy, and stopped target agents do not lose queued work.
- The scheduler never stores secrets or copies sensitive source content into
  SQLite records.

**Decisions or risks to resolve first:** Delivery is cooperative until the
Factory supervisor owns all session lifecycle and exact at-most-once handoff.
`launchd` installation remains an explicit macOS operation and needs a restart
drill with logs before being treated as unattended production automation.
