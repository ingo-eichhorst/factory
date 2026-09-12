#!/usr/bin/env bash
# Bring the standing environments back up if they are not running.
#
#   ensure.sh [env ...]
#
# This restarts what was last released -- the binary already installed in the
# environment's own directory -- and never builds anything. A machine that
# rebooted, or a daemon that died, comes back on the same commit it was on,
# which is the only honest thing to do without being told to release something
# newer.
#
# Exits non-zero if anything that should be running is not, so it is safe to
# put on a timer.

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

# bash 3.2 is what ships on macOS, so no `mapfile` here.
targets=("$@")
if [ ${#targets[@]} -eq 0 ]; then
  while IFS= read -r env; do
    [ -n "$env" ] && targets[${#targets[@]}]="$env"
  done <<EOF
$(standing_envs)
EOF
fi
[ ${#targets[@]} -gt 0 ] || die "no environments declared in $ENVS_CONF"

failed=0
for env in "${targets[@]}"; do
  if is_running "$env"; then
    if wait_for_http "$(http_base "$env")" && verify_network_access "$env"; then
      note "$env already running on $(tailscale_url "$env") and $(http_base "$env")"
    else
      note "$env is running but is not reachable through every required network route"
      failed=1
    fi
    continue
  fi

  bin="$(env_bin "$env")"
  if [ ! -x "$bin" ]; then
    note "$env has never been released -- nothing to start (release.sh $env <ref>)"
    failed=1
    continue
  fi

  port="$(port_for "$env")"
  bind="$(env_bind "$env")"; bind="${bind:-$(lan_ipv4 || true)}"
  if [ -z "$bind" ]; then
    note "$env: cannot determine its LAN address"
    failed=1
    continue
  fi
  if [ "$(mode_for "$env")" != company ] && ! port_is_free "$bind" "$port"; then
    note "$env: $bind:$port is held by something else"
    failed=1
    continue
  fi

  if [ "$(mode_for "$env")" = company ]; then
    launchctl kickstart -k "gui/$(id -u)/$COMPANY_LABEL"
  else
    nohup "$bin" --root "$(env_root "$env")" run >>"$(env_log "$env")" 2>&1 &
    echo $! > "$(env_pid "$env")"
  fi

  if wait_for_http "$(http_base "$env")" && verify_network_access "$env"; then
    note "$env restarted on $(tailscale_url "$env") and $(http_base "$env") at $(released_field "$env" commit | cut -c1-9)"
  else
    note "$env did not come up; last lines of $(env_log "$env"):"
    tail -10 "$(env_log "$env")" >&2 || true
    failed=1
  fi
done

exit "$failed"
