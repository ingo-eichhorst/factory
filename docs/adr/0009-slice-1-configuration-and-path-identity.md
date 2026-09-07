# ADR 0009: Slice-1 prerequisites — YAML library, schema evolution, and path identity

- Status: Accepted
- Date: 2026-09-07
- Owners: Business Factory

## Context

Slice 1 of the implementation backlog listed three unresolved decisions: the
YAML library, the schema-evolution policy, and canonicalization behaviour for
paths that do not yet exist. All three block the first line of Rust, so this
ADR resolves them together.

Against the company design baseline, this ADR implements `.specs/design.md`
§2.1 (the scope configuration file and its canonical-path requirements) and
§2.2 (the agent block, the `agents` list, and `lifetime`). It amends no baseline
section; it makes the parsing, validation, and path-comparison behaviour those
sections assume explicit enough to implement. Per ADR 0010, this reference is
stated rather than left implicit.

Two pieces of evidence were gathered first, and both changed the answers:

- The seven live `.factory/config.yaml` files were surveyed. All carry
  `version`, `scope`, and `agent`. Two (`assistant`, `model-lab`) additionally
  carry a `runtime:` block that appears nowhere in the design schema.
- The filesystem behaviour of the production machine was measured rather than
  assumed.

## Decision 1: use `serde-saphyr` as the YAML library

`serde_yaml`, the historical default, was archived and marked deprecated on
2024-03-25. Its successors are not equivalent. Measured from crates.io on
2026-09-07:

| Crate | Version | Last release | Downloads (90d) | Verdict |
|---|---|---|---:|---|
| `serde-saphyr` | 1.2.0 | 2026-08-30 | 4.66M | **Chosen** |
| `yaml-rust2` | 0.12.0 | 2026-08-18 | 14.8M | Parser only, no Serde derive |
| `serde_norway` | 0.9.42 | 2024-12-21 | 3.74M | Fork that itself went stale |
| `serde_yml` | 0.0.13 | 2026-05-27 | 6.21M | Still `0.0.x`; now unmaintained |
| `serde_yaml` | 0.9.34+deprecated | 2024-03-25 | 89.3M | Deprecated |

`serde-saphyr` is the only candidate that is simultaneously past 1.0, actively
released, Serde-integrated, and free of the `unsafe-libyaml` dependency that
`serde_norway`, `serde-yaml-ng`, and `serde-yaml-bw` all carry. It passes the
full `yaml-test-suite`.

The deciding capability is error reporting. Slice 1 requires that malformed
configuration "fail with the file and corrective action". `serde-saphyr`
reports line and column with snippet rendering, for example
`error: line 3 column 23: invalid here, validation error: ...`. A library
without position information could not meet that criterion.

### Binding constraint on the config model

`serde-saphyr` cannot use `Spanned<T>` inside untagged or internally tagged
enum variants. Therefore the `agent:` shorthand and the `agents:` list
(design §2.1–2.2) **must not** be modelled as an untagged enum. Model them as
two optional fields validated after parsing.

### Addendum 2026-09-08: the API was measured, and one claim was wrong

Everything above about `serde-saphyr` was inferred from crates.io metadata —
version, release date, download counts — not from the API. Before implementation
began, a spike compiled against the real crate. Four claims held; one did not.

| Claim | Result |
|---|---|
| `Spanned<T>` usable on struct fields, and inside `Option` and `Vec` | Holds, all three |
| Unknown top-level keys ignored, so `runtime:` passes over | Holds — rule 2 below works as written |
| Errors carry line and column with a rendered snippet | Holds: `error: line 4 column 3: … ` plus a source excerpt |
| `Spanned<T>` exposes the position as `.span` | **Wrong.** The fields are `value`, `referenced`, and `defined`; the position is `.defined`, a `Location` with `line` and `column` |

One hedge also resolves in our favour. This ADR called `deny_unknown_fields`
"undocumented" and planned to enforce rule 3 by hand if it failed. It works,
including on inner mappings, and produces
`unknown field `max_sesions`, expected one of name, max_sessions`. Rule 3 is one
attribute, not hand-rolled code.

One constraint the spike added: error `Display` renders the source as `<input>`
and offers no way to name the file. Slice 1 requires every failure to name the
file, so `factory-config` **wraps** parse errors with the origin path rather than
forwarding them. That is why `ConfigError` is its own type with a `Location`
carrying a `PathBuf`, rather than a newtype over the library error.

This is the better design regardless: an untagged enum reports failure as
"data did not match any variant", whereas post-parse validation can say
"`agent:` and `agents:` are both set in <file>; use one or the other", which is
what the acceptance criteria demand.

## Decision 2: schema-evolution policy

The `runtime:` block forced this decision rather than leaving it to taste.
`scripts/ensure_assistant_agents.py:55` reads `document.get("runtime")`,
validates its own independent `runtime.version` at line 62, and expands
`${REPO_ROOT}` and `${HOME}` inside it at line 30. The block is owned by a
different tool that happens to share the file.

The policy follows:

1. **Factory owns exactly `version`, `scope`, and `agent`/`agents`.**
2. **Unknown top-level keys are preserved and ignored.** They belong to other
   tools. `AGENTS.md` already forbids overwriting human-maintained
   configuration, and rejecting them would make the `assistant` and `model-lab`
   scopes unloadable today.
3. **Unknown fields inside Factory-owned mappings are rejected.** A typo such
   as `max_sesions: 4` must fail loudly, never silently mean `1`. Strictness
   belongs where Factory owns the meaning, not at the file level.
4. **`version` gates only the Factory-owned blocks.** An unsupported version is
   an actionable error naming the file and the supported range, never a guess.
5. **Within a version**, adding optional fields with defaults is permitted.
   Renaming a field, removing one, or changing the meaning of an existing one
   requires a version bump.
6. **Factory never rewrites a scope config.** Migration is an explicit operator
   command, never a side effect of reading a file.

Rule 2 has a useful consequence: because the top level needs pass-over
behaviour anyway, `serde-saphyr`'s undocumented support for
`deny_unknown_fields` is not load-bearing. Inner strictness (rule 3) can be
enforced by hand if the attribute turns out to be unsupported.

## Decision 3: path identity and canonicalization

Slice 1 asked about paths that do not yet exist. Measuring the machine showed
that is the smaller of two problems, and not the one that bites.

### 3a. A canonical path is not a unique identity on this machine

The production volume is APFS in its default case-insensitive configuration.
Measured on 2026-09-07:

```
/Users/factory/business-factory/projects/factory   → dev 16777230, ino 3947391
/Users/factory/business-factory/PROJECTS/factory   → dev 16777230, ino 3947391
realpath strings identical?  False
```

The two names are the same directory, and `realpath` — like Rust's
`std::fs::canonicalize` — resolves symlinks but does not normalise case. String
comparison therefore reports "different" where the filesystem reports "same".

**Policy.** The canonical path remains the *stored* identity: it is what the
Slice 2 unique constraint indexes and what Slice 3 updates on a move. In
addition, every *runtime* aliasing check — lease acquisition and descendant
tests — compares `(st_dev, st_ino)`, not strings.

### Correction 2026-09-08: the mechanism above is wrong, the policy is right

The measurement in §3a was taken with a **lexical** resolver. Python's
`os.path.realpath` and the shell's `realpath` resolve symlinks without asking
the filesystem for a directory's true spelling. Rust's `std::fs::canonicalize`
does ask, and **normalises case**. Measured on the same machine on 2026-09-08:

```text
rust  canonicalize(".../PROJECTS/factory/CRATES") -> ".../projects/factory/crates"
rust  canonicalize(".../projects/factory/crates") -> ".../projects/factory/crates"
      equal strings? true

python realpath(".../PROJECTS/factory/CRATES")    -> ".../PROJECTS/factory/CRATES"
      equal strings? False      same inode? True
```

So the sentence "`realpath` — like Rust's `std::fs::canonicalize` — resolves
symlinks but does not normalise case" is false of Rust. Two case-variant paths
canonicalized **at the same instant** by Factory produce equal strings, and the
defect as described does not arise that way.

**The policy survives, for a different and better reason.** Factory does not
compare two paths at one instant; it stores a string in SQLite and compares it
against the world later. Across that gap the defect is real:

```text
registered   .../factory-case-probe/Workspace     inode 4132576
operator runs `mv Workspace workspace`
current      .../factory-case-probe/workspace     inode 4132576

stored string == current string ?  false
same (dev, ino) ?                  true
```

A case-only rename preserves the inode, changes the on-disk spelling, and leaves
the stored canonical path disagreeing with a fresh canonicalization of the same
directory. That is the defect that reaches Slices 3, 6, and 8 — a genuine
descendant looking like a non-descendant, and design §6 then refusing legitimate
parent-to-child work.

Two rules follow, where the original ADR gave one:

1. **Every runtime aliasing question compares `(st_dev, st_ino)`** — unchanged.
2. **A stored path is re-canonicalized before it is compared**, never string-matched
   as read from the database. The probe's last line shows why this works: the
   stored `Workspace` still resolves, to `workspace`.

Neither rule subsumes the other. Inode comparison answers "are these two paths
the same directory *now*", and fails to notice that a directory was deleted and
recreated at the same path — a new inode for what an operator means as the same
workspace. Re-canonicalization answers "does this registration still point
where it did", and fails when the path no longer exists. Slice 3's reconcile and
Slice 6's leasing need both, and should not be written as if either alone were
identity.

**Consequence for testing.** A case-variant test that resolves two spellings at
one instant now passes trivially in Rust and proves nothing. The test must span
time: resolve, rename changing only case, then resolve again. This was found
while implementing `factory-paths`, whose test does exactly that.

Inode is deliberately not the primary key. A rename preserves the inode, so
move-and-reconcile keeps working; but a delete-and-recreate at the same path
(removing and re-adding a worktree, restoring from backup) yields a new inode.
Inode is a runtime alias check, not durable identity.

**Cross-slice impact.** This is not only a lease problem. Descendant checks use
the same comparison, so a case-variant prefix can make a genuine descendant look
like a non-descendant — and the delegation rule in design §6 would then deny
legitimate parent-to-child work. The defect reaches slices 3, 6, and 8.

### 3b. Paths that do not yet exist

The commands that accept a path were enumerated:

| Command | Must the path exist? |
|---|---|
| `factory init --root` | **No** — this is the only case |
| `factory scope add --path` | Yes; registration validates a config file that lives in the directory |
| `factory agent start --workspace` | Yes; Slice 6 rejects nonexistent paths |
| `factory task send --workspace` | Yes; design §2.4: "the workspace itself must already exist" |

Because exactly one command can name a non-existent path, **no lexical
normalization code is needed.** `factory init` creates the directory, then
canonicalizes the real directory, then verifies the result. Everywhere else,
canonicalization operates on something that exists.

The general rule: if a path cannot be canonicalized, it cannot be registered or
leased. Fail with an actionable error rather than storing a normalized guess.
A guessed path would be compared against real ones later, which is precisely how
the aliasing defect in 3a arises.

### 3c. Explicitly out of scope: TOCTOU

Canonicalize-then-act is racy; a symlink can be swapped between the check and
the use. The threat model rules this out of scope: design §6 states all agents
run under one trusted macOS account and that scope boundaries are "cooperative
policy, not hard security isolation", which ADR 0001 repeats.

This is stated rather than left implicit because design §2.1 requires rejecting
"symlink escapes", which reads like a security control. It is not one. It is a
correctness control that prevents one directory being leased twice under two
names. It must not be cited as a defence against a hostile local process.

## New open item created by this ADR

`${HOME}` and `${REPO_ROOT}` interpolation is a feature of
`ensure_assistant_agents.py`, not of Factory. **Factory version 1 does not
expand variables in configuration values.** If it ever should, that needs its
own specification — which variables are defined, what an undefined variable
does, whether expansion recurses — and it is a new Slice 1 item rather than
something folded silently into the YAML-library choice.

## Consequences

- Slice 1 can begin. The remaining work is implementation, not decision.
- The config model is two optional fields plus post-parse validation, not an
  untagged enum, and it carries spans for error messages.
- Slices 3, 6, and 8 gain a `(dev, ino)` comparison alongside canonical-path
  storage. Slice 6's acceptance criterion was widened from "aliases through
  symlinks" to include case variants, because the previous wording could be
  satisfied while still shipping the reproduced defect.
- A case-sensitive volume would hide the 3a defect in testing. Fixture tests
  must cover a case-variant path explicitly rather than relying on the
  developer's filesystem to expose it.
