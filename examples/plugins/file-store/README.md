# file-store

A task store that keeps tasks in one JSON file instead of the built-in
sqlite database. Same protocol as `../shell-plugin/`, in the same amount of
Python: one `adapter.py`, stdlib only, plus a `plugin.yaml`.

## Point a scope at it

Drop this directory under `.factory/plugins/file-store/`, then:

```yaml
scopes:
  - name: demo
    path: projects/demo
    task_store: file-store
```

Every other scope keeps using `daemon.task_store` (`sqlite` by default); only
`demo`'s tasks move. Set `FILE_STORE_PATH` in the plugin's own `env:` (in its
`plugin.yaml`) to say where the JSON file lives; left unset, it sits next to
`adapter.py`.

## What it actually has to implement

The daemon routes by scope, not by store. A task goes wherever its scope's
`task_store` points, but the run that attempts it -- attempt number, token,
session, started and ended -- and the journal and the standing agents all
stay in the instance's own ledger store, because none of those have a home
in an issue tracker. So an issue-tracker-backed store only ever answers six
questions:

    task.create   task.get   task.list   task.update   task.delete   task.due

Everything else `TaskStore` defines on the Rust side -- `create_run`,
`get_run`, `update_run`, `runs`, `active_run`, `active_runs`, `runs_between`,
`put_agent`, `get_agent`, `agents`, `delete_agent`, `append_entry`,
`entries`, `run_entries` -- is real to the trait but never reaches a plugin
as a call worth answering: `adapter.py` refuses every one of them by name,
loudly, rather than pretending to have somewhere to put a GitHub issue's
callback token or its terminal transcript.

A third set doesn't even get that far. `append_status`, `status_changes`,
and `status_origin` -- the liveness history a runtime reports -- have
default no-op implementations on the trait itself, and the daemon's plugin
proxy never overrides them. A scope backed by a plugin store like this one
simply never gets asked; its occupancy chart draws runs only, with no
liveness strip underneath, which is the honest picture of what the store
actually knows.

## Trying it by hand

Nothing here needs the daemon running. Pipe request frames at `adapter.py`
on stdin the way the daemon would, one JSON object per line, and read the
replies back on stdout:

```sh
FILE_STORE_PATH=/tmp/file-store-demo/tasks.json python3 adapter.py <<'EOF'
{"id": 1, "method": "describe", "params": {}}
{"id": 2, "method": "task.create", "params": {"task": {"id": "t1", "title": "say hello", "instructions": "echo hi", "scope": "demo", "agent": "shell", "runtime": "shell", "status": "pending", "runs": 0, "labels": {}, "created_at": "2026-09-11T09:00:00Z", "updated_at": "2026-09-11T09:00:00Z"}}}
{"id": 3, "method": "run.create", "params": {"run": {}}}
EOF
```

The third call comes back an error: this store keeps tasks only.
