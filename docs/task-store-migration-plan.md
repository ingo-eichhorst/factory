# Task store migration plan

- Status: Proposal, awaiting go/no-go
- Date: 2026-09-07
- Scope: Moving the prototype task store into the Factory project

`AGENTS.md` requires a migration and rollback plan before any company-root
script or extension is moved. This document is that plan. Nothing described
here has been executed.

## Motivation

`scripts/factory_tasks.py` is the working prototype of backlog slice 11
(shared task audit and cron dispatcher). It owns the durable task, run, event,
decision, artifact, and schedule tables that slice 11 specifies. As project
code it belongs to the Factory project, which owns "domain services,
persistence schema, migrations, adapters, tests".

It is also live production infrastructure. Every Pi agent uses it through the
`run_get` / `run_progress` / `run_decision` / `run_complete` workflow, and a
loaded `launchd` job runs its dispatcher. The move is therefore an operational
change, not a file reorganization.

## What moves

| From | To |
|---|---|
| `scripts/factory_tasks.py` | `projects/factory/scripts/factory_tasks.py` |
| `tests/test_factory_tasks.py` | `projects/factory/tests/test_factory_tasks.py` |
| `launchd/com.business-factory.scheduler.plist` | `projects/factory/launchd/com.business-factory.scheduler.plist` |

## What deliberately stays

- **`.pi/extensions/factory-tasks.ts` stays at the company root.** It is the
  only copy, and it exposes the task tools to every agent in the company. Pi
  resolves extensions per scope, so moving it under `projects/factory/.pi/`
  would remove the task tools from every other agent. Only its path constant
  changes. This is exactly the "operational compatibility path" that
  `AGENTS.md` describes.
- **`.factory/factory.sqlite` stays at the company root.** `design.md` section 4
  pins one operational database to the company root.
- **`.specs/design.md`, `.specs/agent-task-scheduler-design.md`, and
  `.specs/adr/`** stay as company architecture inputs.
- **`scripts/ensure_assistant_agents.py`, `scripts/local-llm/`,
  `scripts/local-image/`** are assistant and company infrastructure, not
  Factory project code.

## Breakage inventory

Four references break, and the first is silent.

1. **`factory_tasks.py:24-25` — silent data loss. RESOLVED 2026-09-07, ahead of
   the move.** The former derivation was:
   ```python
   ROOT = Path(__file__).resolve().parents[1]
   ```
   `parents[1]` resolved the company root from the script's own location. After
   a move it would resolve to `projects/factory`, making the default `DB_PATH` a
   **new, empty** `projects/factory/.factory/factory.sqlite`. SQLite creates the
   file on demand, so there would be no error: every agent silently starts
   writing to an empty store and the existing history disappears from view.

   `resolve_root()` now replaces it, in order: `FACTORY_ROOT`, then the nearest
   ancestor holding `.factory/factory.sqlite`, then the topmost ancestor holding
   `.factory/config.yaml` for a not-yet-initialised root, else a loud
   `SystemExit`. A depth-adjusted `parents[3]` was rejected because it
   reintroduces the same silent failure on the next move.

   Verified by running a copy from `projects/factory/scripts/`: it resolves
   `ROOT` to the company root, where the old code would have pointed at an empty
   `projects/factory/.factory/factory.sqlite`. This removes the only data-loss
   risk from the migration; the three remaining breakages are ordinary path
   edits.

2. **`~/Library/LaunchAgents/com.business-factory.scheduler.plist` — a copy,
   not a symlink.** The loaded job hardcodes
   `/Users/factory/business-factory/scripts/factory_tasks.py`. Editing the
   repository plist changes nothing about the running job; the installed copy
   must be replaced and the job reloaded. (Contrast
   `de.businessfactory.assistant-agents.plist`, which *is* a symlink into the
   repository.)

3. **`.pi/extensions/factory-tasks.ts:7`** —
   `resolve(import.meta.dirname, "../../scripts/factory_tasks.py")` becomes
   `"../../projects/factory/scripts/factory_tasks.py"`.

4. **`tests/test_factory_tasks.py:14`** —
   `parents[1] / "scripts" / "factory_tasks.py"` becomes `parents[1] / "scripts"`
   relative to its own new location.

The `AGENTS.md` task-workflow instructions reference tool *names* only
(`run_get`, `run_progress`, `run_decision`, `run_complete`, `run_block`), never
filesystem paths, so no instruction file needs editing.

## Migration steps

1. Record the pre-migration state: `sqlite3 .factory/factory.sqlite` row counts
   for `tasks`, `task_runs`, `task_events`, `task_decisions`, `artifacts`, and
   `schedules` (verified against the live database), plus
   `shasum .factory/factory.sqlite`.
2. Back up the database with `sqlite3 .factory/factory.sqlite ".backup ..."` to
   a timestamped file outside the repository.
3. Stop the dispatcher: `launchctl bootout gui/$UID/com.business-factory.scheduler`.
   Confirm no `factory_tasks.py` process remains.
4. Confirm no agent has a run in `running` state; a mid-run move would strand it.
5. `git mv` the three files listed above.
6. Update the paths in the Pi extension, the test, and the repository plist.
   (The root derivation needs no work: breakage 1 was fixed on 2026-09-07,
   before the move.)
7. Copy the updated plist to `~/Library/LaunchAgents/` and
   `launchctl bootstrap gui/$UID ...`.
8. Verify (below). Only then commit.

## Verification

- `python3 projects/factory/tests/test_factory_tasks.py` passes (6 tests). The
  suite is `unittest`, not `pytest`, and `unittest discover` fails on the
  directory because it has no `__init__.py`; invoke the file directly.
- `python3 projects/factory/scripts/factory_tasks.py task-list` returns the
  pre-migration tasks. Subcommands are hyphenated (`task-list`, `run-get`,
  `schedule-list`); the underscore forms used in `AGENTS.md` are the Pi tool
  names, not the CLI verbs.
- The row counts from step 1 are unchanged, and no
  `projects/factory/.factory/factory.sqlite` was created.
- A Pi agent can call `run_get` and receive real data.
- `launchctl list | grep com.business-factory.scheduler` shows the job loaded,
  and `.factory/logs/scheduler.log` records a dispatch within two minutes.

## Rollback

Every step is reversible and the database is never moved.

1. `launchctl bootout gui/$UID/com.business-factory.scheduler`.
2. `git checkout` the pre-migration commit, or `git mv` the three files back and
   revert the three reference edits.
3. Restore the previous `~/Library/LaunchAgents/` plist and `launchctl bootstrap`.
4. Restore the database from the step-2 backup only if verification showed
   writes landed in a wrong location.

Because the migration is committed only after verification, rollback before
commit is a working-tree revert.

## Recommendation

Do this as its own task with the dispatcher stopped, not bundled with feature
work. The silent-`DB_PATH` failure in breakage 1 is the only part that can lose
data, and it is worth fixing on its own merits even if the files never move:
the current derivation makes the store's location depend on the script's depth
in the tree.
