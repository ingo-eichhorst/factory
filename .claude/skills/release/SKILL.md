---
name: release
description: Release a commit, branch or worktree of this repo to an environment (staging, production, or an ad-hoc one) and keep the standing environments running. Use when asked to release, deploy, restage, roll back, check what is running where, or put a branch somewhere people can click it before it is merged.
---

# Release

Factory is one binary with the UI compiled into it, so an environment is a
port, a directory of its own, and a daemon. Releasing is: build the commit,
copy the binary into the environment's directory, restart it there, and check
that `/api/status` answers before claiming any of it worked.

The scripts do all of that. Run them; do not re-derive the steps.

    scripts/release.sh <env> <ref|worktree-path> [--bind ADDR] [--scope N=P] [--debug] [--force]
    scripts/status.sh  [env]          what is up, and which commit it is on
    scripts/ensure.sh  [env ...]      restart standing environments that died
    scripts/stop.sh    <env>
    scripts/logs.sh    <env> [n|-f]

`envs.conf` declares the standing environments -- **production** on 8790 and
**staging** on 8791 -- and both are meant to be up at all times. Any other name
is ad-hoc: it gets a port derived from its name and a fresh instance, and it is
there to be thrown away. Pointing one at a worktree is how a branch gets looked
at before it is merged:

    scripts/release.sh review-13 ../worktrees/hall-floor

Environments live under `~/factory-envs/<env>/` -- binary, instance root, log,
and a `RELEASED` file naming the commit. Nothing lives in the repository, so a
release survives a branch switch and a removed worktree.

## What the scripts will not do for you

- **Production takes only what is on `origin/main`.** The check is an ancestry
  test against the remote after a fetch, not against local `main`, which can be
  behind or diverged and still be called main. `--force` exists; if you reach
  for it, say in the same breath why.
- **An environment's data is not build output.** A release swaps the binary.
  The instance root, its config and its database stay. `--scope` is read on the
  first release of an environment and ignored afterwards, because by then that
  file is somebody's environment.
- **Never point an environment at the company's live `.factory/`.** It holds a
  real database and real secrets. Every environment gets its own root under
  `~/factory-envs/`, which is what the scripts do on their own.
- **The default bind is loopback.** `--bind` puts a daemon that runs shell
  commands on the network with no authentication in front of it. It is a
  deliberate act, not a convenience.

## Keeping the two up

`ensure.sh` restarts what was last released -- it never builds, so a machine
that rebooted comes back on the commit it was already on rather than silently
moving forward. It exits non-zero when something that should be running is not,
so it works on a timer (`/loop`, `cron`, a launch agent); wiring that up is a
decision for whoever runs the machine, and this skill deliberately does not do
it behind their back.

## When something is wrong

`status.sh` reports `wedged` for a process that is alive while its port does
not answer -- that is the state worth looking at, and `logs.sh <env>` is the
next command. A release that fails its health check stops the daemon it just
started and exits non-zero, leaving the previous binary in place but not
running: fix forward, or re-release the commit named in `RELEASED`.
