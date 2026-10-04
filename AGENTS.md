# Factory

A daemon that gives tasks to coding agents and watches what happens.

## Layout

    crates/factory-kernel    L0: pure shared vocabulary and every live fact schema (Level/Fact, nested statuses, grants and evidence); no other factory-* dependency
    crates/factory-core      domain, events, wire protocol, the five adapter traits
    crates/factory-plugins   built-in adapters, the plugin host, the registry
    crates/factory-daemon    engine, scheduler, interfaces, the binary
    crates/factory-cli       the `factory` binary
    ui/                      the web UI, compiled into the daemon
    ui/index.html              the page skeleton and the two view containers
    ui/app.css                 every style
    ui/js/core.js              DOM helpers, client state, the HTTP call, the socket
    ui/js/app.js               the wiring: which page shows, what an event means
    ui/js/{tasks,task-form,agents,occupancy,terminal,modal}.js   one per view
    ui/js/schedule.js          a task's schedule between the wire and the form, pure
    ui/js/occupancy-model.js   the occupancy chart's window: live or fixed, zoom, pan, ticks, pure
    ui/js/{dashboard,activity,site,site-render}.js               the new views
    ui/js/{sandboxes,secrets,dependencies}.js                    L2's three tabs
    ui/js/dependencies-model.js                                  Dependencies' pure shaping logic
    ui/js/{benchmarks,knowledge}.js                              L5's two tabs
    ui/js/knowledge-graph.js                                     the knowledge graph's pure layout, filter and tail logic
    ui/js/{datasets,bench-runs}.js                               Benchmarks' Datasets and Runs segments
    ui/js/bench-model.js                                         pure helpers for the Benchmarks tab's three segments
    ui/js/{policy,policy-model}.js                               the L6 Policy tab and its pure shaping logic
    ui/js/{goals,goals-model}.js                                 the L6 Goals tab and its pure shaping logic
    ui/js/{budget,budget-model}.js                               the L6 Budget tab and its pure shaping logic (#164)
    ui/js/occupancy-model.js                                     the occupancy chart's lane geometry (#120), pure
    ui/js/usage-model.js                                         a run's and a task's usage block (#117), pure
    ui/js/pending-model.js                                       pending tasks split into due, scheduled later and manual (#124), pure
    ui/js/task-model.js                                          a task's board column, failure/close line and actions (#122), pure
    ui/js/{scenarios,scenarios-model}.js                         the L6 Scenarios tab and its pure shaping logic
    ui/js/{backup,backup-model}.js                               the L1 Backup tab and its pure shaping logic
    ui/js/{dates,dates-model}.js                                 Important Dates, shared dependency badges and renewal Inbox shaping (#236)
    ui/js/{environments,environments-model}.js                   the L1 Operations tab (#185) and its pure shaping logic
    ui/js/{doctor,doctor-model}.js                               the L1 Doctor dependency view and its pure shaping logic
    ui/js/dashboard-model.js                                     the dashboard's tile vocabulary, default layout and row packing (#163), pure
    ui/js/dashboard-tiles-model.js                                metric-tile and view-tile shaping, and which endpoints a layout needs (#162), pure
    ui/js/dashboard-editor-model.js                               the Customise tile editor's catalogue, search and list edits (#160), pure
    ui/js/clock-model.js                                          the CRA Art. 14 reporting clock's pure shaping, shared by Intake, the Inbox and L6 Policy (#157/#170 phase 2)
    ui/vendor/three.min.js     vendored so the site's lit render works offline
    examples/plugins         a worked example of an out-of-process adapter
    examples/openshell       `sandbox: openshell` (#218): the image recipe, provider profiles, the awesome-herdr block

`README.md` explains the architecture and the plugin protocol. Read it before
changing an adapter trait -- those five traits are the whole point of the shape.

## Working here

    cargo build --workspace
    cargo test --workspace

Never develop against the company's live `.factory/`: it holds a real database
and real secrets. Make a throwaway instance instead:

    mkdir -p /tmp/dev/projects/demo
    target/debug/factory-daemon --root /tmp/dev init
    target/debug/factory-daemon --root /tmp/dev run

The `shell` agent runs the task's instructions as a shell command and reports
the exit status and stdout, so the whole dispatch path can be exercised
without spending a model call. Use it for anything that is not specifically
about an AI harness.

Factory releases are built and scanned outside the daemon with
`examples/factory-dependency-scan.sh built`: it runs `cargo auditable build
--workspace --release` and attaches separate CycloneDX build SBOM and
vulnerability documents. The `running` mode scans only the installed
`~/.local/bin/factory` and `factory-daemon` binaries. The daemon and L1 Doctor
only read those immutable attachments; they never invoke a scanner.

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
same reason -- one worktree per task, made before its first run starts. If it is
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

**A task that changed code ends with an open pull request.** That holds for
every run Factory dispatches here: commit, push the branch, `gh pr create`, and
put the pull request's URL in the `--result` of `factory task report --status
done`. A branch left only in a run's worktree is work nobody reviews, and the
worktree is not where anyone looks for it. If the checks cannot be made green,
open it as a draft and say what fails. Merging is the step that waits for a
person -- never merge, approve or enable auto-merge on your own pull request.
That prohibition is about the pull request to `main`: Factory's single-writer
integrator may locally merge child task branches into a dedicated, non-`main`
integration branch so it can test their combined result and open that one PR.
Those internal merges are coordination, not permission to merge the final PR.
Whoever writes a task says so too, and never writes "do not push".
`.agents/skills/implement-github-issue` is the whole procedure for an issue.

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
  the screen stays a suspicion: it may be shown, never asserted. A turn that
  ended is the same kind of fact: the harness saying so through a lifecycle
  hook -- herdr relaying one for `pi`, or Claude Code's `Stop`/`StopFailure`
  calling `factory task turn-ended` directly -- may fail a run whose agent
  never reported, unless the run is `Blocked` or the harness has background
  work that will wake it. An `idle` read off a screen never may; it may only
  hold such a fact back (a `Stop` another hook can overrule waits for one),
  never stand in for it.
- A task is the standing intent; a run is one attempt at it. Sessions, tokens
  and outcomes belong to the run. The task mirrors its newest run so lists stay
  cheap -- if you add a field to that mirror, clear it too, or a successful
  retry will show the previous attempt's error.
- A scope owns only its `.factory/config.yaml`. Runtime state stays in the
  instance root's `.factory/`; never put the database, socket, worktrees, or
  other daemon-owned state inside a scope. `.factory/knowledge/`,
  `.factory/datasets/`, `.factory/policies/` (including its `drafts/`
  subdirectory), `.factory/goals/`, `.factory/scenarios/`,
  `.factory/quality/`, `.factory/vex/`, `.factory/intake/` and
  `.factory/budgets/` are the one
  exception: authored
  content -- pages,
  dataset YAML, policy catalogues (real and draft), the goals
  direction/cycle files, scenario files, quality profiles, VEX judgments and
  per-scope definitions of ready and monthly budget limits a person or an agent wrote by hand -- that
  nothing in Factory ever deletes
  or regenerates, and that is worth backing up like a scope's own files,
  even though it sits under the instance root's `.factory/` alongside
  everything the daemon does own.
- A plugin that fails must never take the daemon down with it.
- Roles bound what an agent does by accident, not what it could do. Every agent
  runs as the owner and can reach the socket; one that omits its token is the
  owner. Never write anything that implies otherwise.
- A role is data, not a match arm: role definitions, grant expansion and reach live in `factory-core/src/role.rs`
  and are checked in one place. A new request has to say which grant it needs --
  the match in `access.rs` has no wildcard arm, so the compiler asks.
- Live facts and their nested schema live in L0, with a `Fact::Producer` and
  a complete typed catalogue test. The shared `Grant` vocabulary lives there
  too and is re-exported by `role.rs`; wildcard authorization, receipt
  deduplication, task-status mapping and conformance evaluation stay in core.
  L0 never depends on a producing level or gathers evidence itself. Typed
  providers in daemon `facts/l1.rs` through `l5.rs` own the live reads;
  policy and fact-backed metrics ask `Facts<Reader>::get`. A port returns
  only its fact or a collection of it, never another level's report. The
  kernel read boundary enforces a sealed `Producer: Below<Reader>` relation:
  strictly upward reads only, including adjacent levels. Same-level calls
  stay in their service. The router and page composition use `Facts<People>`
  outside the ladder, not an internal level's identity. Crate splitting and
  the strict command ladder remain in #193; the bound alone is not service
  isolation or an authorization boundary.
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
