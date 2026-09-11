# ADR 0023: The daemon serves a browser UI, and what that costs

- Status: Accepted
- Date: 2026-09-11
- Owners: Business Factory
- Decided by: the human owner, 2026-09-11

## Context

Design §8 lists, among the version-1 non-goals, "browser dashboards — a served
web application." That line was written when the only client was the CLI and
the only transport was the Unix socket, and it was the right call then: a
dashboard is a second surface to keep honest, and the surface it would have
shown did not yet exist.

It now exists. Slices 1–12 have put thirteen queries and twenty commands on the
wire — enough to answer, for a real instance, which scopes are registered, which
agents are declared, which sessions are alive, which workspace each one holds,
what every task is doing, and whether the daemon is up. A design study built
against that surface established two things worth recording here:

1. The read surface is already sufficient for a control panel. Four of the
   six decision levels can be filled today from queries that exist.
2. The gaps are precise, not vague. Two levels cannot be filled at all,
   because `task_events` and `task_decisions` are written faithfully and no
   query reads them back — `events::for_task` and `decisions::for_task` both
   exist and have no caller outside tests.

The second point is the argument for serving the page rather than leaving it a
mockup. A dashboard that renders whatever a fixture contains hides the gap; a
dashboard wired to the real daemon cannot, because the empty panel is
load-bearing.

The owner has decided that the §8 non-goal no longer holds. This ADR records
that decision and its cost. **It does not amend `.specs/design.md`** — that is
a source document and the owner's to edit.

## Decision 1: HTTP is a second transport for the same envelope, not a second API

`POST /api` carries exactly ADR 0003's request envelope and answers with
exactly ADR 0003's response envelope, because `http::handle_api` hands the
request body to the same `server::dispatch_line` the socket uses and returns
its output verbatim. A request that works against the socket works over HTTP
byte for byte.

This is ADR 0002's "API transport plugins" taken literally, and it is the whole
reason a second transport is affordable. The alternative — a REST surface with
its own paths, verbs, and shapes — would have been a second place for the
operation vocabulary to live, and the first divergence between the two would
have been discovered by a user rather than a compiler.

The transport therefore decides exactly two things: how bytes are framed, and
who is allowed to send them. It decides nothing about what any operation means.

## Decision 2: the page is compiled in, and there is no file server

`ui/index.html` is `include_str!`-ed into the binary. There is no document
root, no path resolution, and no second route that takes a filename. A
traversal bug is not mitigated here; it is unrepresentable, because no request
field ever reaches a filesystem path.

The cost is that changing the page requires a rebuild. For a page that ships
with the daemon and is versioned with it, that is the correct trade: a UI that
can be swapped underneath a running daemon is a UI whose version nobody knows.

The page also loads no external resource — no font host, no CDN. A local
control panel that phones out on every load would make the daemon's network
behaviour depend on who is looking at it.

## Decision 3: loopback only, and three guards in front of the endpoint

The Unix socket is reachable only by a process that can open a file in
`.factory/`. A TCP port is reachable by anything that can make a request to
`127.0.0.1` — and on a developer's machine that includes **every page open in
their browser**. A daemon that can start agents and send tasks is exactly what
a hostile page would like to reach, and "it is only bound to localhost" is not
by itself an answer to that.

Three guards stand in front of the endpoint, and all three must pass:

1. **The listener binds loopback only.** `Ui::bind` refuses any address whose
   IP is not a loopback address, so the port cannot be published to a network
   by a configuration mistake.
2. **`Host` must name the loopback at the bound port.** A page can point a DNS
   name it controls at `127.0.0.1` — DNS rebinding — and reach this port
   carrying its own origin. The browser puts the attacker's name in `Host`,
   and anything but `127.0.0.1`, `[::1]` or `localhost` at the right port is
   refused with 421.
3. **`Origin`, when present, must be this server.** A cross-origin request
   always carries a foreign origin; refusing it refuses the cross-site request
   even where the browser would have sent it.

A fourth guard falls out of the media type: `POST /api` requires
`Content-Type: application/json`, which is not one of the three types a
cross-origin `fetch` may send without a preflight, and the preflight is an
`OPTIONS` this transport answers with 405. No CORS header is ever emitted.

**This is not authentication, and the module says so.** Anything that can
already run code as this user can reach the Unix socket too. The guards close
the one gap a TCP port opens that the socket did not: the *browser* acting on a
hostile page's behalf.

## Decision 4: the UI never stops the daemon

A port already in use — a second checkout, a stale process, a colleague's
daemon — must not stop a daemon whose actual job is the socket. `serve_with`
reports the failure and carries on without the UI.

The inverse would be worse than it looks: the UI is convenience, the socket is
the product, and a daemon that refuses to start because a browser page could
not be served would fail for a reason that has nothing to do with what it is
for.

## Decision 5: configuration is an environment variable, not a schema change

`FACTORY_UI_ADDR` selects the address, and `off` disables the UI. The default
is `127.0.0.1:7373`.

`.factory/config.yaml` is the instance's declared shape — scopes, agents,
harnesses — and ADR 0009 governs how that schema evolves. Where a transport
binds is a runtime property of one machine, not a property of the company the
configuration describes; two checkouts of the same instance on two machines
have every reason to differ. A typo is refused rather than silently defaulted,
because a UI quietly serving somewhere other than where the operator said is
worse than one that does not start.

A `config.yaml` key remains available later if an instance ever needs the
address to travel with it. It is deliberately not built now.

## Decision 6: the dialog shows the envelope, and validates nothing

Every acting control opens one dialog, and the dialog renders the exact
envelope it is about to POST — built from the same object that is sent, never
a second display copy. Two renderings of one payload is the same drift bug this
ADR's decision 1 avoids one layer up, and the preview is also the confirmation:
an operator about to cancel a run can read the run's id in the payload before
pressing the button.

**The page runs no validation of its own.** The error corpus is a deliverable in
this repository, `deny_unknown_fields` is on every payload, and the
`validation.*` codes already say precisely what is wrong with a request. So the
dialog sends what the operator typed, renders `error.code` and `error.message`
verbatim, and stays open. A second copy of those rules in JavaScript would be
the copy that goes stale, and it would answer in worse words.

Two consequences follow from that stance and are visible in the page:

- An empty required field is sent as an empty string rather than omitted, so
  the daemon answers about *that* field — "a progress note with no text is a
  log line, not a note" — instead of a generic "missing".
- Where an operation refuses a combination rather than a value —
  `task.send`'s "an agent or a session, never neither",
  `schedule.create`'s "an existing template or a new one" — the dialog offers a
  choice and sends only the fields that choice selects. It keeps the operator
  out of the refusal without knowing why the refusal exists.

Client-minted ids are minted as **UUIDv7**, the way `factory-cli`'s `ids::new_id`
does, because `create::list` orders by `created_at, id` and nothing on the
daemon side checks the version — a v4 id would sort arbitrarily inside a
timestamp tie and nothing would say so. `crypto.randomUUID()` is v4 and is
deliberately unused.

Two operations keep the dialog open because their answer *is* the point:
`scope.reconcile` returns the drift report, and `agent.attach_command` returns
the argv. The page does not attach to a session — a session belongs in a
terminal, not a browser tab — so it hands over the command line instead.

`task.done`, `task.fail` and `task.decision` are deliberately absent. They are
what a worker writes about its own run, not what an operator does to someone
else's; putting them on this surface would invite a human to close a run the
session is still inside.

## Consequences

- The daemon binds a TCP port by default where it previously bound only a Unix
  socket. That is a real change in exposure, bounded by decision 3, and it is
  the point of the feature rather than a side effect.
- One request per connection (`Connection: close`). Keep-alive would buy
  nothing for a page that issues a handful of requests every five seconds and
  costs a parser that has to agree with the client about framing.
- The page polls. There is no subscription on the wire — `task.wait` is a long
  poll for a single task — so the UI asks again every five seconds and says so
  in its own status bar rather than implying a stream it does not have.
- Reading the L3 overview costs one `agent.status` per scope, and the lease
  table one more per session, because `agent.status` filters on the caller's
  scope and returns leases only for a named session. The page counts its own
  calls and displays the number rather than hiding it.
- `server::dispatch_line` is now `pub(crate)`. It is the seam the two
  transports share, and it is the reason they cannot drift.

## Open item created by this ADR

The two grey levels are grey for one reason, and it is a short list: there is
no query for `task_events`, none for `task_decisions`, no factory-wide
`session.list`, and none for `delivery_attempts`. All four are reads against
tables that exist, and two of them have the reading function already written
with no caller. Until they exist, the served page can operate the factory but
cannot show why it did anything.
