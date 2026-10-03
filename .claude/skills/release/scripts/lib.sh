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
COMPANY_ROOT="${FACTORY_RELEASE_ROOT:-$HOME/business-factory}"
COMPANY_LABEL="${FACTORY_LAUNCHD_LABEL:-com.business-factory.daemon}"
# One warm cargo cache shared by every release build, so a throwaway build
# worktree does not mean a cold compile every time.
BUILD_CACHE="$ENVS_HOME/.cargo-target"

# Every deployment is recorded with the company daemon (`#185`), whichever
# environment it went to: that is the instance whose Operations tab lists
# them, and the one the `factory` scope this repo is lives in.
RECORD_CLI="${FACTORY_RECORD_CLI:-$HOME/.local/bin/factory}"
RECORD_SCOPE="${FACTORY_RELEASE_SCOPE:-factory}"

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
note() { printf '%s\n' "$*" >&2; }

repo_root() {
  git -C "$SKILL_DIR" rev-parse --show-toplevel 2>/dev/null \
    || die "the release skill is not inside a git repository"
}

env_home() { printf '%s/%s\n' "$ENVS_HOME" "$1"; }
mode_for() {
  local mode
  mode="$(conf_field "$1" 4)"
  printf '%s\n' "${mode:-isolated}"
}

env_root() {
  if [ "$(mode_for "$1")" = company ]; then
    printf '%s\n' "$COMPANY_ROOT"
  else
    printf '%s/root\n' "$(env_home "$1")"
  fi
}
env_bin() {
  if [ "$(mode_for "$1")" = company ]; then
    printf '%s/.local/bin/factory-daemon\n' "$HOME"
  else
    printf '%s/bin/factory-daemon\n' "$(env_home "$1")"
  fi
}
env_cli_bin() { printf '%s/.local/bin/factory\n' "$HOME"; }
env_log() {
  if [ "$(mode_for "$1")" = company ]; then
    printf '%s/.factory/logs/daemon.log\n' "$COMPANY_ROOT"
  else
    printf '%s/daemon.log\n' "$(env_home "$1")"
  fi
}
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
  # The company service follows the machine's current LAN address. Isolated
  # environments keep the address they were released with so ensure.sh never
  # silently widens or narrows their reach.
  if [ "$(mode_for "$1")" = company ]; then
    lan_ipv4 || true
  else
    released_field "$1" bind || true
  fi
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

is_running() {
  if [ "$(mode_for "$1")" = company ]; then
    launchctl print "gui/$(id -u)/$COMPANY_LABEL" 2>/dev/null \
      | grep -q 'state = running'
  else
    pid_of "$1" >/dev/null 2>&1
  fi
}

lan_ipv4() {
  local iface addr
  if command -v route >/dev/null 2>&1 && command -v ipconfig >/dev/null 2>&1; then
    iface="$(route -n get default 2>/dev/null | awk '/interface:/{print $2; exit}')"
    if [ -n "$iface" ]; then
      addr="$(ipconfig getifaddr "$iface" 2>/dev/null || true)"
      if [ -n "$addr" ]; then printf '%s\n' "$addr"; return 0; fi
    fi
  fi
  if command -v ip >/dev/null 2>&1; then
    iface="$(ip route show default 2>/dev/null | awk 'NR == 1 { print $5 }')"
    if [ -n "$iface" ]; then
      addr="$(ip -4 -o addr show dev "$iface" scope global 2>/dev/null | awk 'NR == 1 { split($4, a, "/"); print a[1] }')"
      if [ -n "$addr" ]; then printf '%s\n' "$addr"; return 0; fi
    fi
  fi
  return 1
}

http_base() {
  local bind
  bind="$(env_bind "$1")"
  bind="${bind:-$(lan_ipv4)}"
  printf 'http://%s:%s\n' "$bind" "$(port_for "$1")"
}

tailscale_bin() {
  if command -v tailscale >/dev/null 2>&1; then
    command -v tailscale
  elif [ -x /Applications/Tailscale.app/Contents/MacOS/Tailscale ]; then
    printf '%s\n' /Applications/Tailscale.app/Contents/MacOS/Tailscale
  else
    return 1
  fi
}

tailscale_dns_name() {
  local bin
  bin="$(tailscale_bin)" || return 1
  "$bin" status --json 2>/dev/null | python3 -c '
import json, sys
name = json.load(sys.stdin).get("Self", {}).get("DNSName", "").rstrip(".")
if not name:
    raise SystemExit(1)
print(name)
'
}

tailscale_url() {
  local name
  name="$(tailscale_dns_name)" || return 1
  printf 'https://%s:%s\n' "$name" "$(port_for "$1")"
}

configure_tailscale_serve() {
  local env="$1" target="${2:-}" bin port
  bin="$(tailscale_bin)" || { note "Tailscale CLI is unavailable"; return 1; }
  port="$(port_for "$env")"
  target="${target:-$(http_base "$env")}"
  "$bin" serve --bg --yes --https="$port" "$target" >/dev/null
}

verify_network_access() {
  local env="$1" local_url="${2:-}" tail_url
  local_url="${local_url:-$(http_base "$env")}"
  configure_tailscale_serve "$env" "$local_url" || return 1
  tail_url="$(tailscale_url "$env")" \
    || { note "$env: cannot determine its Tailscale URL"; return 1; }
  wait_for_http "$tail_url" \
    || { note "$env: Tailscale URL did not answer: $tail_url"; return 1; }
  wait_for_http "$local_url" \
    || { note "$env: local-network URL did not answer: $local_url"; return 1; }
}

port_is_free() {
  ! lsof -nP -iTCP@"$1":"$2" -sTCP:LISTEN >/dev/null 2>&1
}

# Poll rather than sleep a fixed amount: a daemon that is up in 200ms should
# not cost two seconds, and one that is wedged should be reported as wedged
# rather than waited on forever.
wait_for_http() {
  local url="$1" tries="${2:-100}"
  local i=0
  while [ "$i" -lt "$tries" ]; do
    if curl --noproxy '*' -fsS -o /dev/null --max-time 2 "$url/api/status" 2>/dev/null; then return 0; fi
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

# `factory` against the company daemon, for recording a deployment. A run
# that calls release.sh keeps its own FACTORY_TOKEN, so the deployment is
# recorded as that run's.
record() {
  [ -x "$RECORD_CLI" ] || return 1
  FACTORY_ROOT="$COMPANY_ROOT" "$RECORD_CLI" --root "$COMPANY_ROOT" "$@"
}
