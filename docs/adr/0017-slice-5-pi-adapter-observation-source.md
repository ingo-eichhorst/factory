# ADR 0017: Slice-5 prerequisites — Herdr is the observation source for Pi

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory
- Supersedes, for Pi only: ADR 0011 decisions 1 and 5

## Context

ADR 0011 made Irrlicht the session observation source, on the strength of its
Pi transcript parser. Two rounds of measurement against the live machine have
since changed the picture for Pi specifically.

The first (recorded in ADR 0011's open item 1) found that Irrlicht resolves the
Herdr pane for every adapter except Pi — 0 of 3 — and a transcript path for one
of three, so the correlation key ADR 0011 assumed does not exist on Irrlicht's
side for Pi.

The second is why this ADR exists. `herdr agent explain` reports, for a Pi pane:

```text
agent: pi
state: idle
screen_detection_skip_reason: full_lifecycle_hook_authority
```

Herdr does not infer Pi's state from the screen. It receives it. Pi loads
`~/.pi/agent/extensions/herdr-agent-state.ts`, a file whose own header says
"installed by herdr", which opens `HERDR_SOCKET_PATH` and reports
`{ state, message, seq }` on lifecycle events.

That inverts the comparison. Irrlicht parses a transcript to *infer* state;
Herdr is *told* it by the harness.

### Why Irrlicht cannot resolve a Pi pane — and why that is fixable

A third measurement found the cause, and it matters for how permanent this
decision is. Irrlicht reads `HERDR_PANE_ID` from a process's environment. That
works for the harnesses where it succeeds and fails for the one where it does
not:

```text
claude  pid 3760   HERDR_ENV=1  HERDR_PANE_ID=wD:p1  HERDR_SESSION=factory
pi      pid 63243  0 environment variables visible to ps
```

Pi overwrites its `argv`/environment region — `ps` shows a bare `pi` with no
arguments and no environment. Any tool reading the pane from the process
environment therefore finds nothing for Pi, while Herdr knows the pane because
it created it and the state extension reports over a socket keyed by it.

**So this is a gap in how Irrlicht correlates, not a limit on what Irrlicht can
observe.** Irrlicht already sees every Pi session and states it correctly; it
simply cannot say which pane one belongs to. Irrlicht is a product of this
company, and the gap is closable — by asking Herdr for the mapping, or by
matching on the transcript path it already records for some sessions.

If it closes, decision 1 should be revisited. What would remain then is the
weaker half of the argument: hook-reported state is more direct than
transcript-inferred state. That is a preference, not a blocker, and it would not
on its own justify preferring one source over the other.

## Decision 1: for Pi, Herdr is the observation source

`observe()` for Pi reads `herdr agent get <pane-id>` and uses `agent_status`.

Herdr's vocabulary is `idle | working | blocked | done | unknown`, which is
closer to Factory's task states than Irrlicht's `ready | working | waiting`. It
already distinguishes `blocked` — the question-and-permission signal that Slice 5
lists as a risk — and `done`.

Irrlicht is not dropped. It remains the observation source for `claude-code`
(Slice 10), where its pane ID is present and its parser is the only source, and
it remains the source of metrics — tokens, context pressure, cost — that Herdr
does not report. For Pi it is *enrichment*, joined on the transcript path, and
never the state of record.

ADR 0011 is right that observation belongs behind a plugin contract and that the
direction of truth never inverts. What it got wrong, for one harness, is which
product holds the better signal.

## Decision 2: the state extension is a precondition, and its absence is detectable

Authoritative state exists only because Pi loads the Herdr extension. `herdr
agent start` does not inject it: agent arguments are passed explicitly after
`--`, and the live `assistant` configuration passes
`--extension ${HOME}/.pi/agent/extensions/herdr-agent-state.ts` by hand.

So a Pi session started without it degrades silently to screen detection, which
is exactly the heuristic Slice 5 was meant to avoid.

**The adapter starts Pi with the extension, and verifies afterwards.** The check
is `screen_detection_skip_reason: full_lifecycle_hook_authority`, a line in the
plain-text output of `herdr agent explain`.

*Corrected during implementation:* an earlier draft of this ADR described the
check as `screen_detection_skipped: true` **together with** that reason, both
from `agent explain`. They come from different commands — `screen_detection_skipped`
is a JSON field of `herdr agent get`, the reason is a text line of
`herdr agent explain` — so the check as written could not be performed as
described. The reason line alone is sufficient and is what the adapter uses:
`skipped` says only that the screen was not read, while the reason says *why*,
and only one reason means hook authority.

If it is absent the adapter records the session as
**observation-degraded** and falls back to manual confirmation. It does not
accept screen-detected state as if it were hook-reported, because the two have
different reliability and only one of them is worth building at-most-once
delivery on.

## Decision 3: correlation runs pane → transcript → session

Factory records the Herdr `pane_id` when it starts a session. Everything else
derives from it:

```text
pane_id  --herdr agent get-->  agent_session.value (transcript path)
                               --> the Pi session UUID, parsed from the filename
                               --> optional Irrlicht enrichment, joined on that path
```

The pane ID is the primary key because it is the one identifier Factory itself
chose and recorded. **Working directory is never a join key for Pi.** Two
sessions of one agent may share a workspace — `assistant` is configured for
exactly that with `max_sessions: 2` — and joining on it would attribute one
session's state to the other. A wrong observation is worse than none, because it
looks like an answer.

## Decision 4: how Herdr's states map, and what is deliberately not mapped

| Herdr `agent_status` | Session state | Task state |
|---|---|---|
| `working` | `running` | `running` |
| `idle` | `running` | no change |
| `blocked` | `running` | `blocked`, reason from the adapter |
| `done` | `running` | no change |
| `unknown` | no change | no change |

Three points where the obvious mapping is wrong:

- **`idle` does not mean the task finished.** It means the harness is waiting for
  input, which is equally true before a task is delivered and after one
  completes. Task completion is recorded from the result the agent reports, not
  inferred from the harness going quiet.
- **`done` is Herdr's word for a finished *turn*, not a finished task.** A task
  may span many turns. Mapping it to task `done` would close tasks at the first
  pause.
- **`unknown` changes nothing.** ADR 0011's open item 2 already requires that an
  unrecognised state be treated as no observation; this extends it to a state
  that is recognised and explicitly means "no information".

Session state stays `running` for every live status because these describe the
harness's activity, not the session's existence. A session becomes
`disconnected` or `failed` when the pane is gone, which is a different question
asked of `herdr agent list`.

## Decision 5: the operator performs terminal actions; the adapter records

Slice 5 is a manual proof. `start`, `send`, `interrupt`, and `stop` are
documented operator procedures using `herdr agent start`, `herdr agent prompt`,
`herdr agent send-keys`, and pane closure. The code implements `observe()` and
the records.

This is not timidity. Design §5 requires a delivery attempt to be recorded
*before* input reaches the PTY, and at-most-once delivery to survive an
ambiguous crash. Automating the write before the observation side is proven
would mean building that guarantee on a state source that has not yet been
demonstrated. Slice 10 automates what this slice proves.

## Consequences

- Slice 5 delivers an adapter contract plus a Herdr-backed implementation for
  Pi, not an Irrlicht-backed one.
- The Herdr CLI becomes a dependency of the adapter. It is invoked as a
  subprocess and its JSON parsed; version drift is a real risk, and the adapter
  records the `herdr --version` it observed (0.8.0 today) with each session.
- Scope configuration for a Pi agent must carry the state extension. The live
  `assistant` scope does; a scope registered without it gets degraded
  observation, and `factory doctor` should report that in Slice 10.
- ADR 0011's open item 2 (version skew) applies unchanged to both sources.

## Open item created by this ADR

Herdr pane identifiers are the primary correlation key, and their stability
across a Herdr restart is unverified. Slice 9's restart drills must establish
whether `pane_id` survives, and if it does not, what Factory re-correlates on.
The transcript path is the candidate, since it outlives the pane.
