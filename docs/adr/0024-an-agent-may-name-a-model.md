# ADR 0024: An agent may name a model, and what that does not mean

- Status: Accepted
- Date: 2026-09-11
- Owners: Business Factory
- Decided by: the human owner, 2026-09-11

## Context

Until now an agent was exactly four things: a name, a harness, a session cap,
and a lifetime. Which model ran was the harness's own business — `~/.pi/agent/
settings.json` carries a `defaultModel`, Claude Code carries its own, and
Factory read neither. That was a deliberate silence, and for a while it was the
right one: a field Factory cannot act on is a field that goes stale.

It stopped being right for one concrete reason. A scope that must run against a
**local** model — a llama.cpp server on this machine, no network, no per-token
cost — cannot say so. The harness's default is per *machine*, not per scope, so
a single default forces every scope on the box onto the same model. Two scopes
with different economics (a cheap local reviewer, a capable remote builder)
have no way to differ, and the operator's only lever is editing a global
harness setting before starting a session and changing it back afterwards.

## Decision 1: an agent may name a model, and the name is opaque

`model:` is an optional key on an agent. The value is a plain string, stored
exactly as written, and Factory never parses, splits, normalises or resolves
it.

**Factory names a model; the harness resolves it.** That boundary is the whole
decision. Pi accepts `--model <pattern>` in a `provider/id` form of its own; so
does Claude Code, with different names. A type here — an enum, a catalogue, a
regex — would be a second copy of every provider's model list, out of date the
week it was written, and its first user-visible act would be rejecting a name
the harness would have accepted. A model shipped tomorrow works today, without
a Factory release.

The only thing validated is that the value is not blank. An empty string would
reach the harness as an empty `--model`, which every harness refuses with a
worse message than the configuration error this produces.

## Decision 2: absent means "the harness's own default", not "no model"

`model:` omitted leaves an agent in exactly the state every agent was in before
this key existed: the harness picks, from its own configuration. The adapter
passes no `--model` flag at all in that case — deliberately not an empty one,
which would override the harness's default with nothing.

This is why the served page labels an agent with no model *Harness-Vorgabe*
rather than leaving the cell blank. A blank reads as "unknown"; the truth is
"decided elsewhere, on purpose".

## Decision 3: the declared model and the recorded model are different facts

`tasks.cost_model` already records the model a run actually used, as reported
by the harness after the fact (ADR 0021 decision 6). This key does not replace
it and must not be confused with it:

- `agents[].model` is an **instruction**, written by a human before the run.
- `tasks.cost_model` is an **observation**, written by the machine after it.

They can legitimately differ — a harness may fall back, a provider may alias, a
session may have been started before the configuration changed. That difference
is information about what actually happened, and collapsing the two into one
field would destroy it. Nothing in Factory compares them or treats a mismatch
as drift.

## Decision 4: this is an additive optional key

ADR 0009 governs configuration schema evolution. `model:` is optional with a
meaningful absence, so every configuration that loaded before this change loads
after it and means the same thing. No migration, no version bump, no
compatibility shim.

The key is on the *agent*, not the scope and not the instance, because that is
where harness already lives: a scope with two agents can already run two
harnesses, and there is no reason it should not run two models.

## Decision 5: the Pi adapter approves the workspace's own project config

Pi discovers project-local extensions and settings — `.pi/extensions/`,
`.pi/settings.json` — only in a directory a human has approved, and ignores
them everywhere else. That is the right default for a tool someone points at a
checkout they just cloned.

Under Factory the situation differs in one specific way: the workspace is not
somewhere the process wandered into, it is the canonical path of a scope an
operator declared in `.factory/config.yaml`, and Factory is the one placing an
agent there. So `PiAdapter` passes `--approve`.

Without it, project-level harness configuration is **inert under Factory**. A
provider registered by the scope's own extension never loads, a `model:`
naming that provider cannot resolve, and the failure surfaces as "model not
found" with nothing pointing at the real cause. Decision 1 would be usable only
for models the machine already knows globally — which is exactly the per-machine
default this ADR exists to escape.

What the flag grants is bounded by what was already granted: the session about
to start has tool access in this very directory. A workspace that cannot be
trusted to register a provider is a workspace no agent should have been started
in.

Claude Code has no equivalent flag and needs none; this decision is Pi's alone.

## What was exercised, and what was not

`PiAdapter` was verified end to end against a live Pi: the flag reaches the
process, and a local llama.cpp provider resolves through it.

`ClaudeAdapter` was **not**. `herdr agent start ... -- ARGS` forwards its
trailing arguments the same way for every kind, and `claude --model` is a
documented flag, so the path is sound by construction — but what this
repository's tests cover for Claude Code is the argv the adapter builds, not a
session started with it. Said here rather than left for someone to assume.

## Consequences

- A scope can be pinned to a local model while its neighbour runs a remote one,
  without touching any global harness setting.
- A name Factory does not understand is a *harness* error, surfaced through
  `herdr agent start` failing, not a configuration error at load time. The
  failure is therefore late — at session start, not at `factory doctor` — and
  that is the price of not keeping a catalogue.
- Whether the named model resolves also depends on the harness having the
  provider loaded at all. For Pi, a provider registered by a project-local
  extension is only discovered in a directory Pi trusts; naming a model that
  such an extension registers is not enough on its own.
