#!/usr/bin/env bash
# Shared by every script in this skill. Nothing here starts, stops or builds
# anything -- it answers "where does this environment live, what port is it
# on, and is it up right now", so the four scripts that do act all agree on
# the answers.

set -euo pipefail

SKILL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENVS_CONF="$SKILL_DIR/envs.conf"

# Every environment lives outside the repository. A release must survive a
# branch switch, a `git clean`, and the worktree it was built from being
# removed, so the running instance owns its own copy of the binary and its own
# data directory and never reaches back into a checkout.
ENVS_HOME="${FACTORY_ENVS:-$HOME/factory-envs}"
# One warm cargo cache shared by every release build, so a throwaway build
# worktree does not mean a cold compile every time.
BUILD_CACHE="$ENVS_HOME/.cargo-target"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
note() { printf '%s\n' "$*" >&2; }

repo_root() {
  git -C "$SKILL_DIR" rev-parse --show-toplevel 2>/dev/null \
    || die "the release skill is not inside a git repository"
}

env_home() { printf '%s/%s\n' "$ENVS_HOME" "$1"; }
env_root() { printf '%s/root\n' "$(env_home "$1")"; }
env_bin()  { printf '%s/bin/factory-daemon\n' "$(env_home "$1")"; }
env_log()  { printf '%s/daemon.log\n' "$(env_home "$1")"; }
env_pid()  { printf '%s/daemon.pid\n' "$(env_home "$1")"; }
env_released() { printf '%s/RELEASED\n' "$(env_home "$1")"; }

conf_field() {
  # $1 env, $2 field number. Empty when the environment is not declared.
  [ -r "$ENVS_CONF" ] || return 0
  awk -v want="$1" -v col="$2" '!/^#/ && NF >= 3 && $1 == want { print $col }' "$ENVS_CONF"
}

# A declared environment keeps the port it is declared with. An ad-hoc one --
# a worktree somebody wants to look at before merging -- gets a port derived
# from its name, so the same name always comes back to the same port and two
# different names almost never collide.
port_for() {
  local p
  p="$(conf_field "$1" 2)"
  if [ -n "$p" ]; then printf '%s\n' "$p"; return; fi
  printf '%s\n' $(( 8800 + $(printf '%s' "$1" | cksum | cut -d' ' -f1) % 100 ))
}

policy_for() {
  local p
  p="$(conf_field "$1" 3)"
  printf '%s\n' "${p:-any}"
}

standing_envs() {
  [ -r "$ENVS_CONF" ] || return 0
  awk '!/^#/ && NF >= 3 { print $1 }' "$ENVS_CONF"
}

known_envs() {
  { standing_envs; [ -d "$ENVS_HOME" ] && find "$ENVS_HOME" -maxdepth 1 -mindepth 1 -type d \
      ! -name '.*' -exec basename {} \; ; } 2>/dev/null | sort -u
}

env_bind() {
  # What this environment last bound to, defaulting to loopback. Written by
  # release.sh; read by ensure.sh so a restart does not silently narrow or
  # widen who can reach it.
  released_field "$1" bind || true
}

released_field() {
  local f
  f="$(env_released "$1")"
  [ -r "$f" ] || return 1
  # The value is everything after the first ": ", timestamps and paths with
  # colons in them included.
  awk -v key="$2" '{ i = index($0, ": ") }
    i && substr($0, 1, i - 1) == key { print substr($0, i + 2); found = 1 }
    END { exit !found }' "$f"
}

pid_of() {
  local f
  f="$(env_pid "$1")"
  [ -r "$f" ] || return 1
  local pid
  pid="$(cat "$f")"
  [ -n "$pid" ] || return 1
  kill -0 "$pid" 2>/dev/null || return 1
  printf '%s\n' "$pid"
}

is_running() { pid_of "$1" >/dev/null 2>&1; }

http_base() {
  local bind port
  bind="$(env_bind "$1")"; bind="${bind:-127.0.0.1}"
  port="$(port_for "$1")"
  printf 'http://%s:%s\n' "$bind" "$port"
}

port_is_free() {
  ! lsof -nP -iTCP:"$1" -sTCP:LISTEN >/dev/null 2>&1
}

# Poll rather than sleep a fixed amount: a daemon that is up in 200ms should
# not cost two seconds, and one that is wedged should be reported as wedged
# rather than waited on forever.
wait_for_http() {
  local url="$1" tries="${2:-100}"
  local i=0
  while [ "$i" -lt "$tries" ]; do
    if curl -fsS -o /dev/null --max-time 2 "$url/api/status" 2>/dev/null; then return 0; fi
    i=$((i + 1))
    sleep 0.1 2>/dev/null || sleep 1
  done
  return 1
}

wait_for_exit() {
  local pid="$1" tries="${2:-100}"
  local i=0
  while [ "$i" -lt "$tries" ]; do
    kill -0 "$pid" 2>/dev/null || return 0
    i=$((i + 1))
    sleep 0.1 2>/dev/null || sleep 1
  done
  return 1
}
