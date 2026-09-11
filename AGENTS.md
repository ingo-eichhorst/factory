# Factory

A daemon that gives tasks to coding agents and watches what happens.

## Layout

    crates/factory-core      domain, events, wire protocol, the four adapter traits
    crates/factory-plugins   built-in adapters, the plugin host, the registry
    crates/factory-daemon    engine, scheduler, interfaces, the binary
    crates/factory-cli       the `factory` binary
    ui/index.html            the web UI, compiled into the daemon
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

## The rules that matter

- A task's status comes from the agent calling `factory task report`, never from
  looking at a terminal and guessing. Runtime status is a liveness signal only.
- Nothing Factory owns is written inside a scope. State lives in `.factory/`.
- A plugin that fails must never take the daemon down with it.
