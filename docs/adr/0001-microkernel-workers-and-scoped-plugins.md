# ADR 0001: Microkernel, worker lifetimes, and scoped plugins

- Status: Accepted
- Date: 2026-09-03
- Owners: Business Factory

## Context

Factory must support two substantially different agent workloads. A
communication agent remains available across conversations, while a worker that
performs a bounded task should exist only for that task. Some tasks require no
language model at all and should run a script or other process in a visible,
isolated terminal.

The initial system uses Herdr, Pi, local macOS services, and company-specific
integrations. These are useful adapters, not properties of the reusable Factory
product. Coupling them to the core would make Factory difficult to reuse and
would expose integrations to scopes that neither need nor authorize them.

## Decision

### 1. Two agent types

Factory defines two explicit agent types with different lifetimes.

#### Thread agent

A thread agent is a persistent service agent intended for communication.

- It has a stable configured identity and remains online while enabled.
- It consumes and produces durable messages in one or more threads.
- It may answer directly, create a task, or report a task result back to the
  originating thread.
- Factory reconciles and restarts it after runtime or machine failure according
  to its configured availability policy.
- Its conversational continuity is not itself canonical knowledge. Durable
  facts still pass through the knowledge capture and promotion process.

#### Task agent

A task agent is an ephemeral worker created for one task run.

- Factory starts it only after the run has been durably queued and assigned.
- It receives a bounded workspace, context pack, capabilities, and task prompt.
- It records progress, decisions, results, and artifacts against that run.
- When the run reaches a terminal state, Factory stops the agent and releases
  its runtime resources and workspace lease.
- Its terminal and native conversation are disposable after required output and
  diagnostics have been captured.

A thread agent may create or supervise tasks, but it does not become the task
agent. The two identities and lifetimes remain distinct.

### 2. A task run selects an executor

An agent is not required to execute a task. Each task run has an execution
specification selecting one executor kind.

```text
task run
├── agent executor   -> create one ephemeral task agent
└── process executor -> run an argv/script in a new terminal
```

The process executor:

- uses an argv array or a repository-owned script path rather than an implicit
  shell command string by default;
- runs in an explicitly resolved workspace with a controlled environment;
- records start time, terminal/runtime reference, exit status, timeout,
  captured output, and artifact references;
- can be shown in a terminal supplied by the selected runtime plugin; and
- terminates and releases resources when the run completes, fails, is
  cancelled, or times out.

Both executor kinds use the same task state machine, audit events, approval
rules, cancellation semantics, and artifact model.

### 3. Threads and tasks are separate but linked

A thread is a durable ordered stream of messages. A task is a durable unit of
work with assignment and lifecycle. Neither substitutes for the other.

- A message may exist without creating a task.
- A thread agent may promote a request in a thread to a task.
- A task records its originating thread and message when applicable.
- Task progress and the final result may be projected back into that thread.
- A schedule creates a task run from a task template; it never injects an
  untracked command directly into a terminal.

### 4. Thin microkernel

The reusable Factory core is a microkernel. It owns only invariants that must be
consistent across every integration:

- stable IDs, scopes, hierarchy, and configuration resolution;
- threads, messages, tasks, runs, executions, and their state machines;
- a canonical append-only event store, CQRS projection checkpoints, and
  workspace leases;
- scheduling semantics and durable trigger deduplication;
- capability grants, approval records, and secret references;
- plugin discovery, compatibility checks, activation, and lifecycle;
- command routing and a versioned machine-readable API; and
- recovery, inspection, and health contracts.

The kernel must not contain code specific to Herdr, a model harness, a messaging
service, an operating-system scheduler, a credential vault, Obsidian, an
embedding provider, or a hosted service.

### 5. Plugin extension points

External systems and replaceable implementations enter through versioned
plugin contracts. Initial extension points are:

| Extension point | Examples |
|---|---|
| Runtime provider | Herdr, direct terminal process, remote runtime |
| Agent harness | Pi, Claude Code, Codex, local model harness |
| Task executor | Agent executor, visible process/script executor |
| Channel | CLI, iMessage, email, Slack, webhook |
| Trigger provider | `launchd`, cron service, webhook |
| Knowledge provider | Markdown vault, graph index, embeddings |
| Secret provider | macOS Keychain, another credential vault |
| Notification provider | terminal, desktop, messaging channel |

Herdr is the first runtime provider. Factory talks only to the runtime contract;
the Herdr plugin owns workspace, pane, terminal, process-observation, and cleanup
behavior.

Plugins request kernel mutations through public contracts and do not write core
database tables directly. Plugin-specific cursors, caches, or indexes live in
namespaced state and must not become hidden canonical state.

### 6. Scope-specific plugin activation

Plugin availability and plugin activation are separate.

- Installing a plugin makes its implementation available but grants no scope
  permission and does not run it.
- Each registered scope explicitly enables the plugins it needs and grants the
  required capabilities.
- Plugins are not implicitly inherited by descendants. A company configuration
  may declare an explicit inherited default, and a child must still be able to
  inspect the resulting activation.
- Business-specific integrations may live in and be enabled by the
  `business-factory` installation without becoming dependencies of the reusable
  `factory` project.
- The same core installation can therefore host scopes with different runtime,
  messaging, knowledge, and secret-provider plugins.

A plugin manifest must at least declare a stable ID, version, compatible kernel
API version, entrypoint, provided extension points, requested capabilities,
configuration schema, and health check.

### 7. Plugin safety boundary

The preferred plugin boundary is an external process using a versioned local
protocol. This keeps dependencies and failures outside the kernel and permits
plugins written in different languages. The first implementation may begin
with JSON messages over standard input/output, but the transport is not part of
this ADR and requires a dedicated protocol decision.

The kernel supplies only the minimum invocation context and capability handles.
Secret values are resolved by an authorized secret provider at execution time
and are never placed in task prompts, plugin manifests, audit payloads, or
knowledge notes.

Because all local processes currently share one trusted operating-system
account, capability enforcement initially provides policy, least-privilege
interfaces, and auditability rather than hard sandbox isolation.

## Consequences

- The former assumption that every configured agent is persistent no longer
  holds; residency is explicit in the agent type.
- Session records become runtime instances rather than the durable identity of
  every agent.
- Script execution becomes a first-class task path without pretending that a
  process is an AI agent.
- Core task and message semantics remain stable if Herdr or Pi is replaced.
- A broken optional integration cannot be allowed to corrupt core state.
- Plugins add API-versioning, capability, lifecycle, and diagnostic work to the
  MVP, but prevent company-specific code from accumulating in the kernel.

## Follow-up decisions

1. Plugin installation layout, dependency verification, and upgrade rollback.
2. Thread delivery and ordering guarantees.
3. Task execution specification, log retention, and terminal cleanup policy.
4. Scope activation and explicit inheritance syntax.

Rust packaging was resolved by ADR 0002. The plugin process protocol and
lifecycle were resolved by ADR 0007.

ADR 0010 reconciles this ADR with the company design baseline
`.specs/design.md`. Two points of scope apply to the decisions above. The thread
agent and task agent of section 1 are the `permanent` and `temporary` agent
lifetimes of design §2.2 — the same distinction under two vocabularies. Threads
and durable messages, including follow-up decision 2, are target state and not
version-1 scope: design §2.4 keeps the task as the only durable unit of work
exchanged. Follow-up decision 3 remains open and is the origin of the
unspecified log-retention policy.
