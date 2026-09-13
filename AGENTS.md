# Factory

A daemon that gives tasks to coding agents and watches what happens.

## Layout

    crates/factory-core      domain, events, wire protocol, the four adapter traits
    crates/factory-plugins   built-in adapters, the plugin host, the registry
    crates/factory-daemon    engine, scheduler, interfaces, the binary
    crates/factory-cli       the `factory` binary
    ui/                      the web UI, compiled into the daemon
    ui/index.html              the page skeleton and the two view containers
    ui/app.css                 every style
    ui/js/core.js              DOM helpers, client state, the HTTP call, the socket
    ui/js/app.js               the wiring: which page shows, what an event means
    ui/js/{tasks,task-form,agents,occupancy,terminal,modal}.js   one per view
    ui/js/{dashboard,activity,site,site-render}.js               the new views
    ui/js/{sandboxes,secrets}.js                                 L2's two tabs
    ui/js/{benchmarks,knowledge}.js                              L5's two tabs
    ui/js/knowledge-graph.js                                     the knowledge graph's pure layout, filter and tail logic
    ui/vendor/three.min.js     vendored so the site's lit render works offline
    examples/plugins         a worked example of an out-of-process adapter

`README.md` explains the architecture and the plugin protocol. Read it before
changing an adapter trait -- those four traits are the whole point of the shape.

## Working here

    cargo build --workspace
    cargo test --workspace

Never develop against the company's live `.factory/`: it holds a real database
and real secrets. Make a throwaway instance instead:

    mkdir -p /tmp/dev/projects/demo
    target/debug/factory-daemon --root /tmp/dev init
    target/debug/factory-daemon --root /tmp/dev run

The `shell` agent runs the task's instructions as a shell command and reports
the exit status, so the whole dispatch path can be exercised without spending a
model call. Use it for anything that is not specifically about an AI harness.

Set the hooks up once per clone, before writing any code, from the primary
checkout:

    git config core.hooksPath "$(git rev-parse --show-toplevel)/.githooks"

The absolute path is the point. `core.hooksPath` is resolved against the
working directory of whichever worktree git is run from, so the obvious
`.githooks` finds nothing in a worktree whose branch predates this directory
-- and finds it silently, with no hook and no complaint. Since the rule below
is that work happens in a worktree, that is the normal case rather than the
edge one. One absolute path is shared by every worktree, because the config is.

Nothing else installs them, and the one in `.githooks/` is the only thing
stopping a push to `main` -- see below.

## Branches, worktrees and pull requests

Code is written on a branch, in a worktree of its own, and reaches `main`
through a pull request. Three rules, and they are one rule really.

**Every piece of coding work gets its own worktree.** Not a checkout that is
already open, and not `main`. Two agents in one working tree edit each other's
files and neither finds out until the build breaks; a worktree costs a
directory and removes the whole class of problem:

    git worktree add -b some-branch-name ../worktrees/some-branch-name main
    cd ../worktrees/some-branch-name

Factory itself now works this way for the tasks it dispatches, for exactly the
same reason -- one worktree per run, made before the agent starts. If it is
right for an agent Factory drives, it is right for one writing Factory.

When the branch is merged, `git worktree remove` it. A worktree left behind is
a stale copy of the repository that something will eventually be built from by
accident.

**`main` is never written to directly.** No commits on it, no pushes to it.
Work lands as a pull request that somebody -- or something -- merges:

    git push -u origin some-branch-name
    gh pr create --fill

Afterwards `git switch main && git pull --ff-only` brings the local `main`
level again. A `--ff-only` that refuses is information: it means `main` has
something local that never went through a review, and that is the thing to fix
rather than force through.

**The hook is the enforcement, and it is weaker than it looks.** GitHub cannot
do this for us: the repository is private on a free plan, where rulesets and
branch protection are both paid. So `.githooks/pre-push` refuses the push, and
it only runs if `core.hooksPath` was set, to an absolute path -- a fresh clone,
a machine nobody configured, or a relative path read from a worktree all leave
no guard at all, and none of them say so. Set it first; do not assume it is
there. `git rev-parse --git-path hooks/pre-push` says which file git will
actually run, which is the way to check rather than trusting that it is fine.
`--no-verify` gets past the hook, which is deliberate: a bypass should be
something a person typed on purpose and can be asked about afterwards, not
something impossible.

## The rules that matter

- A task's status comes from the agent calling `factory task report`, never from
  looking at a terminal and guessing. Runtime status is a liveness signal only --
  except a `blocked` a runtime reports through a lifecycle hook, which is the
  harness itself speaking, not a guess about a terminal, and the daemon may
  mark the run `Blocked` on that alone. A `blocked` a runtime only infers from
  the screen stays a suspicion: it may be shown, never asserted.
- A task is the standing intent; a run is one attempt at it. Sessions, tokens
  and outcomes belong to the run. The task mirrors its newest run so lists stay
  cheap -- if you add a field to that mirror, clear it too, or a successful
  retry will show the previous attempt's error.
- A scope owns only its `.factory/config.yaml`. Runtime state stays in the
  instance root's `.factory/`; never put the database, socket, worktrees, or
  other daemon-owned state inside a scope. `.factory/knowledge/` and
  `.factory/datasets/` are the one exception: authored content -- pages,
  documents and dataset YAML a person or an agent wrote by hand -- that
  nothing in Factory ever deletes or regenerates, and that is worth backing
  up like a scope's own files, even though it sits under the instance root's
  `.factory/` alongside everything the daemon does own.
- A plugin that fails must never take the daemon down with it.
- Roles bound what an agent does by accident, not what it could do. Every agent
  runs as the owner and can reach the socket; one that omits its token is the
  owner. Never write anything that implies otherwise.
- A role is data, not a match arm: grants and reach live in `factory-core/src/role.rs`
  and are checked in one place. A new request has to say which grant it needs --
  the match in `access.rs` has no wildcard arm, so the compiler asks.
- Which roles exist is a question about a scope. `Engine::roles_for(scope)`
  resolves the chain -- presets, the root's `roles:`, then each scope's
  `scope.roles` down to that scope -- from the live snapshot, and `authorize`,
  `set_agent_role`, declaration writes and the guide all ask it; never keep a
  second, flat copy. Ancestry is `Scope.path`, never a name, and inheriting a
  definition never widens reach past the agent's own scope.
- A permanent agent is quiet by design. It is checked for whether its session is
  still there and nothing else -- never for whether it has said anything. The
  run timeouts must not reach it.
- The guide (`AgentContext::factory_guide`) and the reporting contract
  (`AgentContext::reporting_contract`) are the only two places Factory tells
  an agent how to use it. Anything else an agent needs to know belongs in one
  of those two, not in a third place invented for the occasion.
