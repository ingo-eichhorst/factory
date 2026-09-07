# ADR 0007: Supervised plugin process protocol v1

- Status: Accepted
- Date: 2026-09-03
- Owners: Business Factory

## Context

Factory keeps runtimes, agent harnesses, channels, knowledge providers, secret
providers, triggers, notifications, and API transports outside its microkernel.
Those plugins need a language-neutral boundary that isolates dependency and
process failures without adding a second service framework to the first usable
release.

The protocol must support calls in both directions. The daemon invokes plugin
operations, while a transport plugin must submit authenticated client commands
and queries to the kernel. Calls may be retried after timeouts or process
failure, so the boundary must preserve the operation identities, reconciliation
data, and saga semantics established by ADR 0003.

## Decision

### 1. Every plugin is a supervised subprocess

Factory v1 has no third-party in-process plugin ABI. The daemon starts each
enabled plugin as a child process and supervises its entire lifecycle.

- A scope plugin normally has one process for each `(plugin_id, scope_id)`
  activation.
- An API transport plugin has one process for each
  `(plugin_id, installation_id)` activation because it authenticates callers
  before a target scope is selected.
- A future plugin may explicitly declare a safe multi-scope process model, but
  v1 does not assume or require it.
- The daemon gives the process a minimal environment and a private runtime
  directory. Credentials and secret values are not placed in arguments,
  environment variables, configuration JSON, or protocol logs.
- Closing the supervised protocol connection removes the process's authority
  to call back into the kernel.

The bundled local-IPC transport obeys the same process boundary. It is shipped
inside the `factory` executable and launched through a private child-process
mode of that executable. Safe mode starts only this bundled transport and does
not load optional plugin executables.

### 2. JSON-RPC 2.0 over framed standard I/O

The daemon and plugin exchange bidirectional JSON-RPC 2.0 messages over the
child's standard input and standard output. Each UTF-8 JSON body is framed with
an ASCII `Content-Length` header and a blank line, following the framing shape
used by the Language Server Protocol.

```text
Content-Length: 123\r\n
\r\n
{...exactly 123 UTF-8 bytes...}
```

- Standard output contains protocol frames only. Human and structured
  diagnostic logs go to standard error.
- The initialization handshake negotiates a maximum frame size. Exceeding it,
  emitting malformed framing, or writing non-protocol output to standard
  output is a protocol violation.
- Large artifacts, knowledge documents, and binary data travel through
  durable, access-controlled content references with integrity hashes rather
  than inline frames.
- JSON-RPC IDs correlate one live protocol exchange. They do not replace the
  logical API `request_id`, domain `event_id`, saga step ID, operation ID, or
  idempotency key.
- Method names and parameter/result schemas are versioned Factory contracts.
  Unknown required methods fail explicitly rather than being ignored.

This internal host protocol is distinct from the transport-neutral command,
query, response, error, and event envelopes in ADR 0003.

### 3. Initialization and capability binding

Immediately after spawn, the daemon sends `factory.initialize`. Its parameters
include:

- plugin instance, installation, and activation-scope identities;
- kernel version and supported host-protocol versions;
- the already validated manifest identity and digest;
- non-secret validated configuration;
- granted capabilities and resource limits; and
- supported frame size and health-check timing.

The plugin selects one compatible protocol version and returns its plugin
identity, version, manifest digest, provided operations, operation schema
digests, and supported health and reconciliation features. These declarations
must agree with the installed manifest. A mismatch prevents readiness and is
recorded as a diagnostic failure.

After successful validation, the daemon sends `factory.initialized` and marks
the process ready. The supervised pipe itself identifies the plugin instance;
the child does not authenticate itself by sending a bearer credential in JSON.
Every reverse call is authorized against the capabilities bound to that
instance.

The protocol supports these method families:

- daemon to plugin: invoke, reconcile, health check, and graceful shutdown;
- plugin to kernel: authorized commands, queries, event subscriptions, and
  capability-mediated content or secret-provider operations; and
- either direction: bounded notifications associated with an established
  request or subscription.

A plugin never receives a database handle and never writes event-store or
projection tables directly.

### 4. Lifecycle and supervision

A plugin instance moves through explicit persisted operational states:

```text
discovered -> validated -> starting -> ready -> stopping -> stopped
                                  \-> degraded -> failed -> restarting
                                                       \-> quarantined
```

- Startup and initialization have deadlines. An instance is routable only in
  `ready` state.
- The daemon performs bounded health checks and records state transitions.
- Shutdown first stops new dispatch, then sends a graceful shutdown request,
  waits for the configured deadline, terminates the process, and finally kills
  it only if it still does not exit.
- Unexpected exits use exponential restart backoff with jitter and a bounded
  restart budget. Exceeding the budget quarantines the activation until an
  operator explicitly retries or configuration changes.
- Protocol violations, manifest mismatches, and repeated failed health checks
  use the same visible failure and quarantine path.

Restarting a plugin does not imply that an interrupted external operation is
safe to repeat.

### 5. Delivery, idempotency, and reconciliation

Plugin invocation is at-least-once at the protocol boundary. Factory does not
claim exactly-once execution across an external process or service.

Before invocation, the daemon atomically persists the saga step's planned event
and outbox intent. The request carries the durable operation ID, participant
idempotency key, correlation and causation identities, deadline, and the
validated operation input or content reference. A plugin must propagate the
provided idempotency identity when its participant supports one.

The daemon persists the returned result or failure before acknowledging the
outbox item. A timeout, broken pipe, or plugin crash produces an `unknown`
outcome, not proof that the external action failed. Factory asks the restarted
plugin to reconcile the durable operation identity before considering another
invocation. If reconciliation is impossible, the saga enters the explicit
manual-intervention path defined by ADR 0003.

During event replay the daemon does not start, invoke, reconcile, health-check,
or notify plugins. Replay reconstructs only canonical state and derived
projections.

### 6. API transport plugins

API transports use the same subprocess and JSON-RPC boundary; they are not an
in-process exception.

- The transport process owns its Unix socket, TCP listener, WebSocket sessions,
  or HTTP connections.
- It authenticates the transport peer and submits the logical ADR 0003 envelope
  plus trusted invocation context through an authorized reverse RPC.
- The kernel performs scope authorization and domain validation and returns the
  logical response or error.
- Event subscriptions are cursor-based and flow back as bounded
  notifications. The transport must apply backpressure or disconnect a stalled
  client; it may not force unbounded buffering in the daemon.

The bundled local-IPC child is the only API transport enabled by safe mode, but
it receives no privileged domain bypass.

### 7. Compatibility

The handshake negotiates one host-protocol major version. Additive optional
methods and fields may be introduced within a major version only when older
peers can safely ignore them. A breaking framing, lifecycle, authorization, or
method change requires a new major version.

Operation payloads and results use separately versioned schemas identified by
the plugin manifest and verified during initialization. Installing a new
plugin version does not activate it until its manifest, executable identity,
protocol compatibility, and configuration validate successfully.

## Consequences

- Plugins may be implemented in any language that can speak framed JSON-RPC.
- Plugin crashes and dependency conflicts do not corrupt daemon memory.
- One common boundary covers ordinary providers and API transports.
- The bundled recovery transport remains available without creating a separate
  executable or an in-process privileged path.
- Standard I/O is simple and inspectable but is not intended for large payloads
  or high-volume media streams.
- At-least-once delivery requires every effectful plugin operation to define
  idempotency and reconciliation behavior honestly.
- Process separation improves reliability but is not a complete security
  sandbox while plugins run as the same operating-system user.

## Alternatives considered

### In-process Rust plugins

Rejected for v1 because a panic, unsafe dependency, or ABI mismatch could take
down the authoritative daemon, and plugins would be constrained to the
kernel's language and dependency graph.

### gRPC between daemon and plugins

Deferred because it adds code generation, HTTP/2 machinery, and a larger SDK
surface before the operation contracts have stabilized. The framed JSON-RPC
boundary can later be supplemented for a proven high-throughput need.

### Newline-delimited JSON

Rejected because accidental output and framing failures are harder to detect
reliably, and explicit byte lengths make limits and parser recovery clearer.

### One Unix socket per plugin

Deferred because standard I/O already gives each child a private,
daemon-created connection and process identity without socket discovery,
permissions, stale-file cleanup, or an additional authentication handshake.
