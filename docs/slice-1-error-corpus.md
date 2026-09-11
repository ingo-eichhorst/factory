# Slice 1 — configuration diagnostic corpus

## Amendment 2026-09-08: the document shape changed (ADR 0015)

A scope is no longer a directory holding its own `.factory/config.yaml`. The
Factory instance has one configuration listing every scope:

```yaml
version: 1
instance:
  id: <uuid>
  name: business-factory
scopes:
  - id: <uuid>
    name: irrlicht
    path: projects/irrlicht
    git: https://github.com/example/irrlicht.git   # optional
    agent:                                          # or `agents:`
      name: Irrlicht Agent
      harness: pi
```

**Everything below still applies**, with three adjustments:

1. **Scope-level errors name the scope as well as the file.** A problem in one
   entry of a list must say which entry, or the reader has to count. Prefix the
   summary accordingly: ``error: scope `irrlicht` sets both `agent` and
   `agents`; a scope uses one or the other``. The `-->` location, the `note:`
   lines, and the `help:` are unchanged in form.

   **One documented exception: unknown-field and missing-field errors (case 10,
   case 9) do not name the scope.** `deny_unknown_fields` rejects the document
   during deserialization, before any scope entry has been constructed, so at
   that moment no name exists to print. Recovering one would mean re-scanning
   unparsed text to find entry boundaries.

   This is accepted rather than worked around, because those errors already
   carry the file and an exact line and column pointing at the offending key —
   which is what a reader needs to act. The scope name is a convenience when the
   location is a whole entry, and redundant when it is a single line. Revisit it
   only if a real message is found to be ambiguous in practice.
2. **Case 13 (duplicate scope IDs) becomes an intra-file check** and is
   therefore stronger: it runs on every load rather than only when a caller
   remembers to compare two configs. Both locations are in the same file.
   `validate_unique_ids` as a separate cross-file entry point disappears.
   A companion case is added: **two scopes may not share a `path`** either, with
   the same shape of message — sharing a path means two scopes claiming one
   directory, which the Slice 6 lease would later have to reject anyway, and
   catching it at load names both entries instead of one lease failure.
3. **The top-level key scan of case 1 now searches within a scope entry**, whose
   keys are indented. The "first character is not whitespace" rule was correct
   for the old shape and is wrong for this one. Anchor instead on the entry's
   own indentation: within the byte range of the scope entry, find a line whose
   indentation equals that of the entry's other keys and which begins `agent:`
   or `agents:`. The fallback is unchanged and still matters — on anything
   ambiguous, use the value-node location rather than guessing.

`git` is validated as present-or-absent only. Version 1 does not contact a
remote, parse a URL, or verify that a checkout matches the reference; a scope
entry is a record, and checking it against the world is `factory doctor`'s job.

This document is the specification for `factory-config` error output. Slice 1's
acceptance criteria are almost entirely *diagnostic* criteria — "naming the file
and both keys", "both definitions identified", "fails with the supported set",
"fail with the file and corrective action". The parser is the easy half; this
table is the deliverable.

Every row below has a fixture under
`crates/factory-config/fixtures/` and a test asserting the rendered message
**exactly**. A change to a message is a change to this file first.

## Rendered form

Errors render in the rustc convention, which readers already know:

```text
error: <summary>
  --> <file>:<line>:<column>
  note: <a second location that is part of the same problem>
  help: <the corrective action>
```

Rules:

- `--> ` always carries the **absolute path of the file**, never `<input>`.
  `serde-saphyr` renders the source as `<input>`; the file name is supplied by
  `factory-config`, which is why parse errors are wrapped rather than forwarded.
- `note:` appears once per additional location. A problem about two places in a
  file (both `agent` and `agents` set; two agents sharing a name) names both.
- `help:` is present on every error and states an action, not a restatement of
  the problem. "expected one of …" is a summary; "remove `agent:` …" is a help.
- Line and column are 1-based, as `serde-saphyr` reports them.

## Cases that must load

| Fixture | What it proves |
|---|---|
| `valid/root-shorthand.yaml` | The `agent:` shorthand yields exactly one agent |
| `valid/multi-agent.yaml` | The `agents:` list yields several, order preserved |
| `valid/unknown-toplevel.yaml` | A `runtime:` block is ignored, not rejected (ADR 0009 rule 2) |
| `valid/defaults.yaml` | Omitted `max_sessions` is `1`; omitted `lifetime` is `permanent` |

`valid/unknown-toplevel.yaml` is a copy of the real `assistant` scope config,
which carries a `runtime:` block owned by `ensure_assistant_agents.py`. If this
fixture ever fails to load, two live scopes have become unloadable.

## Cases that must fail

Column numbers are what `serde-saphyr` reports for the offending node; where a
fixture's exact column is uncertain the test asserts the line and the message
text, never a guessed column.

### 1. Both `agent` and `agents`

```yaml
agent:
  name: A
agents:
  - name: B
```

```text
error: `agent` and `agents` are both set; a scope uses one or the other
  --> <file>:1:1
  note: `agent` shorthand defined here
  note: `agents` list defined at <file>:3:1
  help: keep `agents:` and delete the `agent:` block, or keep `agent:` and delete `agents:`
```

Both keys are named and both are located, per the acceptance criterion. This is
also why ADR 0009 forbids modelling these as an untagged enum: an untagged enum
can only say "data did not match any variant".

**Both locations must be the line of the key itself.** `Spanned<T>` locates the
spanned *value* node, so for a key whose value is a block mapping or a block
sequence it reports the first nested line — one below the key — and
`serde-saphyr` exposes no way to ask for the key's own position. Reported
naively, this error points at `  name: A` while talking about `agent:`, and a
reader who looks where the arrow points sees a name field rather than the
conflict.

Recover the key's line by scanning **within the scope entry**. `serde-saphyr`
reports a spanned entry's location at that entry's first key, which gives both
the entry's starting line and the indentation its keys share. An entry's line
range runs from there to the line before the next entry starts, or to the end of
the file for the last one. Inside that range, look for a line whose indentation
equals the entry's key indentation and which begins `agent:` or `agents:`.

Requiring both the range and the exact indentation is what makes the scan safe.
The range keeps a sibling entry's `agent:` from being matched — every scope has
one, so without it the scan would find several. The indentation keeps a nested
`agents:` from being matched, such as the one inside the live `assistant`
scope's `runtime:` block.

If the scan finds anything other than exactly one match, **fall back to the
value-node location** rather than guessing. A diagnostic that is one line off is
a papercut; one that points at an unrelated line because a comment contained the
word `agent:` is a defect.

This applies to any error naming a Factory-owned key inside a scope entry.

### 2. Neither `agent` nor `agents`

```text
error: no agent is defined; a scope configures at least one agent
  --> <file>:1:1
  help: add an `agent:` block, or an `agents:` list with at least one entry
```

### 3. Two agents sharing a name

```text
error: two agents in this scope are both named `assistant`
  --> <file>:8:5
  note: first defined at <file>:5:5
  help: agent names address a recipient in `factory task send`, so they must be unique within a scope; rename one
```

The help states *why* the rule exists, because a duplicate name is not obviously
wrong until you know the name is an address.

### 4. Unsupported `lifetime`

```text
error: unsupported lifetime `ephemeral`
  --> <file>:7:15
  help: supported lifetimes are `permanent` and `temporary`; `permanent` is the default and may be omitted
```

### 5. Unsupported `harness`

```text
error: unsupported harness `bash`
  --> <file>:6:12
  help: supported harnesses are `claude-code`, `opencode`, and `pi`
```

### 6. Harness `claude` — the near-miss

```text
error: unsupported harness `claude`
  --> <file>:6:12
  help: did you mean `claude-code`? supported harnesses are `claude-code`, `opencode`, and `pi`
```

A separate case because `projects/awesome-herdr/.factory/config.yaml` sets
exactly this today. See `HARNESS_TABLE` in `src/harness.rs` and the open item in
the backlog.

### 7. Invalid UUID

There are two, because there are two kinds of identity in the file:

```text
error: scope `irrlicht`'s `id` is not a UUID: `not-a-uuid`
  --> <file>:12:9
  help: generate one with `uuidgen`; a scope ID is permanent identity and must not be reused between scopes
```

```text
error: `instance.id` is not a UUID: `not-a-uuid`
  --> <file>:4:7
  help: generate one with `uuidgen`; the instance ID identifies this Factory instance and never changes
```

The instance error cannot name a scope, because the fault is above all of them.
That is the reason it reads differently rather than an inconsistency to fix.

### 8. Unsupported version

```text
error: unsupported configuration version `2`
  --> <file>:1:10
  help: this build of Factory supports version 1
```

Never a guess and never a silent upgrade, per ADR 0009 rule 4.

### 9. Missing required field

```text
error: missing field `scope`
  --> <file>:1:1
  help: add a `scope:` block with an `id` and a `name`
```

The `1:1` here is a property of the fixture, not a rule. `serde-saphyr` reports
a missing field at the last top-level key it processed, so this file — which
contains only `version: 1` — reports line 1. A file with more keys would report
a later line. Assert the message text and let the location follow the input.

### 10. Unknown field inside a Factory-owned mapping

```yaml
agent:
  name: A
  max_sesions: 4
```

```text
error: unknown field `max_sesions` in `agent`
  --> <file>:3:3
  help: expected one of `harness`, `lifetime`, `max_sessions`, `model`, `name`; `max_sesions` looks like a typo for `max_sessions`
```

This is ADR 0009 rule 3, and it is the rule that earns its keep: without it,
`max_sesions: 4` silently means `max_sessions: 1` and an agent quietly runs at a
quarter of its configured parallelism. Strictness applies *inside* Factory-owned
mappings only — the top level stays permissive so `runtime:` survives.

### 11. `max_sessions: 0`

```text
error: `max_sessions` is 0, so this agent could never start a session
  --> <file>:6:17
  help: use at least 1, or remove the agent
```

### 12. Empty file

```text
error: the file is empty
  --> <file>:1:1
  help: a scope configuration needs at least `version`, `scope`, and one agent
```

### 13. Two scopes sharing an ID, or a path

Both run inside `parse`, on every load, and each is reported once for the pair:

```text
error: two scopes share the ID `c5fbc985-656a-4219-8614-fcaeaddbf103`
  --> <file>:12:9
  note: also used by scope `factory` at <file>:31:9
  help: a scope ID is permanent identity; if this entry was copied, generate a new ID with `uuidgen`
```

```text
error: two scopes share the path `projects/irrlicht`
  --> <file>:14:11
  note: also claimed by scope `irrlicht-fix` at <file>:33:11
  help: two scopes cannot own one directory; give one of them a different path
```

This used to be the single Slice 1 check that spanned files, performed by a
separate `validate_unique_ids` a caller had to remember to call. Since ADR 0015
put every scope in one file it is an ordinary intra-file check, which makes it
**stronger**: it now runs on every load rather than only when someone compares
two configs.

The path check is the companion. Two scopes claiming one directory would
otherwise surface much later as a Slice 6 lease failure naming one session;
caught here it names both entries and the file.

## What validation must not do

`factory-config` performs **no** filesystem writes of any kind — no cache, no
lockfile, no log, not even in a temporary directory. Slice 1's criterion is
"validation writes nothing at all", which is the strictest form of the design §4
rule and is asserted directly in `tests/no_writes.rs`.

Validation also does not expand `${HOME}` or `${REPO_ROOT}`. That expansion
belongs to `ensure_assistant_agents.py` and is explicitly out of scope per
ADR 0009's closing open item. A configuration value is used verbatim.
