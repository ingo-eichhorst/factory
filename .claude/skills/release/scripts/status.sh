#!/usr/bin/env bash
# What is running, and which commit it is running.
#
#   status.sh [env]
#
# "up" alone is not worth much -- an environment serving a month-old binary
# looks exactly like one serving this morning's. Every line says what was
# released, from where, and when.

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

report() {
  local env="$1" line state base tail_url
  base="$(http_base "$env")"
  tail_url="$(tailscale_url "$env" 2>/dev/null || echo unavailable)"

  if is_running "$env"; then
    if ! curl --noproxy '*' -fsS -o /dev/null --max-time 2 "$base/api/status" 2>/dev/null; then
      state="wedged"   # the process is alive and its LAN endpoint is not answering
    elif [ "$tail_url" = unavailable ] \
      || ! curl --noproxy '*' -fsS -o /dev/null --max-time 2 "$tail_url/api/status" 2>/dev/null; then
      state="partial"  # LAN is up but the promised Tailscale route is not
    else
      state="up"
    fi
  else
    state="down"
  fi

  line="$(printf '%-12s %-7s %s  lan %s' "$env" "$state" "$tail_url" "$base")"
  if commit="$(released_field "$env" commit 2>/dev/null)"; then
    line="$line  $(printf '%.9s' "$commit")"
    line="$line $(released_field "$env" ref 2>/dev/null || echo '?')"
    [ "$(released_field "$env" dirty 2>/dev/null || echo no)" = "yes" ] && line="$line (dirty)"
    line="$line  released $(released_field "$env" released_at 2>/dev/null || echo '?')"
  else
    line="$line  never released"
  fi
  printf '%s\n' "$line"
}

if [ $# -eq 1 ]; then
  report "$1"
  exit 0
fi

for env in $(standing_envs); do report "$env"; done
for env in $(known_envs); do
  standing_envs | grep -qx "$env" || report "$env"
done
