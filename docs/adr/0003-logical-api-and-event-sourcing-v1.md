# ADR 0003: Logical API messages and event sourcing v1

- Status: Accepted
- Date: 2026-09-03
- Owners: Business Factory
- Supersedes: ADR 0004, ADR 0005, ADR 0006

## Context

Factory clients communicate with one daemon through interchangeable API
transport plugins. Commands, queries, successful responses, errors, and
persisted events need one coherent logical protocol instead of independent
standards that may drift.

Factory must also reconstruct domain state, rebuild query models, audit every
decision, and recover long-running distributed work. Audit-only events are not
sufficient: the event stream must contain the facts needed for deterministic
replay. Calls through runtime, harness, channel, knowledge, and other plugins
may partially succeed, so distributed workflows require durable saga state and
explicit compensation data.

## Decision

### 1. Protocol principles

- The logical API is independent of Unix sockets, WebSockets, HTTP, and any
  other transport.
- Top-level envelopes and their operation-specific payloads are strictly
  validated.
- Stable IDs use UUIDv7 unless a field is explicitly defined as a monotonically
  increasing cursor or stream version.
- Authentication context is established by a transport and cannot be asserted
  inside client-controlled JSON.
- The append-only event store is canonical for domain state. Query tables,
  indexes, snapshots, and caches are derived projections.
- Replaying events never invokes plugins or produces external side effects.

### 2. Command request

Every mutation uses:

```json
{
  "api": "factory.command/v1",
  "request_id": "019c0000-0000-7000-8000-000000000001",
  "idempotency_key": "019c0000-0000-7000-8000-000000000002",
  "scope_id": "c5fbc985-656a-4219-8614-fcaeaddbf103",
  "command": "task.create",
  "payload": {
    "title": "Generate weekly report",
    "executor": {
      "type": "agent"
    }
  }
}
```

Commands that modify an existing stream may also carry:

```json
{
  "expected_revision": 7
}
```

| Field | Required | Meaning |
|---|---:|---|
| `api` | yes | Exact envelope schema, initially `factory.command/v1` |
| `request_id` | yes | UUIDv7 identifying one API attempt |
| `idempotency_key` | yes | UUIDv7 identifying one intended mutation across retries |
| `scope_id` | yes | Stable UUID of the target Factory scope |
| `command` | yes | Dotted imperative operation such as `task.create` |
| `payload` | yes | Object validated against the command schema |
| `expected_revision` | no | Expected current version of the command's primary stream |

On retry, a client creates a new `request_id` and retains the same
`idempotency_key`. Idempotency is scoped to authenticated principal, scope, and
command. Reusing a key with the same canonical payload returns the recorded
outcome. Reusing it with a different payload returns
`conflict.idempotency_key_reused` and appends no domain event.

The command handler loads the target stream, reduces its events to current
state, validates the command and expected revision, and atomically appends one
or more new events. A snapshot may accelerate loading but is never canonical.

### 3. Query request

Every read uses:

```json
{
  "api": "factory.query/v1",
  "request_id": "019c0000-0000-7000-8000-000000000003",
  "scope_id": "c5fbc985-656a-4219-8614-fcaeaddbf103",
  "query": "task.get",
  "payload": {
    "task_id": "019c0000-0000-7000-8000-000000000004"
  }
}
```

| Field | Required | Meaning |
|---|---:|---|
| `api` | yes | Exact envelope schema, initially `factory.query/v1` |
| `request_id` | yes | UUIDv7 correlating this API call |
| `scope_id` | yes | Stable UUID of the target Factory scope |
| `query` | yes | Dotted read operation such as `task.get` |
| `payload` | yes | Object validated against the query schema |

Queries do not carry mutation idempotency or expected revision. Filters,
pagination, limits, and historical positions belong to the typed query payload.
Unless explicitly historical, a query reads a projection at its latest applied
event cursor.

### 4. Successful response

Every successful command or query returns:

```json
{
  "api": "factory.response/v1",
  "request_id": "019c0000-0000-7000-8000-000000000003",
  "event_cursor": 1842,
  "result": {
    "task_id": "019c0000-0000-7000-8000-000000000004",
    "status": "running"
  }
}
```

| Field | Required | Meaning |
|---|---:|---|
| `api` | yes | Exact response schema, initially `factory.response/v1` |
| `request_id` | yes | ID of the originating request attempt |
| `event_cursor` | yes | Latest event included in the result's committed state |
| `result` | yes | Object validated against the operation response schema |

For a successful command, the cursor includes the newly committed events and
the result is derived from the resulting aggregate state. For a query, it is
the projection checkpoint used for the read. A client may apply the response
and subscribe after that cursor without a snapshot/event gap.

The envelope has no `ok` flag. It does not repeat the requested scope. Domain
timestamps, pagination cursors, and totals belong inside the typed result.

### 5. Error response

Every logical failure that can be returned uses:

```json
{
  "api": "factory.error/v1",
  "request_id": "019c0000-0000-7000-8000-000000000003",
  "error": {
    "code": "conflict.revision_mismatch",
    "message": "The task changed after it was read.",
    "retryable": false,
    "details": {
      "task_id": "019c0000-0000-7000-8000-000000000004",
      "expected_revision": 7,
      "actual_revision": 8
    }
  }
}
```

| Field | Required | Meaning |
|---|---:|---|
| `api` | yes | Exact error schema, initially `factory.error/v1` |
| `request_id` | yes, nullable | Originating ID, or `null` if it could not be decoded |
| `error.code` | yes | Stable dotted machine-readable identifier |
| `error.message` | yes | Safe concise human-readable explanation |
| `error.retryable` | yes | Whether the logical request may succeed later unchanged |
| `error.details` | yes | Validated corrective data, or an empty object |

Initial code families are `validation.*`, `authentication.*`,
`authorization.*`, `not_found.*`, `conflict.*`, `unavailable.*`, and
`internal.*`. Messages are not program interfaces. Details never expose
credentials, stack traces, SQL, or raw private plugin output. Transport plugins
may map the error to native status codes while preserving its logical body.

A rejected command appends no domain event. A failure after a command was
accepted is an asynchronous domain fact and therefore appears as a persisted
event rather than a retroactive API error.

### 6. Persisted event

Every canonical domain fact uses:

```json
{
  "api": "factory.event/v1",
  "cursor": 1843,
  "event_id": "019c0000-0000-7000-8000-000000000005",
  "scope_id": "c5fbc985-656a-4219-8614-fcaeaddbf103",
  "stream": {
    "type": "task_run",
    "id": "019c0000-0000-7000-8000-000000000004",
    "version": 3
  },
  "event": "task.run.started",
  "payload_version": 1,
  "recorded_at": "2026-09-03T20:45:42.762Z",
  "actor": {
    "type": "agent",
    "id": "019c0000-0000-7000-8000-000000000006"
  },
  "correlation_id": "019c0000-0000-7000-8000-000000000007",
  "causation_id": "019c0000-0000-7000-8000-000000000001",
  "payload": {
    "executor": "agent",
    "worker_id": "019c0000-0000-7000-8000-000000000008"
  }
}
```

| Field | Required | Meaning |
|---|---:|---|
| `api` | yes | Exact persisted-event envelope, initially `factory.event/v1` |
| `cursor` | yes | Gap-free global append position within the installation |
| `event_id` | yes | Permanent UUIDv7 identity of the fact |
| `scope_id` | yes | Scope that owns and authorizes the fact |
| `stream.type` | yes | Aggregate/stream type |
| `stream.id` | yes | Stable aggregate/stream ID |
| `stream.version` | yes | Gap-free version within that stream, starting at one |
| `event` | yes | Dotted past-tense fact such as `task.run.started` |
| `payload_version` | yes | Schema version for this event payload |
| `recorded_at` | yes | Daemon-assigned UTC commit time |
| `actor` | yes | Authenticated human, agent, plugin, process, or system actor |
| `correlation_id` | yes | ID grouping one command, saga, or wider activity |
| `causation_id` | yes | Request or prior event directly causing this event |
| `payload` | yes | Data required by the event's deterministic reducer |

Events are immutable. An error, correction, cancellation, or supersession is a
new event. Event payloads must contain the durable values or immutable content
references required to reconstruct the resulting aggregate; reducers may not
consult mutable external systems, the current wall clock, randomness, or
another projection.

### 7. Event store, CQRS, and replay

The append-only event store is the source of truth for Factory domain state.

- Each aggregate owns a stream identified by `(stream.type, stream.id)`.
- Stream versions enforce optimistic concurrency.
- The global cursor establishes projection and subscription order.
- Core write-model changes, appended events, idempotency outcomes, outbox
  intents, and synchronously maintained core projections commit atomically.
- Query models, search indexes, dashboards, and plugin projections track the
  last applied global cursor and process every event idempotently.
- A projection can be dropped and rebuilt by applying events from cursor one.
- Snapshots contain a stream version/cursor and state hash, but are disposable
  accelerators.
- Stored events are never rewritten during schema evolution. Versioned pure
  upcasters convert old payloads in memory before reducers consume them.

Replay runs in a mode that disables outbox emission, plugin calls, timers,
notifications, agent creation, process creation, and every other external side
effect. Replaying `task.run.started` reconstructs state; it does not start a
worker again. Replay failures stop at the exact cursor with a deterministic,
inspectable error.

Backups must include the event store, idempotency records, immutable referenced
content required by reducers, and projection definitions/migrations. Derived
projections and indexes need not be backed up when they can be rebuilt.

### 8. Distributed work and sagas

The daemon coordinates multi-step external work as event-sourced sagas. Database
rollback cannot undo a call already accepted by an external participant, so
recovery uses explicit compensating actions.

Before dispatching a saga step, Factory appends a planned event containing:

- saga and step IDs;
- target plugin and operation;
- immutable input or secure/content reference and integrity hash;
- participant idempotency key;
- timeout and retry policy;
- reversibility classification;
- compensation operation and validated input, or the information required to
  construct it after the forward result; and
- preconditions such as external version, ETag, or prior-value reference.

The committed planned event and outbox intent precede the external call. The
resulting success or failure event records the participant's operation ID,
returned version/preconditions, durable result reference, and the data needed
for later compensation without querying a mutable past state.

Compensation is itself a new idempotent command and event sequence. It never
deletes the forward events. Saga steps declare one of:

```text
reversible       exact inverse is supported
compensatable    a business-level mitigating action is supported
irreversible     no meaningful compensation exists
```

Irreversible steps require explicit approval and should be ordered after
reversible work where possible. Examples include a message that has already
been delivered or a destructive external operation with no restore API.
Factory must not claim such a step was rolled back. A failed compensation
enters `compensation_failed` or `manual_intervention_required` with the full
durable evidence needed to continue.

Plugin operation manifests declare their idempotency behavior, reversibility
class, compensation schema, and reconciliation capability. A plugin cannot mark
an operation reversible without implementing and testing the compensation
contract.

### 9. Trusted invocation and sensitive data

The transport supplies principal ID/kind, authorized scope access,
capabilities, transport/connection identity, and agent/worker/plugin binding as
trusted invocation context alongside commands and queries. The daemon derives
the authoritative event actor from that context.

Credentials and secret values never enter logical messages or the event store.
Sensitive data needed for replay or compensation is held through a durable,
access-controlled content reference with integrity metadata and explicit
retention. If required compensation data cannot be retained safely, the step
cannot be classified as automatically reversible.

## Consequences

- All logical API messages and persisted events have one normative ADR.
- Event streams, rather than mutable domain rows, are canonical Factory state.
- Query tables and indexes are rebuildable CQRS projections.
- Schema evolution requires immutable event retention, payload versioning,
  reducer tests, and deterministic upcasters.
- External side effects are never replayed.
- Distributed failure recovery requires operation-specific idempotency,
  reconciliation, and compensation support from plugins.
- Some real-world actions remain irreversible and must be approval-gated rather
  than described as rollback-safe.
