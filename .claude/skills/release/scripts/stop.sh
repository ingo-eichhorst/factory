#!/usr/bin/env bash
# Stop an environment and wait for it to actually be gone.
#
#   stop.sh <env>
#
# The waiting is the point. A daemon refuses to start while another still
# holds the instance's socket, so a stop that returns before the process has
# exited turns the next release into a confusing failure.

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

[ $# -eq 1 ] || { echo "usage: stop.sh <env>" >&2; exit 2; }
ENV_NAME="$1"

if [ "$(mode_for "$ENV_NAME")" = company ]; then
  die "$ENV_NAME is the launchd-managed company instance; stop it explicitly with launchctl bootout if downtime is intended"
fi

if ! PID="$(pid_of "$ENV_NAME")"; then
  note "$ENV_NAME is not running"
  rm -f "$(env_pid "$ENV_NAME")"
  exit 0
fi

kill "$PID" 2>/dev/null || true
if ! wait_for_exit "$PID"; then
  note "$ENV_NAME ignored a polite stop; killing $PID"
  kill -9 "$PID" 2>/dev/null || true
  wait_for_exit "$PID" || die "pid $PID is still alive"
fi

rm -f "$(env_pid "$ENV_NAME")"
note "$ENV_NAME stopped"
