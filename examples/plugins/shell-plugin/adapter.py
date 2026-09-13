#!/usr/bin/env python3
"""An agent adapter that lives outside the daemon.

It is deliberately the smallest thing that is still a real adapter: it runs the
task's instructions as a shell command and reports the outcome. Copy this file
to start a plugin of your own.

The protocol is line-delimited JSON on stdin and stdout:

    in   {"id": 1, "method": "agent.prompt", "params": {...}}
    out  {"id": 1, "result": {...}}
    out  {"id": 1, "error": {"message": "..."}}

Anything you write to stderr is the daemon's stderr. Anything you write to
stdout that is not a reply is ignored, so print your debugging to stderr.

An agent adapter answers two methods:

  describe          -- prove you are alive and speak the protocol
  agent.launch_spec -- how the runtime should bring your agent up
  agent.prompt      -- what to say to it once it is up

Both agent methods are given the whole context: the task, the working
directory, the path to the `factory` binary, the task's callback token,
`reporting_contract` (the exact wording the built-in agents use to tell an
agent how to report back), and `factory_guide` (the exact wording they use to
tell it what Factory is, who it is, and what its role lets it do). Paste both
rather than rewriting them.
"""

import json
import sys


def describe(_params):
    return {
        "name": "shell-plugin",
        "kind": "agent",
        "protocol": 1,
    }


def launch_spec(params):
    # An empty command means "use the shell that is already in the session".
    # A real agent would name a harness instead:
    #     {"kind": {"named": "gemini"}}
    return {
        "kind": {"command": []},
        "args": [],
        "env": params.get("env", {}),
    }


def prompt(params):
    task = params["task"]
    factory = params["factory_bin"]
    task_id = task["id"]
    command = (task.get("instructions") or "").strip() or "true"

    # One line, because it is typed at a shell prompt. The report is part of the
    # same line so the task cannot be left open by a command that succeeds and
    # then forgets to say so. The subshell keeps an instruction ending in `exit`
    # from taking the pane's shell with it.
    return {
        "prompt": (
            f"{factory} task report {task_id} --status running "
            f"--message 'shell-plugin started' >/dev/null; "
            f"if ( {command} ); then "
            f"{factory} task report {task_id} --status done --result 'command exited 0'; "
            f"else {factory} task report {task_id} --status failed --error \"command exited $?\"; fi"
        )
    }


METHODS = {
    "describe": describe,
    "agent.launch_spec": launch_spec,
    "agent.prompt": prompt,
}


def main():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as exc:
            print(f"shell-plugin: unreadable request: {exc}", file=sys.stderr)
            continue

        call_id = message.get("id")
        method = message.get("method", "")
        params = message.get("params") or {}

        handler = METHODS.get(method)
        if handler is None:
            reply = {"id": call_id, "error": {"message": f"unknown method: {method}"}}
        else:
            try:
                reply = {"id": call_id, "result": handler(params)}
            except Exception as exc:  # a plugin must not die on one bad call
                reply = {"id": call_id, "error": {"message": f"{type(exc).__name__}: {exc}"}}

        sys.stdout.write(json.dumps(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
