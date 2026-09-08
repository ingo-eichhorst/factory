# Version-1 Implementation Backlog

## Delivery approach

Build and exercise each slice first with documented operator commands and
fixtures. Keep domain state and boundary contracts independent of the command
line so the Factory CLI can replace manual procedures without changing the
model. No slice may make external changes (create worktrees, merge branches,
or alter harness-owned files) as part of normal operation.

Two scope notes from ADR 0010, which reconciles this backlog with the project
ADRs. Where the design baseline and the ADRs use different words for the same
thing — supervisor and daemon, harness adapter and supervised plugin, permanent
and thread agent — they mean the same component, and no slice implements two of
them. Threads and durable messages are target state and have no slice here: the
task remains the only durable unit of work exchanged in version 1.

Settled on 2026-09-08 by ADR 0014: **a long-running daemon owns all mutations,
and the CLI is a client.** This confirms ADR 0002 and closes ADR 0010's last
open structural question. Slices 1–4 were identical under either answer — a
claim ADR 0012 checked rather than assumed — and were implemented and committed
before the decision was taken, without any of them having to guess.

Slices 5 onward describe a daemon and its clients. Two consequences reach
existing entries: the single-supervisor `flock` ADR 0012 deferred becomes real,
and `factory doctor` must keep working when the daemon is down, since that is
precisely the condition an operator runs it to diagnose.

## 1. Repository contract and configuration validation

**Objective:** Establish the safe, inspectable local contract for a company root
and a registered scope.

**Scope:** Define the version-1 YAML schema for the instance's
`.factory/config.yaml`, defaults (`max_sessions: 1`), UUID and harness
validation, canonical-path utilities, and human-readable validation errors.
Document a manual registration fixture: an instance configuration registering
two scopes, and the `AGENTS.md` files those scopes provide.

**Reworked by ADR 0015.** One file per scope became one instance file listing
every scope. The agent schema, the validation rules, the harness table, and the
whole diagnostic corpus survive unchanged; what moved is the document shape, and
duplicate-ID detection becomes an intra-file check rather than a cross-file one —
which makes it stronger, since it now runs on every load rather than only when a
caller remembers to compare two configs.

**Dependencies:** None.

**Acceptance criteria:**
- A valid instance configuration loads into a typed model carrying the instance
  and its registered scopes, each with one or more agents. Both the `agent:`
  shorthand and the `agents:` list are accepted within a scope entry; a scope
  setting both fails, naming the file, the scope, and both keys.
- A scope entry carries a `path`, and optionally a `git` reference. `git` marks
  the project as its own repository; its absence means the project's files live
  in the instance's repository. Neither form causes Factory to write anything at
  the scope's path.
- Two scopes may not share a `path` or an `id`. Both are reported with the two
  entries identified, in one pass over the single file.
- Two agents in one scope may not share a name, because the name addresses an
  agent in `task send`. A duplicate fails with both definitions identified.
- `lifetime` is optional and defaults to `permanent`; only `permanent` and
  `temporary` are accepted. Any other value fails with the supported set.
- `max_sessions` is per agent entry and defaults to `1`. Version 1 has no
  scope-level aggregate cap: a scope's session bound is the sum of its agents'
  limits. The bound applies identically to both lifetimes, where for a
  `temporary` agent it caps concurrently live instances.
- Missing or malformed fields, unsupported versions, invalid UUIDs, duplicate
  IDs, and unsupported harnesses fail with the file and corrective action.
- `factory-paths` resolves relative paths, `..`, and symlinked paths to
  canonical absolute paths, and compares them by `(st_dev, st_ino)` rather than
  by string. **Configuration parsing does not canonicalize.** A scope's `path`
  is stored as written, and the duplicate-path check above is textual. Reading a
  configuration file must not depend on the filesystem the way canonicalization
  does — a config naming a path that does not exist yet is a registration
  problem for Slice 3 to report, not a parse failure that makes the file
  unreadable. The two capabilities ship in this slice; joining them is Slice 3's
  work.
- Validation writes nothing at all, and more generally no command writes outside
  a `.factory/` directory (design §4). This subsumes the older rule that
  `AGENTS.md`, `CLAUDE.md`, `.pi/`, and `.claude/` are never overwritten, and
  unlike that list it cannot be outgrown by a harness added later.

**Decisions or risks to resolve first:** None remain; Slice 1 is ready to start.
The implementation language is settled by ADR 0002, with ADR 0008 recording why
Rust and what it costs. ADR 0009 settles the remaining three: `serde-saphyr` as
the YAML library, the schema-evolution policy (Factory owns `version`, `scope`,
`agent`/`agents`; unknown top-level keys are preserved and ignored; unknown
fields inside Factory-owned mappings are rejected), and path identity
(canonical path is stored identity, `(st_dev, st_ino)` is the runtime aliasing
check; only `init --root` may name a non-existent path).

**New open item from ADR 0009:** whether Factory should ever expand `${HOME}` /
`${REPO_ROOT}` in configuration values. Version 1 does not. The existing
expansion belongs to `ensure_assistant_agents.py`, not to Factory; adding it
would need its own specification.

**Open item, decided provisionally during implementation — the accepted harness
set.** Design §2.2 lists the initial harnesses as exactly `pi` and
`claude-code`. Surveying the seven live configs found two that a literal reading
rejects: `model-lab` sets `opencode`, and `awesome-herdr` sets `claude`. ADR 0009
surveyed the same files and recorded the `runtime:` block but missed this.

Version 1 accepts `pi`, `claude-code`, and `opencode`, and rejects `claude` with
"did you mean `claude-code`?" — `awesome-herdr` runs Claude Code, so that one is
a typo, whereas `opencode` is a real harness in real use and §2.2's word
"initial" anticipates the list growing. Accepting a name here is not a claim that
an adapter exists for it; Slice 1 has no adapters, and Slice 5 and Slice 10
answer whether Factory can drive a harness.

The whole decision lives in `HARNESS_TABLE` in
`crates/factory-config/src/harness.rs`, so narrowing it back to §2.2 is deleting
one row. It is recorded here because it was taken without the user present and
amends a design section.

**Measured, not assumed.** The implemented loader was run against all seven live
`.factory/config.yaml` files on 2026-09-08. Six load; `awesome-herdr` fails with
the did-you-mean, which is the intended outcome for a typo. `validate_unique_ids`
over the six reports no duplicate scope IDs. Under a literal reading of §2.2 it
would be two failures rather than one, and the second — `model-lab` — would be a
correctly configured scope that Factory simply could not read.

That run also confirmed the load-bearing half of ADR 0009 rule 2 against the
real files rather than a fixture copy: the `assistant` config carries a
top-level `agent:` **and** an indented `agents:` inside a `runtime:` block owned
by `ensure_assistant_agents.py`, and it loads as exactly one agent. A stricter
top-level policy would make that scope unloadable today.

**Open item found by that run — the `assistant` scope under-declares itself.**
Factory's schema sees one agent, `Assistant`, with `max_sessions: 2`. What
actually runs is two *different* agents: `assistant` and `assistant-chat`, with
different models, prompts, and extensions, declared in the `runtime:` block that
belongs to another tool. Design §2.2 names this very scope as the motivating
example for multi-agent scopes — "one agent watching an iMessage channel and a
separate agent answering internal requests, as in the `assistant` scope" — so
the design and the reality agree, and only the configuration disagrees with
both.

The consequences land in later slices, not in Slice 1. `max_sessions` is per
agent (Slice 6), so Factory would permit two sessions of one agent where reality
is two agents that are not interchangeable. Tasks address an agent by name
(Slices 7–8), so `assistant-chat` is unaddressable as written. Nothing breaks
today because no adapter reads this yet.

The fix is a configuration migration — moving the two agents into Factory's
`agents:` list — which overlaps `ensure_assistant_agents.py`'s ownership of the
`runtime:` block and therefore needs its own task with a migration and rollback
plan, like the task store. It is deliberately not folded into a slice here.

**Resolved 2026-09-08.** The registry's `assistant` entry now uses `agents:`
to name both agents the `runtime:` block actually starts — `assistant` and
`assistant-chat`, harness `pi`, `lifetime: permanent`, `max_sessions: 1`
each — instead of one agent with `max_sessions: 2`. `crates/factory-config`
already accepted an `agents:` list before this fix; only the registry entry
was wrong, so nothing in the crate changed.

The two agents' real working directories differ — `assistant` and
`assistant/chat` — and the scope declares one `path:`. That is not a gap in
the schema and needs no follow-up: design §2.3 makes the workspace a property
of a *session*, not of an agent (`start(scope, session_id, workspace,
generated_context)`), and §2.3 lists an existing unregistered descendant as a
permitted workspace, which is exactly what `assistant/chat` is. A per-agent
`cwd:` in the registry would be a second place to say where a session runs,
disagreeing with the one the start call already carries. Slice 6 validates and
leases that path; the review of this fix confirmed the reading against the
design rather than assuming it.
The migration and rollback plan this item asked for, sized to the change: the
edit is one entry in one already-committed file (`a49bd20`), so rollback is
`git checkout -- .factory/config.yaml`. No consumer needs reconciling —
`ensure_assistant_agents.py` reads only the `runtime:` block in its own,
untouched file; `factory_tasks.py` uses the registry's path as an ancestor
marker without parsing agents; `factory-config` is the only parser of this
entry and has no other caller yet. Covered by
`crates/factory-config/tests/live_registry.rs`, which loads this file
directly (skipping, not failing, where the company root is not checked out —
`projects/factory`'s own subtree remote, see its `git:` comment above) and
fails on the old shape.
Also stale as of this fix: Slice 10's acceptance criteria and risk analysis
below still cite `assistant` as a live example of one agent configured with
`max_sessions: 2` in one workspace (search for "like `assistant`"). That
example no longer matches the registry — flagged here rather than corrected
there, since re-deriving Slice 10's concrete example is that slice's work, not
this fix's.

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

**Decisions or risks to resolve first:** None remain; ADR 0012 settles all four.
`rusqlite` with `bundled` as the driver and `rusqlite_migration` tracking schema
state in `PRAGMA user_version`; WAL with `busy_timeout` and `BEGIN IMMEDIATE`,
which *serializes* concurrent mutators rather than rejecting them; `VACUUM INTO`
for backups with a restore drill that opens the snapshot read-write, retention
left to the operator; and `starting`, `running`, and `disconnected` holding a
workspace lease while `stopped` and `failed` release it.

Two consequences reach other slices. `disconnected` holding its lease makes a
stale-lease recovery action **mandatory** in Slice 9 rather than optional —
otherwise a session that never returns holds its workspace forever. And the
single-supervisor question turns out not to live here: serialization is the same
answer under a daemon or a process per invocation, so the claim that slices 1–4
are identical either way is now checked rather than assumed. ADR 0014 has since
chosen the daemon, and the exclusive `flock` deferred here now has an owner.

## 3. Explicit scope registry and hierarchy reconciliation

**Objective:** Register scopes deliberately and derive parentage safely from
paths and stable IDs.

**Scope:** Project the instance configuration's `scopes:` list into scope
records; list, move, and reconcile them. Detect drift between a recorded path
and the filesystem.

**This slice is materially smaller than it was.** ADR 0015 made a scope a
registry entry rather than a discoverable file, which deleted three of its
problems outright: a bounded scan root, Git-worktree copy detection, and the
fixture-exclusion mechanism this entry used to require. None of them describe
anything now, because Factory no longer searches for scopes at all. What remains
is path canonicalization, ancestry, and drift.

**Dependencies:** Slices 1–2.

**Acceptance criteria:**
- The instance and its registered scopes can be listed with stable UUID, name,
  canonical path, optional `git` reference, and nearest registered ancestor.
- Moving a registered directory and updating its `path` retains its UUID; the
  UUID is identity and the path is a location.
- Duplicate IDs and path collisions are reported without partially updating the
  registry.
- `scope reconcile` reports a recorded path that no longer exists, or one whose
  identity has changed, and repairs nothing on its own.
- **Gap found during implementation, still open:** the six drift kinds all key
  on the path. A human who only renames a scope or adds a `git` reference
  changes nothing about where it is, so reconcile reports a clean registry while
  the projection holds the old `name` and `git`. This is silent staleness, and
  it is the failure mode ADR 0016's rebuild rule exists to bound — a rebuild
  fixes it, but nothing tells an operator a rebuild is needed. Either add a
  `FieldsChanged` drift kind, or have reconcile compare every projected column
  and not just the path.
- Symlink escapes cannot make a non-descendant appear to be a child.
- Registration verifies that the scope has a readable `AGENTS.md`. ADR 0013
  makes Slice 4 fail hard on a context source it cannot read, so the absence
  must be caught when a scope is registered rather than when an agent starts.
- The registry supplies **canonical** paths to the context compiler. Slice 4
  prints the source path into the compiled text and uses what it is given
  verbatim, by design — it is a pure function and does not explore the
  filesystem. So byte-stability is guaranteed per input, and it is this slice
  that must guarantee the input. A relative path reaching the compiler would
  produce different bytes for the same scope depending on the working
  directory, which is a §2.5 violation created here and only observed there.
**Decisions or risks to resolve first:** Define the operator confirmation policy
for a moved scope. The fixture-exclusion question is closed rather than answered:
a fixture is now just a file that nothing reads unless a test passes it in, so
there is no scan to exclude it from. The `fixture: true` marker may stay as
human-readable documentation, but it no longer stands in for a safeguard.

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
- Compilation reads `AGENTS.md` and never writes outside `.factory/` (design §4),
  so existing `AGENTS.md`, `CLAUDE.md`, `.pi/`, and `.claude/` cannot be modified
  by construction rather than by a check that must be remembered.
- A context source that cannot be read stops compilation, naming the file, the
  scope that expected it, and the action. It is never replaced by an empty
  section, because a silently skipped `AGENTS.md` produces an agent running with
  less instruction than its operator believes it has — mandates about secrets
  and approval gates quietly absent, with no symptom until the agent does
  something it should have been told not to do.
- `factory context show` reports each source, whether it was readable, and its
  byte count, so an operator can see the composition and not only the result.
- Compilation follows no `[[links]]` out of `AGENTS.md`. Context is the
  `AGENTS.md` chain, the agent definition, and the task prompt, and nothing
  else.
- ~~An existing non-Factory target under generated output stops generation with
  a conflict instead of being overwritten.~~ **Deferred:** version 1 generates
  no compatibility file, so this criterion is vacuous. It is struck rather than
  deleted, because the rule in design §4 still applies the moment an adapter
  needs one.

**Decisions or risks to resolve first:** None remain; ADR 0013 settles both.
Version 1 generates no compatibility file, which removes the only writing path
in this slice and makes the design §4 write rule hold by construction. There is
no size limit — any constant would be invented, since the real bound is the
harness's context window and Factory does not know which model a harness runs —
but the size is always reported. A missing context source is a hard failure.
Version 1 follows no knowledge links, which answers the §12.4 traversal question
for now: an unbounded walk is neither bounded nor byte-stable, and a
depth-bounded one is still unstable, since adding one link to an unrelated note
would silently change what every agent in that subtree receives.

## 5. Manual harness adapter proof for Pi

**Objective:** Validate one real interactive harness boundary before automating
terminal control.

**Scope:** Specify and manually exercise the adapter lifecycle
`start/send/observe/interrupt/stop` for Pi in a scope workspace. Record pane
identity, command, generated context handoff, readiness evidence, completion
markers, question/permission signals, and final-result capture rules. Persist
only observations in the database; an operator performs terminal actions.

`observe()` takes its state from **Herdr**, not Irrlicht and not the PTY
(ADR 0017, which supersedes ADR 0011 for Pi only). Herdr does not infer Pi's
state: Pi loads a Herdr-installed extension that reports lifecycle events over a
socket, which `herdr agent explain` confirms as
`screen_detection_skip_reason: full_lifecycle_hook_authority`. Irrlicht parses a
transcript to infer state; Herdr is told it by the harness.

Herdr's vocabulary is `idle | working | blocked | done | unknown`, which already
carries the question-and-permission signal this slice lists as a risk. Irrlicht
remains the source for `claude-code` in Slice 10, and remains the source of
token and cost metrics that Herdr does not report.

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
- A Pi session started inside a Herdr pane is correlated through Herdr's
  pane-to-transcript mapping, not through Irrlicht's pane ID, which is absent
  for every Pi session measured. Where no transcript path is available on either
  side, the adapter records **no** observation and falls back to manual
  confirmation rather than inferring state.
- Two Pi sessions of one agent in the same workspace are never confused. This is
  the criterion that a working-directory join silently fails, and the `assistant`
  scope is configured for exactly that with `max_sessions: 2`. Test it with two
  live sessions in one directory, not one.
- With Irrlicht stopped, the adapter still completes the lifecycle: for Pi it
  loses only enrichment, since state comes from Herdr. With **Herdr** stopped it
  falls back to manual confirmation, and no task is lost or duplicated.
- A Pi session started without the Herdr state extension is recorded as
  **observation-degraded** and gets manual confirmation. The adapter must not
  accept screen-detected state as if it were hook-reported: it verifies
  `screen_detection_skip_reason: full_lifecycle_hook_authority` and treats its
  absence as no observation.
- `idle` never closes a task, and `done` never closes a task. `idle` means the
  harness waits for input, which is equally true before delivery and after
  completion; `done` is Herdr's word for a finished turn, and a task may span
  many turns. Completion is recorded from the agent's reported result.

**Decisions or risks to resolve first:** ADR 0011's open item 1 is now answered,
by measurement against the live machine on 2026-09-08, and the answer changes
this slice.

Irrlicht detects every Herdr-launched session — ten sessions matching Herdr's
ten panes, each with a state. **Correlation is what fails, and only for Pi.**
Irrlicht resolves the Herdr pane for 4/4 `claude-code`, 2/2 `codex`, and 1/1
`opencode` sessions, but **0 of 3 `pi` sessions**, and a transcript path for
only one of the three. Herdr reports a transcript path for all three.

So the join key ADR 0011 decision 5 assumed — workspace path, plus a session
UUID "where present" — is not available for Pi. Working directory is not unique
either: two directories on this machine host several sessions each, and a scope
with `max_sessions: 2` like `assistant` makes that structural rather than
incidental.

- For **Pi**, take the join from Herdr: `herdr agent list` gives per pane the
  transcript path (hence the session UUID), `agent_status`, and
  `interactive_ready`. Consume Irrlicht's richer state only after matching a
  session that way. **Do not join Pi sessions on working directory** — with two
  sessions in one workspace it would attribute one session's state to the other,
  which is worse than no observation because it looks like an answer.
- For **`claude-code`** (Slice 10) the Irrlicht pane ID is present and the
  original design works unchanged.

What remains open is Herdr pane-identifier stability across a Herdr restart,
which the Slice 9 drills must establish.

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
  through symlinks and through case variants on a case-insensitive volume
  (the production default). Per ADR 0009 and its 2026-09-08 correction, this
  needs both halves: `(st_dev, st_ino)` for "are these the same directory now",
  and re-canonicalizing a stored path before comparing it rather than
  string-matching what the database returned.
- The case-variant test must span time — resolve, rename changing only case,
  resolve again — because Rust's `canonicalize` normalises case, so a test that
  resolves two spellings at one instant passes trivially and proves nothing.
  A lease test that cannot fail is worse than no lease test, because it reports
  confidence it has not earned.
- Registered descendant scopes are rejected as another scope's workspace.
- Existing unregistered descendants and validated same-repository worktrees are
  accepted; nonexistent paths and foreign-repository worktrees are rejected.
- `max_sessions` prevents further starts without changing existing sessions, and
  is evaluated per agent rather than per scope.
- A `permanent` agent's session survives between tasks and is restarted after a
  crash. A `temporary` agent's session is created when a task is delivered to it
  and torn down once that task reaches `done`, `failed`, or `cancelled`.
  `blocked` is not terminal: a temporary agent awaiting clarification or
  permission keeps its session and its lease.
- When a temporary agent's session dies while its task is not in a terminal
  state — harness crash, Herdr restart, machine restart — the session stops
  holding anything, its workspace lease is released, and the task becomes
  `blocked: interrupted`. Factory never restarts the agent or redelivers the
  prompt automatically: after an ambiguous failure it is unknown whether the
  first delivery already had external effect. A human creates a replacement
  task, which gets a fresh temporary agent and session.

  This said "the session record is removed", which reads as `DELETE FROM
  sessions` and is not what the implementation does. Corrected on 2026-09-08
  after building it, for two reasons found in that order. Measured first:
  `workspace_leases.session_id` is `NOT NULL` with no `ON DELETE`, so under
  `PRAGMA foreign_keys = ON` the delete fails outright while a lease row
  references the session — and `workspace_leases` is a durable audit journal,
  so deleting *it* first to make room is the wrong direction. The stronger
  reason came second: a deleted row erases the fact that a session died
  mid-task, which is exactly the evidence §12.5's operating data needs to
  report session mortality at all. A session that dies is moved to `failed`,
  which releases the lease by ADR 0012's rule and leaves the death on the
  record. Nothing counts it afterwards: `max_sessions` counts sessions that
  hold a lease, so a `failed` row occupies no slot.

**Decisions or risks to resolve first:** Define the exact repository identity
test (for example, common Git dir plus canonical repository root). The
lease-holding states are settled by ADR 0012 — `starting`, `running`, and
`disconnected` hold; `stopped` and `failed` release — and because
`disconnected` holds, stale-lease recovery is no longer a rule to define at
leisure but a required operator action in Slice 9.

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
sessions of a single agent in distinct workspaces, and separately two agents of
one scope running concurrently, then establish operator runbooks for human
targeting, parent-to-child delegation, task review, and terminal attach. The
delegation rule is per scope, so agents sharing a scope share its permissions;
targeting an individual agent selects a recipient, it does not grant one.

**Dependencies:** Slices 3, 6–7.

**Acceptance criteria:**
- A human can queue work to any registered scope.
- A parent scope can queue work to a registered descendant, and a scope can
  queue work to a sibling sharing its parent.
- A session is rejected when targeting its own ancestor, itself, or a scope that
  is neither a descendant nor a sibling — a nephew or cousin included, which
  must be reached through its parent. Rejection leaves no task or delivery
  record.
- Every task carries the ordered delegation chain of scopes it has passed
  through, and targeting a scope already in that chain is rejected. A two-step
  cycle (`A → B → A`) and a longer one are both refused, so peer delegation
  cannot loop indefinitely.
- The chain is recorded durably with the task, so a delegation loop is
  reconstructable after a restart rather than only detectable while running.
- Two sessions for one scope can run separate tasks concurrently without shared
  workspace leases or prompt multiplexing.
- The operator can inspect session, task, lease, and compiled-context records
  without relying on terminal scrollback.

**Schema gap found during slice 7:** `tasks` records `target_scope_id` but no
agent. `agent_name` exists only on `sessions`. Design §2.4 says "a task sent
only to an agent is assigned to any idle session" and this slice's own scope
line says targeting an individual agent selects a recipient — neither is
expressible today. It is not hypothetical: the `assistant` scope runs two
agents, `assistant` and `assistant-chat`, so a task aimed at that scope cannot
say which one it is for. Slice 7's `assign` works around it by taking
`agent_name` as a call parameter, which means the choice is made at assignment
time and never recorded, so after a restart the task row cannot say which agent
it was meant for. Resolving this needs a migration adding `target_agent_name`
to `tasks`, and a rule for what an absent value means — any agent of the scope,
or a rejected task.

**Decisions or risks to resolve first:** Choose how sender identity is bound to
an interactive session and clearly document that this is cooperative policy,
not security isolation.

**Resolved during slice 8.** Sender identity is bound as an *API shape*, not a
column: the agent-facing entry point takes a session id and resolves
`sessions.scope_id` from that row, so a caller can never name its own sender
scope. The six decisions this slice was built on — including why it needed no
migration, why two root scopes are not siblings, and why the Rust check is the
rule's home while `UNIQUE (task_id, scope_id)` is only its backstop — are
written into `crates/factory-delegation/src/lib.rs`'s crate documentation
rather than repeated here, because that is where the next person to change the
rule will be standing. Ancestry lives in `factory_registry::kinship`, its one
home; nothing else in the workspace walks `parent_id`.

**Coverage gap found during slice 8, in code slice 7 delivered.** `assign`
filtered idle sessions by `agent_name`, and no test covered it: deleting the
filter outright left all 84 `factory-task` tests green. This slice's own "two agents
of one scope" demonstration passed for the wrong reason — the two sessions
happened to be tried in `rowid` order, so a broken filter still chose
correctly. `untargeted_assign_never_picks_another_agents_idle_session` now
tests the rule head-on, and the demonstration was reordered so the ordering can
no longer rescue it.

**Left open by slice 8:** `target_agent_name`. None of this slice's acceptance
criteria need it — the delegation rule is per scope, and `assign` already takes
`agent_name` as a call parameter — so adding the column here would have mixed a
second, unrelated question into the delegation chain. The gap recorded above
stands exactly as written, and the `assistant` scope's two agents remain
undecidable from a task row alone.

## 9. Reconciliation, restart drills, and failure handling

**Objective:** Make recovery conservative and auditable before unattended
operation.

**Scope:** Reconcile database records with Herdr panes and harness observations;
restore known sessions and leases where possible; provide explicit review,
resume, and replacement-task actions. Run failure drills for supervisor,
Herdr, machine, harness, and harness-change events.

**Dependencies:** Slices 5–8.

ADR 0019 settles the half of this that needs no live evidence: what a restored
database may claim on its own. After a restore every session is `disconnected`
with its lease held, and every task with a delivery attempt against it is
`blocked: interrupted`. This slice starts from that known claim and owns the
opposite direction — what live Herdr and adapter evidence is allowed to change
about it.

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

**Resume has no mechanism yet, found during slice 7.** `deliver` refuses any
task that already carries a `delivery_attempts` row, which is the correct
reading of design §5's "does not automatically resend a possibly delivered
prompt". The consequence is that a `blocked → queued` task can be re-assigned
but never re-delivered: it can only be replaced. Design §5 offers resume as the
other way on, and a human authorising one further delivery is not the automatic
resend §5 forbids — so resume needs a way to say so, and that permission must
be recorded with the task rather than passed as a call argument, or a restart
cannot tell an authorised second delivery from an accidental one.

**Decisions or risks to resolve first:** Determine what Herdr can reliably
reconnect to after each restart class and whether task resume means an explicit
new delivery or a replacement task.

## 10. CLI automation and Claude Code parity

**Objective:** Replace validated manual runbooks with an idempotent Factory CLI
and add the second adapter without changing domain behavior.

**Scope:** Implement the specified `init`, supervisor, scope, agent, task,
context, and `doctor` command surface over prior services. Automate only adapter
actions that have passed manual proof; add Claude Code using the same adapter
contract and repeat the core lifecycle, task, and recovery tests.

`factory doctor` is read-only diagnosis and belongs here because it inspects
what slices 1–9 built. It reports and never repairs: recovery keeps the explicit
review, resume, and replacement actions of slice 9, so there are not two ways to
change the same state.

The read-only pass needs a door that does not exist yet. `Store::open_at` is the
only way into the database and it migrates unconditionally, so today's doctor
would change the schema before reporting it. ADR 0018 decision 1 provides
`Store::open_read_only` for exactly this, and decisions 2 and 5 of ADRs 0018 and
0019 add two things to doctor's report: whether migrations are pending, and the
count, size, and age span of `.factory/backups/`.

**Dependencies:** Slices 1–9.

**Acceptance criteria:**
- `factory doctor` reports, in one read-only pass and without mutating anything:
  configuration validity for every registered scope, database integrity and
  schema version, registry drift (a registered path that no longer exists),
  leases held with no live session, database sessions against actual Herdr
  panes, and whether the scheduler job is loaded and when it last fired. It
  exits non-zero when it finds something, so it is usable from a check.
- The scheduler check closes a real gap: today nothing would reveal that a cron
  schedule has not fired for days.
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
results, recurring task creation, and the version-1 production-station hooks of
design section 12, without introducing a separate workflow engine.

**Scope:** Add transactional SQLite tables for task templates, task runs,
append-only events, decisions, artifacts, and cron schedules. Expose the same
task tool to every Pi agent. A minute-level dispatcher creates idempotent cron
runs and attempts delivery only to an assigned idle agent; otherwise work stays
queued for central assignment. Add `factory schedule create|list|enable|disable`
as its own CLI command group, distinct from `factory task send`, mirroring the
`schedule_create`/`schedule_list`/`schedule_enable`/`schedule_disable` agent
tools.

Carry the production-station hooks in the same tables and the same migration:
acceptance criteria and a version on a task template; the executed template
version, an optional reworked-run reference with the finding that caused it, and
adapter-reported cost metrics on a task run; and a `verification` event type in
the append-only log. These are durable fields and event types only. This slice
does not automate an inspection gate, does not decide rework, and adds no
projection, report, or command over the recorded figures.

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
- `factory schedule create` registers a durable cron rule against a task
  template without creating a run itself; `list` shows every schedule with its
  active state and next/last run; `enable`/`disable` toggle a schedule without
  deleting its history or affecting already-created runs.
- A task template stores acceptance criteria and a version; every run records
  the template version it executed, and that record does not change when the
  template is later revised.
- A verification verdict is stored as an append-only event attributed to a
  session other than the one that produced the result, and never replaces or
  overwrites the worker's own completion record.
- A run can reference the run it reworks together with the finding that caused
  the rework, without modifying the referenced run.
- A run stores model identifier, token counts, and duration when its adapter
  reports them, and remains valid when the adapter reports none of them.

**Decisions or risks to resolve first:** Delivery is cooperative until the
Factory supervisor owns all session lifecycle and exact at-most-once handoff.
`launchd` installation remains an explicit macOS operation and needs a restart
drill with logs before being treated as unattended production automation.
Decide whether a verification verdict may transition a run's state in version 1
or may only annotate it, and confirm which harness adapters can report token
counts at all before treating cost metrics as a required field. Design section 6
forbids a scope from targeting itself, so decide who may legally create a
verification run: a human, the parent scope, a sibling scope, or an explicit
delegation exception. Allowing siblings does not settle this on its own, since
the inspecting session would still have to live outside the scope under
inspection.

## 12. Agent discovery and durable writing

**Objective:** Let an agent find out who else exists and who it may delegate to,
and give it one enforced path for writing shared knowledge and scope memory.

**Scope:** Implement `factory agent list`, `factory knowledge write|list|show`,
and `factory memory add|list` per design section 7.

`agent list` returns the registered scopes and their agents with harness,
lifetime, and availability, and marks which the calling scope may target under
the section 6 rule. It answers the permission question directly rather than
returning the tree and leaving each agent to apply the rule, so the rule cannot
drift between harnesses. Availability comes from the session observation of
ADR 0011; where no observation is available the entry reports availability as
unknown rather than guessing idle.

`knowledge write` creates or updates a note in the company root's
`.factory/knowledge/`. `memory add` writes an entry to the calling scope's
`.factory/memory/`. Both live under `.factory/` because of the design section 4
rule that no Factory command writes outside it. The two are separate because the
material differs: knowledge is shared and sourced, memory is scope-local and
unsourced.

Knowledge is a note graph in the Obsidian convention, not a folder tree: one
Markdown file per note, filename as stable identifier, `[[note-name]]` links
between them. Links are canonical and live in the files; backlinks and the index
are projections rebuilt by rereading the notes, which is what design section 4
requires of derived indexes. No index file is maintained by hand, and no index
service, embedding, or retrieval engine is built, so the section 8 deferral of
knowledge databases is untouched.

**Dependencies:** Slices 3, 8, and 10.

**Acceptance criteria:**
- `factory agent list` run from a scope marks exactly the descendants and
  siblings as targetable, and marks ancestors, itself, nephews, and cousins as
  not targetable, matching the slice 8 rule with no second implementation.
- An agent that is registered but has no live session is listed with its
  availability stated, and is never silently omitted.
- No command in the whole CLI writes, creates, or deletes a file outside a
  `.factory/` directory. This is asserted once against every mutating command,
  not per command, so a command added later cannot quietly escape it.
- `knowledge write` refuses a note whose frontmatter lacks a title, status,
  updated date, or at least one source, naming what is missing.
- `knowledge write` refuses any source path under `data/secrets/`, and the
  refusal names the rule rather than the file contents.
- A note write is atomic: after a failure part-way, the note is either fully
  written or unchanged, never truncated.
- `factory knowledge list` derives the index and the backlinks by rereading the
  notes, holds no separate link table, and produces identical output when run
  twice against unchanged notes.
- A `[[link]]` to a note that does not exist is accepted on write and reported by
  `list` as unresolved. It marks a gap to fill and is never an error.
- Deleting `.factory/knowledge/` and restoring the note files reproduces the same
  index and backlinks, proving the graph is derived rather than stored.
- `memory add` writes only under the instance's `.factory/memory/<scope>/` for
  the calling scope, never another scope's directory, and leaves existing entries
  unmodified. Memory is keyed by scope rather than stored beside it (ADR 0015),
  so it survives the project being moved, re-cloned, or deleted.
- Both writing commands record the write as a task event, so the provenance of a
  note or memory entry survives the session that produced it.

**Decisions or risks to resolve first:** Decide how the note body reaches the
command — standard input is the obvious answer for prose, but it must be settled
before the interface is written. Decide whether `knowledge write` may update an
existing note or only create one: checking a new note against existing ones for
contradictions is a judgement the CLI cannot make and must leave with the agent,
so an update path needs a rule for what the command verifies and what it trusts.
Decide the note-name rules, since the filename is the link target and renaming a
note breaks every `[[link]]` pointing at it.

**Migration, not part of this slice:** `knowledge/wiki/` holds seventeen curated
pages today with their own `SCHEMA.md`, and `MEMORY.md` files exist at scope
roots. Both are gitignored, so a careless move is not recoverable from Git.
Moving them into `.factory/` and converting the pages to linked notes needs its
own task with a migration and rollback plan, in the same way the task store does.

## 13. Scope-bound secret storage

**Objective:** Give plugins and integrations a credential boundary that keeps
secret values out of prompts, task records, context, logs, and the operational
database.

**Scope:** Implement `factory secret set|list|remove` per design section 7.
Values go to the macOS Keychain under a deterministic, scope-bound service name.
Only a non-secret reference and capability metadata are written to the
company-root secret registry. `list` shows references and status, never values;
`remove` deletes the vault item and its registry entry after confirmation.

Port the existing prototype registry rather than inventing a format: the live
`.factory/secrets.yaml` already carries `scope`, `integration`, `account`,
`capabilities`, and `status`, and matches the design. Tighten its file mode
while porting — it is currently world-readable, which is wrong for a file
naming accounts and integrations even though it holds no secret values.

**Dependencies:** Slice 10.

This is a slice of its own rather than one more command group in slice 10
because Keychain access is a platform integration with failure modes no other
command has, and burying them inside the CLI-and-Claude-Code slice would hide
them. It comes after slice 10 because no earlier slice consumes a credential:
the prototype registry carries the load until then, and design section 10 does
not list secrets among the MVP completeness criteria.

**Acceptance criteria:**
- A secret set for one scope is retrievable by that scope and is not reachable
  through another scope's service name.
- No command path writes a secret value to the database, a task record, a
  generated context, a log, or terminal output; `list` output is safe to paste.
- `remove` deletes both the vault item and the registry reference, and a partial
  failure leaves an inspectable state rather than a dangling reference.
- The registry remains readable and correct after a Keychain item is deleted
  outside Factory; the entry reports a missing-vault-item status instead of
  failing the whole command.
- The registry file is not world-readable.

**Decisions or risks to resolve first:** Confirm the Keychain access behaviour
for an unsigned binary. An unsigned `factory` may prompt on every access, and
"always allow" depends on stable code signing, which makes this partly a release
packaging question rather than only an implementation one. Decide whether
version 1 accepts repeated prompts, and whether a non-macOS vault backend is in
scope at all.

## Deferred production stations

Design section 12 classifies four further stations as later work, and ADR 0020
adds a fifth item that is not a §12 station but is deferred on the same terms.
None is a version-1 slice and none has acceptance criteria here; they are listed
so the delivery plan states what was deliberately left out.

| Station | Smallest next step after version 1 |
|---|---|
| Material supply (12.4) | Declared material paths in `AGENTS.md`, compiled with source headings and failing loudly on a missing path. Changes the Slice 4 context contract. |
| Operating data (12.5) | One read projection over task-run events plus a read-only `factory stats` command. Depends on the 12.1 and 12.2 hooks carrying real verdicts. |
| Goods receipt and dispatch (12.7) | An intake path that turns an inbound message into a task run with recorded provenance, and a dispatch run whose external effect is approval-gated. |
| Learning loop (12.8) | A scheduled review run that reads decisions and verification findings and proposes template and `AGENTS.md` changes for human approval. |
| Evaluation bench (ADR 0020) | A fixture corpus with executable acceptance gates, plus a runner that executes one case across a harness-and-model matrix and reports the comparison. Version 1 records the run fields ADR 0020 pins; the corpus, runner, and report are post-version-1. |
