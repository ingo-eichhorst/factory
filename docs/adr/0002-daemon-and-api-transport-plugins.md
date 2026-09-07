# ADR 0002: Daemon-owned kernel and API transport plugins

- Status: Accepted
- Date: 2026-09-03
- Owners: Business Factory

## Context

Factory coordinates persistent thread agents, ephemeral task workers, scheduled
work, durable messages, plugin processes, subscriptions, and recovery. Although
these jobs can be driven by repeated CLI invocations, a continuously running
owner provides faster dispatch, a single mutation boundary, live event streams,
central plugin supervision, and one place to reconcile runtime state.

The command-line client and a future dashboard must use the same behavior and
authorization rules. Neither should become a privileged alternative code path
or gain direct access to the operational database or runtime providers.

## Decision

### 1. One Rust distribution, daemon and client modes

Factory is distributed as one Rust executable named `factory`.

- `factory daemon run` starts the long-running kernel process.
- `factory daemon start|stop|status` manages or inspects the configured service.
- Ordinary `factory ...` commands run as API clients of that daemon.
- A future `factoryd` alias may be provided for operating-system integration,
  but it is not a separate implementation or package.

There is one authoritative daemon per Factory installation/company root. It
holds an exclusive installation lock and is the only process permitted to
mutate the core operational database.

### 2. Daemon responsibilities

The daemon owns:

- the command/query boundary and domain state machines;
- SQLite event-store connections, transactions, migrations, CQRS projections,
  and replay;
- the durable action outbox and idempotent dispatch;
- schedules and trigger deduplication;
- plugin startup, health, restart, and shutdown;
- thread-agent availability and reconciliation;
- task-agent and process-executor lifecycle and cleanup;
- workspace leases and concurrency limits;
- authentication context, capability enforcement, and approvals;
- resumable event subscriptions; and
- recovery after daemon, plugin, runtime, or machine restart.

Clients, plugins, agents, and task processes never write core tables directly.
They submit commands and queries through the kernel API.

### 3. Transport-neutral command and event API

The kernel defines a versioned logical API independent of any wire protocol.
It contains three interaction styles:

- commands that may change state;
- queries that read current state; and
- a cursor-based event stream for subscriptions and resumption.

Every request carries an API version, request ID, optional idempotency key,
target installation/scope, operation, and typed payload. Authentication context
is established by the transport and cannot be trusted merely because a client
supplied an actor name. Responses include the request ID and structured result
or error. Persisted events include a monotonic cursor and correlation IDs.

The write path is:

```text
client
  -> API transport plugin
  -> authenticate and normalize
  -> kernel authorization
  -> domain command
  -> SQLite transaction: events + projections + outbox
  -> response and subscribed events
```

### 4. API transports are plugins

The kernel does not embed a CLI protocol, HTTP server, WebSocket server, or UI.
API transport plugins expose the logical kernel API to clients.

Initial transport plugins are expected to include:

| Plugin | Intended clients |
|---|---|
| Local IPC | `factory` CLI and local administrative tools |
| WebSocket | Live dashboard and interactive applications |
| HTTP | Optional automation and integration clients |

The CLI and dashboard are applications, not kernel components. They may format
and present data but may not reimplement domain state transitions,
authorization, scheduling, or runtime control.

Transport plugins authenticate callers and translate protocol frames. The
kernel remains responsible for authorization and domain validation. Transport
plugins cannot bypass the command API or access core storage directly.

### 5. Bootstrap and recovery access

A plugin-based API requires a recovery path when optional configuration or a
third-party plugin is broken.

- Factory ships a first-party local-IPC transport plugin alongside the binary.
- A new installation enables that plugin by default in its root configuration.
- The daemon reads only bootstrap configuration and plugin manifests directly;
  all ordinary client operations still use the API.
- `factory daemon run --safe-mode` loads the bundled local-IPC transport and
  core inspection/repair operations while suppressing optional plugins and
  external listeners.
- Safe mode is local-only, is recorded in the audit log, and does not permit a
  second daemon to bypass the installation lock.

The bundled transport is replaceable in normal operation but always available
for recovery from the installed Factory distribution.

### 6. Local socket and authentication

The first local-IPC transport uses a Unix-domain socket.

- On macOS its default location is
  `~/Library/Application Support/Factory/<installation-id>/factory.sock`.
- On Linux its default location is
  `$XDG_RUNTIME_DIR/factory/<installation-id>/factory.sock`.
- The parent runtime directory is mode `0700`; the socket is accessible only to
  its owner.
- The transport verifies peer credentials and rejects a different operating-
  system user.

Same-user access identifies a local human/operator boundary but does not assign
an agent identity. Agents, task workers, and plugins receive short-lived,
scope-bound capability credentials created by the daemon. The daemon maps those
credentials to the recorded actor and allowed operations. Credential values are
never placed in prompts, audit payloads, plugin manifests, or repository files.

A WebSocket or HTTP transport requires its own explicit authentication
configuration. Loopback binding is the default. Non-loopback exposure requires
transport security and cannot be enabled merely by installing the plugin.

### 7. Activation levels

API transport plugins are enabled at the Factory-installation root because they
must accept and authenticate a request before its target scope is known. Their
authenticated principals are still limited to authorized scopes.

Runtime, harness, channel, knowledge, trigger, secret, and notification plugins
may be activated per scope according to ADR 0001. Installation-level activation
does not grant blanket scope access.

### 8. Startup, shutdown, and restart

Daemon startup proceeds in this order:

1. resolve and validate the Factory installation root;
2. acquire the exclusive installation lock;
3. open, migrate, and validate core storage;
4. load the bundled local-IPC transport or enter safe mode on request;
5. load configured API transports and scope plugins;
6. reconcile persisted agents, task executions, leases, schedules, and outbox
   actions; and
7. report ready.

On shutdown, the daemon stops accepting new mutations, drains or durably
requeues dispatch work, requests plugin shutdown, records its state, and
releases the lock. Ephemeral task executions follow their configured shutdown
policy; they are not silently reported as completed.

After restart, persisted state remains authoritative. Ambiguous external
actions are reconciled through their plugin and idempotency record rather than
blindly repeated.

## Consequences

- The daemon becomes a required local service for ordinary Factory operations.
- CLI, dashboard, and future clients receive identical behavior through the
  same logical API.
- SQLite has one mutation owner, simplifying transactional invariants and event
  subscriptions.
- Herdr and every network listener remain replaceable plugins rather than
  kernel dependencies.
- The system must implement service installation, local IPC, authentication,
  safe mode, health reporting, and restart reconciliation in its first usable
  release.
- Offline database mutation is deliberately excluded. Backup verification and
  low-level repair require the daemon to be stopped or started in safe mode.

## Follow-up decisions

1. Capability credential format and delivery to agents and plugins.
2. Service-manager packaging for macOS and Linux.
3. WebSocket authentication and browser-session model.

The plugin host protocol and API transport process boundary were resolved by
ADR 0007.
