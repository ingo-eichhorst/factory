#!/usr/bin/env bash
# Put a commit in front of people: build it, install the binary into the
# environment's own directory, restart the daemon on that environment's port,
# and refuse to say it worked until `/api/status` answers.
#
#   release.sh <env> <ref|worktree-path> [options]
#
# The environment's data outlives the release. A release swaps the binary and
# nothing else -- the instance root, its config and its database stay where
# they are, which is what makes `staging` and `production` mean anything.

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

usage() {
  cat >&2 <<'USAGE'
usage: release.sh <env> <ref|worktree-path> [options]

  <env>               production, staging, or any name for an ad-hoc environment
  <ref>               a branch, tag or commit -- resolved after `git fetch`
  <worktree-path>     a directory to build as it stands, for looking at work
                      before it is merged

options:
  --bind ADDR         LAN address to listen on (default: this machine's current
                      default-route IPv4 address). Do not use a wildcard: it
                      conflicts with Tailscale Serve on the same port.
  --scope NAME=PATH   declare a scope on first release of this environment.
                      Repeatable. Later releases leave the config alone.
  --debug             build without optimisation -- faster, for ad-hoc looks
  --force             skip the policy check (see envs.conf)
USAGE
  exit 2
}

[ $# -ge 2 ] || usage
ENV_NAME="$1"; SOURCE="$2"; shift 2

BIND=""
PROFILE="release"
FORCE=0
SCOPES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --bind) [ $# -ge 2 ] || usage; BIND="$2"; shift 2 ;;
    --scope) [ $# -ge 2 ] || usage; SCOPES+=("$2"); shift 2 ;;
    --debug) PROFILE="debug"; shift ;;
    --force) FORCE=1; shift ;;
    -h|--help) usage ;;
    *) die "unknown option: $1" ;;
  esac
done

BIND="${BIND:-$(lan_ipv4 || true)}"
[ -n "$BIND" ] || die "cannot determine the local-network IPv4 address; pass --bind ADDR"
case "$BIND" in
  0.0.0.0|127.*|localhost|::1) die "--bind must be a LAN address, not $BIND" ;;
esac

case "$ENV_NAME" in
  ''|*[!a-zA-Z0-9_-]*) die "environment name must be letters, digits, - or _" ;;
esac

PORT="$(port_for "$ENV_NAME")"
POLICY="$(policy_for "$ENV_NAME")"
MODE="$(mode_for "$ENV_NAME")"
HOME_DIR="$(env_home "$ENV_NAME")"
REPO="$(repo_root)"

# ---------------------------------------------------------------- the source

BUILD_DIR=""
TEMP_WORKTREE=""
# The deployment recorded with the company daemon, once the swap begins, and
# whether its ending has been recorded yet. Recording is best-effort: a
# daemon that is down, or a `factory` from before `deploy` existed, is a
# warning, never a failed release.
DEPLOY_ID=""
DEPLOY_DONE=0
DEPLOY_STARTED=""
cleanup() {
  if [ -n "$DEPLOY_ID" ] && [ "$DEPLOY_DONE" -eq 0 ]; then
    record deploy finish "$DEPLOY_ID" --status failed \
      --reason "release.sh stopped before $ENV_NAME was up and reachable" >/dev/null 2>&1 || true
  fi
  if [ -n "$TEMP_WORKTREE" ] && [ -d "$TEMP_WORKTREE" ]; then
    git -C "$REPO" worktree remove --force "$TEMP_WORKTREE" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if [ -d "$SOURCE" ]; then
  # A worktree, built as it stands -- uncommitted changes included, which is
  # the whole point of pointing an environment at one.
  BUILD_DIR="$(cd "$SOURCE" && pwd)"
  git -C "$BUILD_DIR" rev-parse --git-dir >/dev/null 2>&1 \
    || die "$BUILD_DIR is not a git worktree"
  SHA="$(git -C "$BUILD_DIR" rev-parse HEAD)"
  REF="$(git -C "$BUILD_DIR" rev-parse --abbrev-ref HEAD)"
  DIRTY=no
  [ -z "$(git -C "$BUILD_DIR" status --porcelain)" ] || DIRTY=yes
  SOURCE_DESC="worktree $BUILD_DIR"
else
  # A ref, resolved against the remote rather than whatever the local branch
  # happens to say: a local `main` can be six commits behind and still look
  # like main.
  git -C "$REPO" fetch origin --quiet || note "warning: git fetch failed, using what is already here"
  SHA="$(git -C "$REPO" rev-parse --verify --quiet "${SOURCE}^{commit}" || true)"
  [ -n "$SHA" ] || die "cannot resolve '$SOURCE' to a commit"
  REF="$SOURCE"
  DIRTY=no
  SOURCE_DESC="ref $SOURCE"
  TEMP_WORKTREE="$REPO/../worktrees/.release-$ENV_NAME"
  git -C "$REPO" worktree remove --force "$TEMP_WORKTREE" >/dev/null 2>&1 || true
  git -C "$REPO" worktree add --detach --quiet "$TEMP_WORKTREE" "$SHA" \
    || die "could not check $SHA out into a build worktree"
  BUILD_DIR="$(cd "$TEMP_WORKTREE" && pwd)"
fi

if [ "$POLICY" = "main-only" ] && [ "$FORCE" -eq 0 ]; then
  git -C "$REPO" fetch origin --quiet || true
  git -C "$REPO" merge-base --is-ancestor "$SHA" origin/main 2>/dev/null \
    || die "$ENV_NAME takes only commits that are on origin/main -- $(git -C "$REPO" rev-parse --short "$SHA") is not. Land it through a pull request, or pass --force and say why."
  [ "$DIRTY" = "no" ] \
    || die "$ENV_NAME will not take a dirty worktree. Commit it, land it, then release."
fi

DESCRIBE="$(git -C "$REPO" describe --tags --always "$SHA" 2>/dev/null || git -C "$REPO" rev-parse --short "$SHA")"
note "releasing $ENV_NAME <- $SOURCE_DESC ($(git -C "$REPO" rev-parse --short "$SHA")${DIRTY:+, dirty=$DIRTY})"

# ----------------------------------------------------------------- the build

mkdir -p "$BUILD_CACHE" "$HOME_DIR/bin" "$(env_root "$ENV_NAME")"
note "building ($PROFILE) in $BUILD_DIR"
if [ "$PROFILE" = "release" ]; then
  ( cd "$BUILD_DIR" && CARGO_TARGET_DIR="$BUILD_CACHE" cargo build --workspace --release --quiet )
  BUILT="$BUILD_CACHE/release/factory-daemon"
  BUILT_CLI="$BUILD_CACHE/release/factory"
else
  ( cd "$BUILD_DIR" && CARGO_TARGET_DIR="$BUILD_CACHE" cargo build --workspace --quiet )
  BUILT="$BUILD_CACHE/debug/factory-daemon"
  BUILT_CLI="$BUILD_CACHE/debug/factory"
fi
[ -x "$BUILT" ] || die "the build produced no factory-daemon at $BUILT"
[ -x "$BUILT_CLI" ] || die "the build produced no factory CLI at $BUILT_CLI"

# -------------------------------------------------------------- the instance

ROOT="$(env_root "$ENV_NAME")"
CONFIG="$ROOT/.factory/config.yaml"
if [ ! -f "$CONFIG" ]; then
  [ "$MODE" != company ] \
    || die "the company instance has no config at $CONFIG; initialise it deliberately before releasing"
  note "first release of $ENV_NAME -- writing a new instance root at $ROOT"
  "$BUILT" --root "$ROOT" init >/dev/null
  FRESH=yes
else
  FRESH=no
fi

# `init` declares the instance root itself as a scope, which is right for a
# person's own checkout and wrong here: an environment's data directory is not
# a codebase to dispatch work into. On a fresh instance the scopes are replaced
# by whatever `--scope` said, or by none at all. An instance that already
# exists keeps its config: that is somebody's environment, not build output.
# The company instance is never fresh here; only its HTTP bind is changed.
python3 - "$CONFIG" "$BIND" "$PORT" "$FRESH" "$ENV_NAME" "${SCOPES[@]+"${SCOPES[@]}"}" <<'PY'
import sys, pathlib

config, bind, port, fresh, env_name, *scopes = sys.argv[1:]
text = pathlib.Path(config).read_text()

# The http interface's `bind` is the one thing every release must own: the
# environment's port is not the instance's to remember across a rebind.
lines, out, in_http = text.split("\n"), [], False
for line in lines:
    stripped = line.strip()
    if stripped == "- kind: http":
        in_http = True
        out.append(line)
        indent = line[:len(line) - len(line.lstrip())]
        out.append("%s  bind: %s:%s" % (indent, bind, port))
        continue
    if in_http:
        if stripped.startswith("bind:"):
            continue                      # replaced above
        if stripped.startswith("- kind:") or (line and not line.startswith(" ")):
            in_http = False
    out.append(line)
text = "\n".join(out)

if fresh == "yes":
    # Replace the whole `scopes:` block -- its key line plus the list under it,
    # which is every following line that is indented or starts a list item.
    # A regex over this is how the first version of this script wrote a config
    # the daemon would not parse.
    block = "scopes: []" if not scopes else "scopes:\n" + "\n".join(
        "- name: %s\n  path: %s" % tuple(s.split("=", 1)) for s in scopes)
    lines, out, i = text.split("\n"), [], 0
    while i < len(lines):
        if lines[i].rstrip() == "scopes:" or lines[i].startswith("scopes:"):
            out.append(block)
            i += 1
            while i < len(lines) and (lines[i][:1] in (" ", "\t", "-") or lines[i] == ""):
                if lines[i] == "" and i + 1 < len(lines) and lines[i + 1][:1] not in (" ", "\t", "-"):
                    break
                i += 1
            continue
        out.append(lines[i])
        i += 1
    text = "\n".join(out)

    # `init` names the instance after the directory it was written into, which
    # here is every environment's `root`. Name it for the environment instead --
    # it is what the UI puts in its header, and "root" tells nobody anything.
    lines, out, in_instance = text.split("\n"), [], False
    for line in lines:
        if line.startswith("instance:"):
            in_instance = True
        elif line and not line.startswith(" "):
            in_instance = False
        if in_instance and line.startswith("  name: "):
            line = "  name: %s" % env_name
        out.append(line)
    text = "\n".join(out)

pathlib.Path(config).write_text(text)
PY

# ----------------------------------------------------------------- the swap

COMMITTED_AT="$(git -C "$REPO" show -s --format=%cI "$SHA" 2>/dev/null || true)"
RELEASE_ARGS=(--env "$ENV_NAME" --scope "$RECORD_SCOPE" --commit "$SHA" --describe "$DESCRIBE"
  --profile "$PROFILE" --source "$SOURCE_DESC" --via release.sh)
[ -z "$COMMITTED_AT" ] || RELEASE_ARGS+=(--committed-at "$COMMITTED_AT")
[ "$DIRTY" = "no" ] || RELEASE_ARGS+=(--dirty)
DEPLOY_STARTED="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
if DEPLOY_ID="$(record deploy start "${RELEASE_ARGS[@]}" 2>/dev/null)"; then
  note "recording deployment $DEPLOY_ID"
else
  DEPLOY_ID=""
  note "warning: could not record the deployment's start; it is recorded once $ENV_NAME is up"
fi

if [ "$MODE" = company ]; then
  # This is the self-hosted company daemon. It runs from ~/.local/bin under
  # launchd; rename both binaries before kickstart so an agent callback and the
  # daemon always speak the same wire protocol.
  mkdir -p "$(dirname "$(env_bin "$ENV_NAME")")"
  cp "$BUILT" "$(env_bin "$ENV_NAME").new"
  chmod +x "$(env_bin "$ENV_NAME").new"
  mv "$(env_bin "$ENV_NAME").new" "$(env_bin "$ENV_NAME")"
  cp "$BUILT_CLI" "$(env_cli_bin).new"
  chmod +x "$(env_cli_bin).new"
  mv "$(env_cli_bin).new" "$(env_cli_bin)"
else
  "$(dirname "${BASH_SOURCE[0]}")/stop.sh" "$ENV_NAME" >/dev/null 2>&1 || true
  port_is_free "$BIND" "$PORT" \
    || die "$BIND:$PORT is still held by something that is not this environment"
  cp "$BUILT" "$(env_bin "$ENV_NAME").new"
  chmod +x "$(env_bin "$ENV_NAME").new"
  mv "$(env_bin "$ENV_NAME").new" "$(env_bin "$ENV_NAME")"
fi

LOG="$(env_log "$ENV_NAME")"
if [ "$MODE" = company ]; then
  launchctl kickstart -k "gui/$(id -u)/$COMPANY_LABEL"
else
  nohup "$(env_bin "$ENV_NAME")" --root "$ROOT" run >>"$LOG" 2>&1 &
  echo $! > "$(env_pid "$ENV_NAME")"
fi

BASE="http://$BIND:$PORT"
if ! wait_for_http "$BASE"; then
  note "--- last 20 lines of $LOG ---"
  tail -20 "$LOG" >&2 || true
  if [ "$MODE" != company ]; then
    "$(dirname "${BASH_SOURCE[0]}")/stop.sh" "$ENV_NAME" >/dev/null 2>&1 || true
  fi
  die "$ENV_NAME did not come up on $BASE"
fi

if ! verify_network_access "$ENV_NAME" "$BASE"; then
  if [ "$MODE" != company ]; then
    "$(dirname "${BASH_SOURCE[0]}")/stop.sh" "$ENV_NAME" >/dev/null 2>&1 || true
  fi
  die "$ENV_NAME is not reachable through both Tailscale and the local network"
fi

TAILSCALE_URL="$(tailscale_url "$ENV_NAME")"

# The ending, which runs the environment's declared health checks before a
# success is recorded as one. A start that could not be recorded -- the first
# release of a CLI that has `deploy` -- is recorded whole now instead.
DEPLOY_DONE=1
if [ -n "$DEPLOY_ID" ]; then
  RECORDED="$(record deploy finish "$DEPLOY_ID" --status succeeded 2>&1)" && RECORD_OK=1 || RECORD_OK=0
else
  RECORDED="$(record deploy record "${RELEASE_ARGS[@]}" --started-at "$DEPLOY_STARTED" --status succeeded 2>&1)" \
    && RECORD_OK=1 || RECORD_OK=0
fi
if [ "$RECORD_OK" -eq 1 ]; then
  note "$RECORDED"
elif printf '%s' "$RECORDED" | grep -q "recorded as failed"; then
  note "$RECORDED"
  die "$ENV_NAME is up, but its declared health checks did not pass; the deployment is recorded as failed"
else
  note "warning: the deployment was not recorded: $RECORDED"
fi

cat > "$(env_released "$ENV_NAME")" <<EOF
env: $ENV_NAME
ref: $REF
commit: $SHA
describe: $DESCRIBE
dirty: $DIRTY
source: $SOURCE_DESC
profile: $PROFILE
mode: $MODE
bind: $BIND
port: $PORT
root: $ROOT
tailscale_url: $TAILSCALE_URL
lan_url: $BASE
released_at: $(date -u +%Y-%m-%dT%H:%M:%SZ)
EOF

note ""
note "$ENV_NAME is up at $TAILSCALE_URL/"
note "  local    $BASE/"
note "  commit   $(git -C "$REPO" rev-parse --short "$SHA") ($DESCRIBE) from $SOURCE_DESC"
note "  root     $ROOT"
note "  log      $LOG"
note ""
"$(dirname "${BASH_SOURCE[0]}")/status.sh"
