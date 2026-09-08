# Slice 1 — operator guide

The backlog's delivery approach is to build and exercise each slice first with
documented operator commands and fixtures, so that the CLI can later replace a
manual procedure without changing the model underneath it. This document is
that manual procedure for Slice 1: repository contract and configuration
validation (`docs/implementation-backlog.md`).

## What this slice delivers, and what it does not

Slice 1 gives you a typed, validated model of a scope's
`.factory/config.yaml`: the `factory-config` crate's `load` and `parse`
functions read a file or a string and return a `ScopeConfig` (a version, a
`Scope` with a UUID and a name, and one or more `Agent`s) or a `ConfigError`
rendered per `docs/slice-1-error-corpus.md`. Its `validate_unique_ids`
function checks a set of already-loaded configs for a shared scope ID. The
`factory-paths` crate gives you canonical path resolution and the
`(st_dev, st_ino)` aliasing check that canonical-path strings alone cannot
provide on a case-insensitive volume (ADR 0009 §3a).

That is the entire boundary. Slice 1 does not:

- start an agent, a session, or a Herdr pane;
- write a database — `factory.sqlite` does not exist until Slice 2;
- register anything — there is no scope registry until Slice 3;
- compile context — `AGENTS.md` concatenation is Slice 4;
- provide a `factory` binary at all. There is no executable to run. Every
  procedure below drives the library crates directly, through their tests or
  through a throwaway program you write and discard.

If a command below looks like it invokes a CLI, it does not exist yet. Nothing
in this document should be read as a preview of `factory`'s interface; the
real CLI surface is design §7, and Slice 1 implements none of it.

Validation itself is also read-only in the strictest sense the design allows:
`factory-config` performs no filesystem writes of any kind, not even a
temporary file, which is asserted directly in the crate's own tests
(`docs/slice-1-error-corpus.md`, "What validation must not do").

## Getting a toolchain

Rust on this machine was installed with `rustup ... --no-modify-path`, which
means `~/.cargo/bin` is deliberately **not** on `PATH`. Every fresh shell needs
one line before `cargo` or `rustc` resolve to anything:

```sh
. "$HOME/.cargo/env"
```

This trips up everyone exactly once, usually as "`cargo: command not found`"
in a shell that worked five minutes ago in a different terminal tab. There is
no way around it and no reason to work around it — `check.sh` sources the same
file internally so it works regardless of your shell's state, but an
interactive session needs the line run by hand first.

`rust-toolchain.toml` at the workspace root pins the `stable` channel plus the
`rustfmt` and `clippy` components. Once `rustup` itself is on `PATH`, running
any `cargo` command inside this workspace installs and selects that toolchain
automatically; you do not separately manage toolchain versions.

## Running the checks

From the workspace root:

```sh
./check.sh
```

It can also be run from anywhere — it resolves its own location and `cd`s
there before doing anything else. It runs three stages, in order, and stops at
the first failure:

1. `cargo fmt --all -- --check` — every file is already in canonical
   `rustfmt` form. This catches nothing about correctness; it exists so that
   no review ever spends time on formatting, and so diffs stay minimal when
   two agents touch adjacent code.
2. `cargo clippy --workspace --all-targets -- -D warnings` — the lint gate,
   with warnings promoted to errors. This is where an `unsafe_code = "forbid"`
   violation, a needless clone, or a suspicious pattern gets caught before it
   is ever a runtime bug.
3. `cargo test --workspace` — every unit and integration test across every
   crate, including the full diagnostic corpus in
   `docs/slice-1-error-corpus.md` and the no-filesystem-writes assertion in
   `factory-config`'s test suite. This is the stage that proves the
   acceptance criteria, not just that the code compiles.

`./check.sh` is the single gate a change must pass before it is handed back.
Passing it locally is not optional groundwork for CI — at this stage of the
project, it *is* the whole verification story.

## Validating a real scope config today

Since there is no `factory` binary, "validate this config" means "call the
library the same way the test suite does." The worked example is the fixture
at `fixtures/registration/`, a miniature company built for exactly this
purpose:

```text
fixtures/registration/
├── .factory/config.yaml        # the instance, registering BOTH scopes
├── AGENTS.md                   # company context
└── projects/
    └── example-project/
        └── AGENTS.md           # project context, additive to the root
```

Note what the project directory does *not* contain. Since ADR 0015 a scope is an
entry in the instance's configuration, so `example-project` holds only its own
`AGENTS.md` — nothing of Factory's. That is the whole point of the change: a
project that is someone else's repository stays clean.

The first scope entry uses the `agent:` shorthand for a single agent. The second
uses the `agents:` list to define two — one `permanent` (`harness: pi`) and one
`temporary` (`harness: opencode`) — so the fixture exercises both accepted
spellings and both `lifetime` values in one file. The second entry also carries
a `git` reference, covering the case where a project is its own repository.

The file carries a top-level `fixture: true` key. It means nothing to the schema:
Factory ignores unknown top-level keys per ADR 0009 rule 2, the same rule that
lets the real `assistant` scope keep its unrelated `runtime:` block.

Be precise about what that marker does, because it was once described as more.
It makes the file greppable for a human. It does **not** protect anything, and
it never did — Factory ignores it by design. What actually prevents a fixture
being mistaken for a live scope is ADR 0015: Factory no longer searches the
filesystem for configurations at all, so a fixture is simply a file that nothing
reads unless a test hands it over.

To actually run a config through the loader without a CLI, write a short test
against the public API and run it with `cargo test`. This is deliberately not
committed anywhere permanent yet — Slice 1's own fixtures and assertions live
under `crates/factory-config/fixtures/` and are owned by that crate's test
suite — but the pattern an operator uses to check an arbitrary file today is:

```rust
// `cargo test` runs a package's test binaries with the working directory set
// to that package's own root (`crates/factory-config/`), not the workspace
// root, so a path relative to the workspace would silently resolve to the
// wrong file. Anchor on `CARGO_MANIFEST_DIR`, which Cargo sets at compile
// time, instead of guessing the invocation's current directory.
const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/registration");

#[test]
fn fixture_company_loads() {
    let root_path = format!("{FIXTURE}/.factory/config.yaml");
    let child_path = format!("{FIXTURE}/projects/example-project/.factory/config.yaml");

    let root = factory_config::load(&root_path).expect("company root should be valid");
    let child = factory_config::load(&child_path).expect("child scope should be valid");

    assert_eq!(root.agents.len(), 1);
    assert_eq!(child.agents.len(), 2);

    // The one Slice 1 check that spans files: two distinct scope IDs must
    // never collide, which is exactly what a copied config would produce.
    factory_config::validate_unique_ids(&[root, child])
        .expect("fixture scopes must not share an ID");
}
```

Drop a test like this into a temporary file under `crates/factory-config/tests/`,
run `cargo test`, read the result, then discard it — or ask whoever owns that
crate to add a permanent version if the check is worth keeping. Either way,
this is the whole "validate a config" procedure available today: load each
file, inspect the typed `ScopeConfig` or the rendered `ConfigError`, and check
cross-file invariants with `validate_unique_ids`. If you deliberately break a
copy of the fixture — set both `agent:` and `agents:`, duplicate an agent
name, use `lifetime: ephemeral` — the error you get back should match the
corresponding case in `docs/slice-1-error-corpus.md` exactly; that document is
the specification for what the message must say, not a sample of it.

## Registering a scope by hand

Design §4 is explicit that scope registration is deliberate:
`factory scope add` names a path and a scope; a bounded `factory scope
reconcile` may later rebuild or check the registry from a supplied root; and
recursive globbing for `.factory/config.yaml` files is never normal operation.
None of that command surface exists yet — Slice 3 builds the registry itself —
but a scope is defined by its config file regardless of whether anything has
registered it, so you can create one by hand today the same way `factory scope
add` will tomorrow.

To register a new scope at some path:

1. Create the directory if it does not already exist, then create
   `.factory/` inside it. (Once `factory scope add` exists, this directory
   creation is the one thing it is allowed to do outside `.factory/` itself —
   see the invariant below.)
2. Write `.factory/config.yaml`:

   ```yaml
   version: 1

   scope:
     id: <fresh UUID>
     name: <scope-name>

   agent:
     name: <Agent Name>
     harness: <pi | claude-code | opencode>
   ```

   - `version` must be `1`; there is no other supported version yet, and
     Factory never guesses or silently upgrades one (ADR 0009 rule 4).
   - `scope.id` must be a real UUID, generated with `uuidgen`, and must not be
     copied from another config — a scope ID is permanent identity, and two
     scopes sharing one is a Slice 1 validation error, not a Slice 3 one.
   - Use `agent:` for a single agent or `agents:` for a list of more than one;
     a file may set only one of the two.
   - Agent names must be unique within the scope — a name is how
     `factory task send` will address a recipient.
   - `harness` must be one of `pi`, `claude-code`, or `opencode`.
   - `lifetime` is optional and defaults to `permanent`; the only other value
     is `temporary`.
   - `max_sessions` is optional, defaults to `1`, and must be at least `1` —
     an agent that could never start a session is a validation error, not a
     silent no-op.
3. Add an `AGENTS.md` beside the config with real project context — not a
   placeholder. Design §2.5 compiles these root-to-leaf: company context,
   then every ancestor scope, then the current scope, then the agent
   definition, then the task prompt. A child scope's `AGENTS.md` should
   therefore add to what the company root already says rather than restate
   or contradict it — `fixtures/registration/projects/example-project/AGENTS.md`
   is a short worked example of that relationship.

Nothing in this procedure is enforced by a command yet; the enforcement is
Slice 1's validator, which you run against the file as described above.

## The one invariant you must never violate by hand

> No Factory command writes to a file outside a `.factory/` directory.

This is design §4's central rule, and it is the reason scope configuration,
generated compatibility files, and (starting in Slice 2) the operational
database all live under `.factory/` rather than scattered at the scope root.
It replaces a denylist — "never overwrite `AGENTS.md`, `CLAUDE.md`, `.pi/`,
`.claude/`" — with an allowlist that cannot be outgrown by whichever harness
appears next.

Two clarifications keep it exact, and both matter when you are doing any of
this by hand:

- **`factory init` and `factory scope add` may create the scope directory
  itself.** Creating the container that will hold `.factory/` is not writing
  content beside it. If you are registering a scope in a directory that does
  not exist yet, creating that directory is the one exception, not a
  violation.
- **The rule constrains Factory, not agents.** An agent carrying out a task
  writes wherever the task requires — source files, reports, worktree
  changes — and that is ordinary work product, not a Factory write. The
  invariant is about Factory's own bookkeeping: it never leaves `.factory/`,
  so a bug in Factory itself can never damage a repository, and everything
  Factory owns can be removed by deleting one directory.

Nothing in Slice 1 writes anything at all, by hand or otherwise, so this
invariant is trivially satisfied today. It is stated here because every later
slice tests against it, and the fixture in `fixtures/registration/` was built
to already look like a scope that respects it.
