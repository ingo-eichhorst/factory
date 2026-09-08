# ADR 0015: Scopes are entries in a central registry, not directories carrying a config file

- Status: Accepted
- Date: 2026-09-08
- Owners: Business Factory
- Decided by: the human owner, 2026-09-08

## Context

Until now a scope was defined by a file inside it. Design §2.1: "A directory is
a registered Factory scope only when it contains its own `.factory/config.yaml`."
Seven such files exist today, one per scope.

The owner raised the objection that this pollutes the project repositories, and
that it may not be necessary. It is a fair objection and it is worse than it
first appears, because several registered scopes are *foreign* repositories.
`projects/awesome-herdr` is a GitHub repository with its own remote; registering
it means either committing Factory's configuration into someone else's project
or carrying a permanently dirty working tree.

The decision: **keep Factory's own state in the Factory instance, and let the
central configuration hold a reference to each project instead.**

## Decision

A Factory instance is a directory containing `.factory/`. Its
`.factory/config.yaml` declares the instance and every scope registered with it:

```yaml
version: 1

instance:
  id: 1c54efcd-2a40-4b2c-9a8f-02740f208a73
  name: business-factory

scopes:
  - id: e8361a02-0285-4368-aa51-63b262b48613
    name: irrlicht
    path: projects/irrlicht          # relative to the instance root
    git: https://github.com/…        # optional; the project's own repository
    agents:
      - name: Irrlicht Agent
        harness: pi
        max_sessions: 1
```

**No `.factory/` directory is created inside a registered project.** A project
directory holds the project. Factory's bookkeeping stays in the instance.

A scope names its location in one of two ways, and the distinction is the point
of the change:

- **`path`** — the project's files live inside the Factory instance's own
  repository. This suits scopes that have no repository of their own, such as
  `projects/ingo` today.
- **`git`** — the project is its own repository, checked out at `path`. Factory
  records the reference and touches nothing inside it. A project without a
  repository can always be adopted into the Factory instance's repository
  instead, which is what `path` means.

## Why this is more than tidiness: it deletes a class of defects

The old rule made scope identity *discoverable*, and everything discoverable
must then be defended against being discovered wrongly. Three guards existed
purely for that reason, and all three now describe nothing:

- **The copied config in a Git worktree.** Design §4 required that workspaces
  are "never discovered as scopes merely because a Git checkout contains a
  copied `.factory/config.yaml`", and Slice 3 carried an acceptance criterion
  for ignoring exactly that. A worktree is a checkout of a project repository;
  since no Factory file lives there any more, there is nothing to copy and
  nothing to mistake.
- **Recursive discovery.** Slice 3's "normal operation never recursively
  discovers scopes" was a rule against a capability that no longer exists. The
  registry is a list; reading it is not a search.
- **Fixture configs indistinguishable from real ones.** Two days ago the Slice 1
  fixtures were given a `fixture: true` marker, and it was then found to protect
  nothing, because Factory ignores unknown top-level keys by policy. Slice 3
  gained a criterion to exclude those paths from scanning. With no scanning,
  a fixture is just a file that nothing reads unless a test passes it in.

A design in which the hazard cannot arise is better than one that lists the
hazard and asks each future slice to remember it. This is the same reasoning
that replaced §4's denylist of protected files with the single `.factory/`
allowlist.

## What stays where it is: `AGENTS.md`

`AGENTS.md` remains in the scope directory. It is not Factory's file — design §4
already forbids Factory writing it — and centralizing it would cost more than it
saves.

**Measured rather than assumed:** Pi discovers and loads it natively. `pi --help`
lists `--no-context-files, -nc   Disable AGENTS.md and CLAUDE.md discovery and
loading`, so the behaviour is on by default and can only be turned off. Moving
`AGENTS.md` into the instance would break context loading that works today, for
every session started outside Factory. `awesome-herdr` also carries its own
`AGENTS.md` and `CLAUDE.md` as legitimate project documentation, written for
that project rather than for Factory.

This is not an exception to the decision. `AGENTS.md` is a shared convention
several tools read; Factory's own configuration is not. Removing the latter from
project repositories is the goal, and the former was never the problem.

For a scope that should carry no such file, a scope entry may name a context
file kept under the instance's `.factory/`. That is an option on the entry, not
a replacement for the convention.

## Consequences

- Design §2.1, §2.5, §4, and §7 change. The sentence defining a scope by a local
  file is replaced by a registry entry, and §4's paragraphs about copied configs
  are deleted rather than reworded, because they describe a situation that can
  no longer occur.
- **Slice 1 is reworked.** `factory-config` parses one scope per file today; it
  must parse an instance with a list of scopes. The schema of an agent, the
  validation rules, the harness table, and the whole diagnostic corpus survive —
  what changes is the document shape and that duplicate-ID checking becomes an
  intra-file concern rather than a cross-file one.
- **Slice 3 becomes materially smaller**, for the reasons above. What remains is
  path canonicalization, ancestry, and drift between a recorded path and the
  filesystem.
- Slices 2 and 4 are unaffected. `factory-store` never read configuration, and
  `factory-context` already takes explicit paths.
- A single file, not a `.factory/scopes/` directory. Seven entries today, and the
  bulky `runtime:` blocks stay behind in the projects (see migration). Splitting
  a list into files later is mechanical.

## Migration, deliberately not executed with this ADR

Seven live scopes carry a `.factory/config.yaml`. Two facts were verified before
this decision was taken, because both are load-bearing:

- **`scripts/ensure_assistant_agents.py` reads only the `runtime:` block.** It
  calls `document.get("runtime")` and validates `runtime.version`; it never
  reads the top-level `version`, `scope`, or `agent`. So
  `assistant/.factory/config.yaml` can keep its `runtime:` block and lose
  everything else, and that script continues to work untouched.
- **`scripts/factory_tasks.py` still resolves the company root.** Its
  `resolve_root()` walks ancestors for `.factory/factory.sqlite` before falling
  back to `.factory/config.yaml`, and the instance root keeps both. Run from
  `projects/ingo` — a directory that would no longer have a `.factory/` — it
  still resolves to `/Users/factory/business-factory`.

The move itself gets its own task with a migration and rollback plan, in the
same way the task store did. It rewrites configuration for seven live scopes,
and doing it as a side effect of writing a decision record is exactly the kind
of unreviewed change that plan exists to prevent.
