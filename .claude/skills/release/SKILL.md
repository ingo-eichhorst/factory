---
name: release
description: Release a commit, branch or worktree of this repo to an environment (staging, production, or an ad-hoc one) and keep the standing environments running. Use when asked to release, deploy, restage, roll back, check what is running where, or put a branch somewhere people can click it before it is merged.
---

# Release

Factory's UI is compiled into the daemon. Releasing is: build the commit,
install the binaries by atomic rename, restart the target, publish its port
through Tailscale Serve, and check `/api/status` through both Tailscale HTTPS
and the local network before claiming any of it worked.

The scripts do all of that. Run them; do not re-derive the steps.

    scripts/release.sh <env> <ref|worktree-path> [--bind ADDR] [--scope N=P] [--debug] [--force]
    scripts/status.sh  [env]          what is up, and which commit it is on
    scripts/ensure.sh  [env ...]      restart standing environments that died
    scripts/stop.sh    <env>
    scripts/logs.sh    <env> [n|-f]

`envs.conf` declares the standing environments -- **production** on 8790 and
**staging** on 8791 -- and both are meant to be up at all times. Staging is the
self-hosted company instance: its root is `~/business-factory`, its binaries
are installed in `~/.local/bin`, and launchd keeps it alive. Production keeps
an isolated instance root. Any other name is an isolated ad-hoc environment:
it gets a port derived from its name and a fresh instance, and it is there to
be thrown away. Pointing one at a worktree is how a branch gets looked at
before it is merged:

    scripts/release.sh review-13 ../worktrees/hall-floor

Release metadata lives under `~/factory-envs/<env>/`, along with `src/<sha>/`, the
exported tree a ref was built from (the newest three are kept). It is not a
scratch copy: the daemon's OpenShell provisioner builds the sandbox image's
`factory` CLI from it after the release has finished (`#234`). Isolated environments
also keep their binary, root and log there. Staging deliberately uses the live
`~/business-factory/.factory/` database and config so the UI shows the real
company scopes and tasks; a release never replaces that state.

Every successful release prints two URLs. Give the Tailscale HTTPS URL to the
requester (for example `https://factory.taile2330c.ts.net:8791/`) and also
report the LAN URL. Never hand off `127.0.0.1`, `0.0.0.0`, or an unverified URL.

## Every release is recorded

`release.sh` records each release as a deployment with the company daemon
(`#185`) -- whichever environment it went to -- so the L1 Operations tab and
`factory deploy list` show what runs where, since when and who put it there:
`factory deploy start --via release.sh` just before the swap, and `factory
deploy finish` once the environment is up and reachable, which also runs the
environment's declared health checks and records the release as failed if
they do not pass. A release that dies half-way is recorded as failed. The
first release made with a `factory` that predates `deploy` records the whole
deployment at the end instead. Recording never fails a release on its own: a
daemon that cannot be reached is a warning. Set `FACTORY_RELEASE_SCOPE` if the
scope is not called `factory`.

`RELEASED` still holds the latest release for these scripts; the history is
the daemon's. The two standing environments are declared in the `factory`
scope's `.factory/config.yaml` (`~/business-factory/projects/factory/.factory/`,
which is untracked: it is the company's config, not build output), so the
company daemon health-checks them and they carry an SLO. A scope config is read
only at daemon start: after editing it, restart the company daemon
(`launchctl kickstart -k gui/$(id -u)/com.business-factory.daemon`) once no
run is `running`. `envs.conf` stays the scripts' own table of port, release
policy and mode; the daemon does not read it, so a new standing environment
needs a line in both. The declaration in use:

```yaml
  environments:
    - name: production
      tier: production
      url: https://factory.taile2330c.ts.net:8790
      checks:
        - { name: status, kind: http, path: /api/status, expect: 200, body: '"instance":"production"', every: 60s, timeout: 5s, slow_after_ms: 2000 }
      slo: { availability: 99.5%, window: 28d }
    - name: staging
      tier: staging
      promotes_to: production
      url: https://factory.taile2330c.ts.net:8791
      checks:
        - { name: status, kind: http, path: /api/status, expect: 200, body: '"instance":"business-factory"', every: 60s, timeout: 5s, slow_after_ms: 2000 }
      slo: { availability: 99%, window: 28d }
```

## What the scripts will not do for you

- **Production takes only what is on `origin/main`.** The check is an ancestry
  test against the remote after a fetch, not against local `main`, which can be
  behind or diverged and still be called main. `--force` exists; if you reach
  for it, say in the same breath why.
- **An environment's data is not build output.** A release swaps the binary.
  The instance root, its config and its database stay. `--scope` is read on the
  first release of an environment and ignored afterwards, because by then that
  file is somebody's environment. Staging's existing company config and
  database are preserved; `--scope` only applies to a fresh isolated instance.
- **Every release is available through Tailscale and LAN.** The daemon binds
  the machine's current LAN address and Tailscale Serve terminates HTTPS on the
  tailnet DNS name. Do not bind `0.0.0.0`: Tailscale owns the same port on its
  virtual address, so a wildcard conflicts with the HTTPS listener.
- **These are trusted-network services.** The daemon can run shell commands and
  has no application-level authentication. Tailscale ACLs and the LAN boundary
  are the access controls. Never enable Tailscale Funnel or a public port
  forward as part of a release.

## Keeping the two up

`ensure.sh` restarts what was last released and repairs its Tailscale route -- it never builds, so a machine
that rebooted comes back on the commit it was already on rather than silently
moving forward. It exits non-zero when something that should be running is not,
so it works on a timer (`/loop`, `cron`, a launch agent); wiring that up is a
decision for whoever runs the machine, and this skill deliberately does not do
it behind their back.

Every restart or route repair first persists an offline action start in the
company instance root, using the current `FACTORY_RECORD_CLI` (`factory` by
default). Install a CLI with `recovery-journal` support before using this
version of `ensure.sh`. Recording does not need the company daemon running;
if the CLI or its durable start is unavailable, no restart or route repair is
attempted. A finish records the action's reported exit and observed LAN and
required-route probes. A missing finish remains unknown, never a guessed
Factory task status. The Operations page imports these separate standalone
receipts after restart; they are not deployments, declared health samples or
SLA evidence and do not bypass Factory recovery workflow approvals.
`FACTORY_RELEASE_SCOPE` selects the receipt scope. `FACTORY_ENVS_CONF` may
select an explicit alternative environment file. Neither option installs a
timer or changes a live declaration.

## When something is wrong

`status.sh` reports `wedged` for a process that is alive while its LAN endpoint
does not answer, and `partial` when LAN works but Tailscale does not. Either is
worth inspecting with `logs.sh <env>`. A failed isolated release stops the
daemon it started. Staging is launchd-managed and may be retried by launchd;
inspect its log, fix forward, or re-release the commit named in `RELEASED`.
