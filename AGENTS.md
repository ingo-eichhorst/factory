# Factory

## Purpose

You build and maintain the production Business Factory implementation: the
local-first CLI and microkernel that let humans and agents operate scopes,
threads, messages, durable tasks, schedules, context, and shared knowledge.

The Rust `factory` executable provides both daemon and client modes. The daemon
is the sole owner of core mutations and runtime coordination. CLI, dashboard,
agents, and other clients use its versioned logical API through enabled API
transport plugins; they never access core SQLite tables or Herdr directly.

The append-only domain event store is canonical. Mutable read tables, search
indexes, snapshots, and dashboards are CQRS projections that must be
rebuildable by deterministic replay. Replay must never invoke plugins or repeat
external side effects.

This project turns the architecture in the company-root `.specs/` directory
into an installable, testable system. Prefer a small coherent domain model over
separate tools that duplicate identity, persistence, authorization, or audit
concepts.

## Ownership

- Own the production Factory CLI, its domain services, persistence schema,
  migrations, adapters, tests, operator documentation, and release packaging.
- Treat the company-root `../../.specs/` documents and ADRs as architectural
  inputs. Record material departures as explicit decisions before implementing
  them.
- Existing company-root scripts and extensions are prototypes or operational
  compatibility paths. Do not move, replace, or remove them without a task that
  defines the migration and rollback plan.
- Herdr is the initial terminal-runtime plugin. Harness- and runtime-specific
  behavior must remain behind plugins and must not leak into the core domain
  model.

## Product boundaries

- Keep thread agents and task agents distinct. Thread agents are persistent
  communication services. Task agents are ephemeral workers created for one
  task run and removed after their result and diagnostics are captured. In
  configuration these are an agent's `permanent` and `temporary` lifetimes; the
  two vocabularies name the same distinction (ADR 0010).
- A task may instead use a process executor to run an argv or repository-owned
  script in a new terminal. Processes and task agents share task lifecycle,
  audit, approval, cancellation, and artifact semantics.
- **Target state, not version 1:** threads and messages as durable objects
  distinct from tasks, linked so that a thread may produce a task and results
  may return to it. Version 1 has no message primitive — a task is the only
  durable unit of work exchanged, per design §2.4 and ADR 0010. A permanent
  agent serves a communication channel without Factory owning the thread, which
  is how the assistant scope already works. Do not build a message store or
  thread delivery for version 1.
- Keep the reusable kernel independent of external systems. Herdr, harnesses,
  messaging channels, OS schedulers, credential vaults, knowledge backends, and
  similar integrations belong behind versioned plugin contracts.
- Run every plugin as a supervised subprocess. Use the framed, bidirectional
  JSON-RPC host protocol for plugin calls; reserve plugin standard output for
  protocol frames and standard error for diagnostics.
- Treat API protocols as plugins as well. The bundled local-IPC transport,
  WebSocket, HTTP, and future transports expose one transport-neutral kernel
  command/query/event API rather than implementing domain behavior themselves.
- Preserve a local safe-mode recovery path using the bundled local-IPC plugin
  when optional plugins or external listeners fail to start.
- Make plugin activation explicit per registered scope. A plugin installed or
  enabled for `business-factory` must not silently become a dependency or
  capability of the reusable `factory` project or another child scope.
- Keep durable canonical state inspectable and local. Derived search, graph, or
  embedding indexes must be rebuildable from canonical records and files.
- **No Factory command writes to a file outside a `.factory/` directory.** This
  is the allowlist form of the rule below and supersedes it in strength: a list
  of forbidden paths is only as complete as its author's imagination, while a
  single permitted root is testable in one assertion. `factory init` and
  `factory scope add` may create the directory that will contain `.factory/`;
  creating the container is not writing content beside it. The rule constrains
  Factory, not agents — an agent performing a task writes wherever the task
  requires, and that work product is not a Factory write.
- Keep human-maintained instructions and knowledge safe. Never overwrite
  `AGENTS.md`, source documents, harness configuration, or user changes unless
  an explicit ownership marker and task authorize regeneration.
- Never store secrets in prompts, task records, knowledge notes, logs, fixtures,
  or generated context. Refer to credentials through the configured secret
  boundary.

## Engineering rules

- Develop in vertical slices with documented CLI behavior and automated tests.
- Make mutations transactional and idempotent where retries or restarts are
  possible. Persist every domain change as an immutable replayable event.
- Coordinate distributed plugin calls as sagas. Persist forward intent,
  participant idempotency, result evidence, and operation-specific compensation
  data before relying on an external effect. Approval-gate steps that cannot be
  meaningfully compensated.
- Treat plugin delivery as at-least-once. After timeout, pipe failure, or plugin
  crash, reconcile the durable operation identity before retrying; never infer
  that an external action failed merely because its response was lost.
- Design read-only inspection before automation: users must be able to see the
  exact scope, task, message, context, knowledge source, and runtime state that
  caused an action.
- External side effects remain approval-gated. A schedule or agent decision does
  not itself authorize sending, publishing, purchasing, deleting, or changing
  an external system.
- Do not create, merge, delete, or clean up Git worktrees unless the human asks
  explicitly.

## Task workflow

Use the shared Factory task system for every assigned run: inspect it with
`run_get`, record start and material progress with `run_progress`, record each
material choice with `run_decision` including rationale and alternatives, then
finish with `run_complete` and artifact references or use `run_block` with a
specific reason.
