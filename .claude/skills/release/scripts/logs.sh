#!/usr/bin/env bash
# Tail an environment's log.
#
#   logs.sh <env> [lines]     lines defaults to 60
#   logs.sh <env> -f          follow
#
# One log per environment, appended across releases, so the line above a
# restart is the reason for it.

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

[ $# -ge 1 ] || { echo "usage: logs.sh <env> [lines|-f]" >&2; exit 2; }
ENV_NAME="$1"; shift
LOG="$(env_log "$ENV_NAME")"
[ -r "$LOG" ] || die "$ENV_NAME has no log at $LOG"

case "${1:-60}" in
  -f) exec tail -f "$LOG" ;;
  *)  exec tail -n "${1:-60}" "$LOG" ;;
esac
