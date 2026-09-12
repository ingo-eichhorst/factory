#!/usr/bin/env python3
"""A task store that lives outside the daemon, backed by one JSON file.

Copy this file to start a store of your own -- an issue tracker, a shared
spreadsheet, whatever holds "the standing intent" for your team already.

The protocol is the same line-delimited JSON as any other plugin:

    in   {"id": 1, "method": "task.create", "params": {...}}
    out  {"id": 1, "result": {...}}
    out  {"id": 1, "error": {"message": "..."}}

Anything you write to stderr is the daemon's stderr. Anything you write to
stdout that is not a reply is ignored, so print your debugging to stderr.

WHAT A TASK-STORE PLUGIN IS ACTUALLY ASKED

This is the one thing worth reading before the rest of the file. The daemon
routes by scope, not by store: a task goes wherever its scope names for
`task_store`, but the run that attempts it -- attempt number, token, session,
started/ended -- and the journal and the standing agents stay in the
instance's own ledger store, because none of those have a home in an issue
tracker. A GitHub issue does not grow a column for a callback token.

So this plugin answers exactly six methods -- task.create, task.get,
task.list, task.update, task.delete, task.due -- and refuses everything else
`TaskStore` can be asked, loudly and by name, rather than pretending to
support it. That refusal is not a shortcut taken to keep this example short;
it is the actual shape of the seam. Three more methods -- the liveness
history a runtime reports (`append_status`, `status_changes`,
`status_origin`) -- never even reach a plugin: the daemon's proxy for this
trait does not forward them, so a scope backed by a store like this one loses
nothing by not answering them, and its occupancy chart simply draws runs
without the liveness strip underneath.

WHERE THE FILE LIVES

The only configuration the protocol offers a plugin is `plugin.yaml`'s `env:`
map, which the daemon sets as environment variables on this child process
(see `factory-plugins/src/host.rs`, which also runs the child with its
working directory set to this plugin's own directory -- so a relative value
there resolves beside the manifest for free). Set `FILE_STORE_PATH` there to
say where the JSON file lives; left unset, it sits next to this script.
"""

import json
import os
import re
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path


class ProtocolError(Exception):
    """A well-formed refusal, carrying exactly the text that belongs on the
    wire -- as opposed to a bug in this script, which the dispatch loop below
    reports with its Python type attached so it is obviously not that."""


# -- where the tasks live -----------------------------------------------------


def store_path():
    configured = os.environ.get("FILE_STORE_PATH")
    if configured:
        return Path(configured)
    return Path(__file__).resolve().parent / "tasks.json"


def load_store(path):
    if not path.exists():
        return {"tasks": {}}
    text = path.read_text()
    if not text.strip():
        # A zero-byte file is what `touch` or a half-finished first write
        # leaves behind; treat it as "nothing here yet" rather than a corrupt
        # database.
        return {"tasks": {}}
    try:
        data = json.loads(text)
    except json.JSONDecodeError as exc:
        # Starting from an empty store here would look like a working plugin
        # and quietly erase every task on the next write. A daemon that
        # cannot read its own tasks should say so and stop, not improvise.
        raise ProtocolError(f"{path} is not readable JSON: {exc}") from exc
    if not isinstance(data.get("tasks"), dict):
        raise ProtocolError(f"{path} does not look like a file-store database")
    return data


def atomic_write(path, data):
    """Write beside the target and rename over it, so a daemon killed
    mid-write leaves either the old file or the new one -- never a half-
    written one. `fsync` before the rename, or the durability half of that
    promise only holds until the page cache decides otherwise."""
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(dir=str(path.parent), prefix=".file-store-", suffix=".tmp")
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(data, f, indent=2, sort_keys=True)
            f.write("\n")
            f.flush()
            os.fsync(f.fileno())
        os.replace(tmp_name, path)
    except BaseException:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass
        raise


# -- timestamps ---------------------------------------------------------------

_TIME_RE = re.compile(r"^(.*T\d{2}:\d{2}:\d{2})(\.\d+)?([+-]\d{2}:\d{2})$")


def parse_dt(raw):
    """Parse whatever chrono wrote. `DateTime<Utc>`'s serde impl uses a `Z`
    suffix and, depending on the value, anywhere up to nine fractional digits
    -- both of which `datetime.fromisoformat` rejects on the Python versions
    this is meant to run on without a dependency. Normalising both away here
    is cheaper than asking every caller to remember to."""
    s = raw.strip()
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    m = _TIME_RE.match(s)
    if m:
        base, frac, offset = m.groups()
        if frac:
            s = f"{base}.{(frac[1:] + '000000')[:6]}{offset}"
        else:
            s = f"{base}{offset}"
    return datetime.fromisoformat(s)


def now_iso():
    # A field we mint ourselves, not one we are just relaying, so it has to
    # carry an offset -- a naive string here is one chrono's own deserializer
    # on the other end will refuse, and that refusal surfaces three calls away
    # from this line with no clue it was a timestamp at fault.
    return datetime.now(timezone.utc).isoformat()


# -- describe -----------------------------------------------------------------


def describe(_params):
    return {
        "name": "file-store",
        "kind": "task",
        "protocol": 1,
    }


# -- the six task ops -----------------------------------------------------


def task_create(params):
    # The daemon has already minted the id and filled in every field a task
    # is created with; this store's job is only to keep what it was handed.
    task = params["task"]
    data = load_store(store_path())
    data["tasks"][task["id"]] = task
    atomic_write(store_path(), data)
    return task


def task_get(params):
    data = load_store(store_path())
    return data["tasks"].get(params["id"])


def task_list(params):
    filt = params.get("filter") or {}
    tasks = list(load_store(store_path())["tasks"].values())

    status = filt.get("status")
    if status is not None:
        tasks = [t for t in tasks if t.get("status") == status]
    scope = filt.get("scope")
    if scope is not None:
        tasks = [t for t in tasks if t.get("scope") == scope]

    tasks.sort(key=lambda t: parse_dt(t["created_at"]), reverse=True)

    limit = filt.get("limit")
    if limit is not None:
        tasks = tasks[:limit]
    return tasks


def task_update(params):
    task_id = params["id"]
    patch = params.get("patch") or {}
    data = load_store(store_path())

    task = data["tasks"].get(task_id)
    if task is None:
        raise ProtocolError(f"no such task: {task_id}")
    task = dict(task)

    # Same order the built-in sqlite store applies a patch in: every `clear_*`
    # runs immediately before the field it clears, so a patch that (in theory)
    # carried both a clear and a replacement leaves the replacement standing --
    # and getting this order wrong is exactly how a successful retry keeps
    # showing the previous attempt's error or schedule.
    if patch.get("title") is not None:
        task["title"] = patch["title"]
    if patch.get("instructions") is not None:
        task["instructions"] = patch["instructions"]
    if patch.get("status") is not None:
        task["status"] = patch["status"]
    if patch.get("clear_schedule"):
        task["schedule"] = None
        task["next_run_at"] = None
    if patch.get("schedule") is not None:
        task["schedule"] = patch["schedule"]
    if patch.get("clear_estimate"):
        task["estimate_seconds"] = None
    if patch.get("estimate_seconds") is not None:
        task["estimate_seconds"] = patch["estimate_seconds"]
    if patch.get("scope") is not None:
        task["scope"] = patch["scope"]
    if patch.get("agent") is not None:
        task["agent"] = patch["agent"]
    if patch.get("runtime") is not None:
        task["runtime"] = patch["runtime"]
    if patch.get("clear_ack_timeout"):
        task["ack_timeout_seconds"] = None
    if patch.get("ack_timeout_seconds") is not None:
        task["ack_timeout_seconds"] = patch["ack_timeout_seconds"]
    if patch.get("clear_timeout"):
        task["timeout_seconds"] = None
    if patch.get("timeout_seconds") is not None:
        task["timeout_seconds"] = patch["timeout_seconds"]
    if patch.get("clear_result"):
        task["result"] = None
    if patch.get("result") is not None:
        task["result"] = patch["result"]
    if patch.get("clear_error"):
        task["error"] = None
    if patch.get("error") is not None:
        task["error"] = patch["error"]
    if patch.get("runs") is not None:
        task["runs"] = patch["runs"]
    if patch.get("last_run_at") is not None:
        task["last_run_at"] = patch["last_run_at"]
    if patch.get("next_run_at") is not None:
        task["next_run_at"] = patch["next_run_at"]
    if patch.get("labels") is not None:
        task["labels"] = patch["labels"]
    task["updated_at"] = now_iso()

    data["tasks"][task_id] = task
    atomic_write(store_path(), data)
    return task


def task_delete(params):
    data = load_store(store_path())
    existed = data["tasks"].pop(params["id"], None) is not None
    if existed:
        atomic_write(store_path(), data)
    return existed


def task_due(params):
    now = parse_dt(params["now"])
    due = []
    for task in load_store(store_path())["tasks"].values():
        # `pending` only: a task already dispatched, or one that finished, is
        # not waiting for the scheduler a second time just because its old
        # `next_run_at` is still sitting there in the past.
        if task.get("status") != "pending":
            continue
        next_run_at = task.get("next_run_at")
        if not next_run_at:
            continue
        if parse_dt(next_run_at) <= now:
            due.append(task)
    due.sort(key=lambda t: parse_dt(t["next_run_at"]))
    return due


METHODS = {
    "describe": describe,
    "task.create": task_create,
    "task.get": task_get,
    "task.list": task_list,
    "task.update": task_update,
    "task.delete": task_delete,
    "task.due": task_due,
}

# Everything else `TaskStore` can be asked. Runs, the journal, and standing
# agents all have a home in the instance's own ledger store instead -- see the
# module docstring for why a task-store plugin never has to make one up.
REFUSED = {
    "run.create",
    "run.get",
    "run.update",
    "run.list",
    "run.active",
    "run.active_all",
    "run.between",
    "run.entries",
    "agent.put",
    "agent.get",
    "agent.list",
    "agent.delete",
    "task.append_entry",
    "task.entries",
}


def refusal(method):
    # The daemon prefixes every plugin error with the adapter's own name
    # already (see `factory-plugins/src/host.rs`), so the text here names the
    # method instead -- the log line ends up "file-store: run.create: ...",
    # not "file-store: file-store: ...".
    return (
        f"{method}: this store keeps tasks only; runs, the journal, and "
        "standing agents live in the instance's own ledger store, not in a "
        "task-store plugin"
    )


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as exc:
            print(f"file-store: unreadable request: {exc}", file=sys.stderr)
            continue

        call_id = message.get("id")
        method = message.get("method", "")
        params = message.get("params") or {}

        if method in REFUSED:
            reply = {"id": call_id, "error": {"message": refusal(method)}}
        else:
            handler = METHODS.get(method)
            if handler is None:
                reply = {"id": call_id, "error": {"message": f"unknown method: {method}"}}
            else:
                try:
                    reply = {"id": call_id, "result": handler(params)}
                except ProtocolError as exc:
                    reply = {"id": call_id, "error": {"message": str(exc)}}
                except Exception as exc:  # a plugin must not die on one bad call
                    reply = {"id": call_id, "error": {"message": f"{type(exc).__name__}: {exc}"}}

        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
