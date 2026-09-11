# Factory

Production implementation of the Business Factory CLI and plugin microkernel.

The project is currently in feature-definition and architecture planning. The
kernel coordinates persistent thread agents, ephemeral task agents, and
script/process task execution. Runtime systems such as Herdr and all external
channels, harnesses, schedulers, credential stores, and knowledge providers are
scope-enabled plugins rather than kernel dependencies.

The Rust `factory` executable runs both the authoritative daemon and API client.
The CLI, a future dashboard, and other applications attach through API transport
plugins such as local IPC, WebSocket, or HTTP. The daemon alone mutates core
state and coordinates plugins and workers.

The initial architecture decision is documented in
[`docs/adr/0001-microkernel-workers-and-scoped-plugins.md`](docs/adr/0001-microkernel-workers-and-scoped-plugins.md).
The daemon and client API boundary is documented in
[`docs/adr/0002-daemon-and-api-transport-plugins.md`](docs/adr/0002-daemon-and-api-transport-plugins.md).
The logical command, query, response, error, persisted-event, CQRS replay, and
saga-compensation standards are consolidated in
[`docs/adr/0003-logical-api-and-event-sourcing-v1.md`](docs/adr/0003-logical-api-and-event-sourcing-v1.md).
The supervised subprocess lifecycle and framed JSON-RPC plugin host protocol
are documented in
[`docs/adr/0007-supervised-plugin-process-protocol-v1.md`](docs/adr/0007-supervised-plugin-process-protocol-v1.md).
The choice of Rust, the alternatives weighed against it, and what it costs are
documented in
[`docs/adr/0008-rust-as-implementation-language.md`](docs/adr/0008-rust-as-implementation-language.md).
The YAML library, the configuration schema-evolution policy, and path identity
on a case-insensitive volume are documented in
[`docs/adr/0009-slice-1-configuration-and-path-identity.md`](docs/adr/0009-slice-1-configuration-and-path-identity.md).
How these ADRs relate to the company design baseline — which differing terms
name the same component, and which capabilities are target state rather than
version 1 — is documented in
[`docs/adr/0010-design-baseline-reconciliation.md`](docs/adr/0010-design-baseline-reconciliation.md).
Where agent session state is observed, and why it comes from Irrlicht rather
than from terminal output, is documented in
[`docs/adr/0011-irrlicht-as-observation-plugin.md`](docs/adr/0011-irrlicht-as-observation-plugin.md).

The SQLite driver, migration tooling, locking and durability pragmas, the backup
procedure, and which session states hold a workspace lease are documented in
[`docs/adr/0012-slice-2-sqlite-driver-migrations-and-locking.md`](docs/adr/0012-slice-2-sqlite-driver-migrations-and-locking.md).

Why version 1 generates no harness compatibility file, why a context source that
cannot be read is a hard failure rather than an empty section, and why context
compilation follows no knowledge links are documented in
[`docs/adr/0013-slice-4-context-compilation-policy.md`](docs/adr/0013-slice-4-context-compilation-policy.md).
That a long-running daemon owns all mutations, and what that means for the CLI,
for `factory doctor`, and for sessions that outlive the daemon, is documented in
[`docs/adr/0014-daemon-owns-all-mutations.md`](docs/adr/0014-daemon-owns-all-mutations.md).
Why a scope is an entry in a central registry rather than a directory carrying
its own configuration file — and which class of defects that deletes — is
documented in
[`docs/adr/0015-central-scope-registry.md`](docs/adr/0015-central-scope-registry.md),
with the plan for moving the seven live scopes in
[`docs/scope-registry-migration-plan.md`](docs/scope-registry-migration-plan.md).

Why the scope table is a projection of the declared configuration rather than a
second source of truth, and why reconciliation reports a drift before it repairs
one, are documented in
[`docs/adr/0016-slice-3-registry-projection-and-reconcile.md`](docs/adr/0016-slice-3-registry-projection-and-reconcile.md).
Why session state for Pi is taken from Herdr rather than Irrlicht, how a pane, a
transcript, and a session are correlated, and which of Herdr's states may move a
task, are documented in
[`docs/adr/0017-slice-5-pi-adapter-observation-source.md`](docs/adr/0017-slice-5-pi-adapter-observation-source.md).

When a migration runs on an installed instance, why an update always migrates
and a downgrade never does, and why a configuration version change stays the
human's edit, are documented in
[`docs/adr/0018-updating-an-installed-instance.md`](docs/adr/0018-updating-an-installed-instance.md).
Why restoring is an explicit command rather than a consequence of opening a
file, what a recovered database may claim about processes it never observed, and
what a backup does and does not cover, are documented in
[`docs/adr/0019-restore-and-what-a-recovered-database-may-claim.md`](docs/adr/0019-restore-and-what-a-recovered-database-may-claim.md).

What is recorded about a run so that it can be measured — the pinned tuple that
makes two runs comparable, which metrics survive a comparison across harnesses,
how friction is captured, and why the fixture rather than a judge produces a
bench verdict — is documented in
[`docs/adr/0020-run-telemetry-and-the-evaluation-bench.md`](docs/adr/0020-run-telemetry-and-the-evaluation-bench.md).

The version-1 delivery plan is owned by this project in
[`docs/implementation-backlog.md`](docs/implementation-backlog.md).

## Where this repository lives

Factory is developed inside the private Business Factory working repository, at
`projects/factory/`, and published here as its own repository with the project
at the root. The published history is produced with `git subtree split`, so it
contains this directory's commits and nothing from the company root.

That split is deliberate rather than incidental. The working repository is the
company root: it carries the company design baseline, scope configuration for
every department and project, and operational scripts. Publishing it wholesale
to a repository named `factory` would put material here that has no reason to
be, so the boundary is enforced by what is exported rather than by remembering
what not to push.

To publish new work from the working repository:

```sh
git subtree push --prefix=projects/factory \
  https://github.com/ingo-eichhorst/factory.git main
```

**No remote for this repository is configured in the working repository, on
purpose.** A remote named `origin` there is how someone eventually runs
`git push --all` and publishes the company root by accident. Passing the URL
explicitly costs one line and removes that failure mode.

This does mean the project is tracked in two places, and that the two can drift.
Making this repository the single home — and reducing the working repository's
copy to a checkout, as it already does for other projects' code — is the
alternative, and it is a decision for the owner rather than a detail to settle
in passing.

## Building

The toolchain was installed with `rustup --no-modify-path`, so `~/.cargo/bin` is
deliberately not on `PATH`. Every shell needs this first:

```sh
. "$HOME/.cargo/env"
```

Then, from this directory:

```sh
./check.sh    # rustfmt, clippy with warnings denied, and the test suite
```

`check.sh` is the only gate. It sources the toolchain itself, so it also works
from a fresh shell.

### Layout

```text
crates/factory-config/   parsing and validation of .factory/config.yaml
crates/factory-context/  root-to-leaf compilation of the text a harness receives
crates/factory-paths/    canonical path identity and descendant tests
crates/factory-store/    the company-root SQLite store, migrations, and backup
fixtures/registration/   a miniature company used as a manual fixture
docs/                    ADRs, the delivery backlog, operator guides
```

There is no `factory` binary yet, and that is deliberate rather than pending.
The slices deliver domain libraries with tests first; the command surface
arrives in Slice 10, once the manual procedures it replaces have been proven.
Slices 1, 2, and 4 are implemented. Slice 3 (the scope registry) and Slice 5
onward are not.

Slice 5 was gated on whether a long-running daemon owns all mutations or each
invocation is its own process. That is now settled — a daemon owns them, and the
CLI is a client — in
[`docs/adr/0014-daemon-owns-all-mutations.md`](docs/adr/0014-daemon-owns-all-mutations.md).
Slices 1–4 were identical either way, which was checked while settling ADR
0012's locking strategy rather than merely assumed, so they were built and
committed before the decision was taken and needed no change afterwards.

Error messages are specified before they are implemented, in
[`docs/slice-1-error-corpus.md`](docs/slice-1-error-corpus.md). Slice 1's
acceptance criteria are almost entirely diagnostic-quality criteria — "naming
the file and both keys", "fails with the supported set" — so that corpus, not
the parser, is the deliverable.

Company-wide architecture inputs remain in [`../../.specs/`](../../.specs/):
the system design, the agent task and scheduler design, and the company
infrastructure ADRs. Existing root-level scripts are prototypes and remain in
place until an explicit migration is designed.
