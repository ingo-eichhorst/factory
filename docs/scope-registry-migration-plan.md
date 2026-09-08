# Migration plan: per-scope config files into the central registry

ADR 0015 makes a scope an entry in the Factory instance's `.factory/config.yaml`
rather than a directory containing its own. Seven live scopes carry the old
form. This plan describes the move; it is deliberately not executed as part of
writing the decision, for the same reason the task-store migration was not: it
rewrites live configuration, and doing that as a side effect of an unrelated
change is how a working system quietly stops working.

## What exists today

| Scope | Config file | Notes |
|---|---|---|
| `business-factory` | `.factory/config.yaml` | The instance itself |
| `assistant` | `assistant/.factory/config.yaml` | **Also carries a `runtime:` block owned by another tool** |
| `awesome-herdr` | `projects/awesome-herdr/.factory/config.yaml` | Its own GitHub repository |
| `factory` | `projects/factory/.factory/config.yaml` | Its own GitHub repository |
| `ingo` | `projects/ingo/.factory/config.yaml` | No repository of its own |
| `irrlicht` | `projects/irrlicht/.factory/config.yaml` | Has `code/Irrlicht` checkout |
| `model-lab` | `projects/model-lab/.factory/config.yaml` | Also carries a `runtime:` block |

## Two facts verified before this plan was written

Both are load-bearing, and both were measured rather than assumed.

**`scripts/ensure_assistant_agents.py` reads only the `runtime:` block.** It
calls `document.get("runtime")` and validates `runtime.version`; it never reads
the top-level `version`, `scope`, or `agent`. Therefore
`assistant/.factory/config.yaml` may keep its `runtime:` block and lose
everything else, and that script continues to work with no change. The same
holds for `model-lab`, whose `runtime:` block has no reader in this repository
but follows the same shape.

**`scripts/factory_tasks.py` still resolves the company root.** Its
`resolve_root()` walks ancestors looking for `.factory/factory.sqlite` first and
`.factory/config.yaml` second; the instance root keeps both. Executed from
`projects/ingo` — a directory that will no longer have a `.factory/` — it
resolves to `/Users/factory/business-factory` today, with the per-project
directory already absent from the search path.

## Steps

1. **Snapshot.** `git status` must be clean for the seven files. Record the
   current content of each so step 6 can verify nothing was lost:
   `find . -path '*/.factory/config.yaml' -not -path './projects/factory/fixtures/*' | sort | xargs shasum`
2. **Write the new registry** into `.factory/config.yaml`: `version`, an
   `instance:` block taking the current `business-factory` scope's id and name,
   and a `scopes:` list with one entry per remaining scope — id, name, `path`
   relative to the instance root, `git` where the project is its own repository,
   and the existing agent block verbatim. **Reuse every existing UUID.** A scope
   ID is permanent identity; regenerating one silently creates a different scope.
3. **Strip, do not delete, the two files with a `runtime:` block.**
   `assistant/.factory/config.yaml` and `projects/model-lab/.factory/config.yaml`
   keep only `runtime:`. They stop being Factory files and become the other
   tool's files, which is what they always were in part.
4. **Delete the four purely-Factory config files** and their now-empty
   `.factory/` directories: `awesome-herdr`, `factory`, `ingo`, `irrlicht`.
   Note that two of these are inside their own Git repositories, so the deletion
   must be committed there as well, or the working tree stays dirty — which is
   exactly the pollution ADR 0015 removes.
5. **Run `ensure_assistant_agents.py` in whatever dry-run or check mode it
   offers** and confirm it still parses its file. If it has no such mode, run it
   and compare its intended actions against the previous run.
6. **Verify.** `python3 -c "import sys; sys.path.insert(0,'scripts'); import factory_tasks; print(factory_tasks.ROOT)"`
   from inside `projects/ingo` must still print the instance root. Every UUID in
   the new registry must match the recorded hashes' scopes from step 1.

## Rollback

Every removed file is recoverable from Git in this repository, and steps 3 and 4
are the only destructive ones. `git checkout -- <paths>` restores them, and the
new `.factory/config.yaml` is reverted the same way. The two foreign
repositories (`awesome-herdr`, `factory`) need their own revert if step 4 was
already committed there.

**The one thing Git cannot restore** is a UUID regenerated instead of reused in
step 2, because the old value would exist only in the deleted file. This is why
step 2 says reuse and step 6 verifies it.

## Sequencing

This migration should follow the `factory-config` rework rather than precede it.
Until the crate can read the new shape, the new registry would be a file nothing
validates — and writing configuration that no code has ever parsed is how a
schema and its consumer drift apart before either is finished.
