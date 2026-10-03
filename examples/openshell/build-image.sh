#!/bin/sh
# Build the sandbox image for Factory's `sandbox: openshell` (#218), and
# rebuild it whenever Factory's CLI, herdr, or the files beside this script
# change -- the image carries its own copy of each.
#
#   examples/openshell/build-image.sh            # docker if present, else rootfs
#   examples/openshell/build-image.sh docker     # tag $IMAGE_TAG with Docker/Podman
#   examples/openshell/build-image.sh rootfs     # write $OUT/factory-agent-rootfs.tar.gz
#
# `docker` is for a gateway on the Docker or Podman driver. `rootfs` needs no
# container engine at all -- it flattens the base image with `crane export`
# (brew install crane) and lays the overlay on top with bsdtar -- and is what
# a Mac running the MicroVM driver without Docker Desktop uses:
#
#   openshell:
#     image: /Users/you/.local/share/factory/openshell/factory-agent-rootfs.tar.gz
#
# Both produce the same filesystem: NVIDIA's community base image, plus
# Factory's CLI (a static aarch64 Linux build, cross-compiled here with
# rust-lld -- no Linux toolchain needed), herdr's and jq's Linux builds,
# Claude Code's managed settings, a git config that uses gh for GitHub, and a
# Claude Code home that has finished onboarding and trusts /sandbox/work.
#
# Environment:
#   BASE_IMAGE      default ghcr.io/nvidia/openshell-community/sandboxes/base:latest
#   HERDR_VERSION   default v0.9.3      (herdrdev/herdr release tag)
#   JQ_VERSION      default jq-1.8.2    (jqlang/jq release tag)
#   GIT_USER_NAME / GIT_USER_EMAIL   the commit identity inside the sandbox;
#                   default: this host's `git config user.name/user.email`
#   IMAGE_TAG       default factory-agent:latest            (docker mode)
#   OUT             default ~/.local/share/factory/openshell (rootfs mode)
set -eu

mode=${1:-auto}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
base=${BASE_IMAGE:-ghcr.io/nvidia/openshell-community/sandboxes/base:latest}
herdr_version=${HERDR_VERSION:-v0.9.3}
jq_version=${JQ_VERSION:-jq-1.8.2}
git_name=${GIT_USER_NAME:-$(git config --global user.name || true)}
git_email=${GIT_USER_EMAIL:-$(git config --global user.email || true)}
tag=${IMAGE_TAG:-factory-agent:latest}
out=${OUT:-$HOME/.local/share/factory/openshell}

if [ "$mode" = auto ]; then
  if command -v docker >/dev/null 2>&1; then mode=docker; else mode=rootfs; fi
fi
case "$mode" in docker|rootfs) ;; *) echo "usage: $0 [docker|rootfs]" >&2; exit 2 ;; esac

work=$(mktemp -d "${TMPDIR:-/tmp}/factory-openshell-image-XXXXXX")
trap 'rm -rf "$work"' EXIT
stage="$work/stage"
mkdir -p "$stage/usr/local/bin" "$stage/etc/claude-code" "$stage/sandbox"

echo "==> Factory CLI for aarch64 Linux (static, musl, linked by rust-lld)"
cargo=${CARGO:-cargo}
command -v "$cargo" >/dev/null 2>&1 || cargo="$HOME/.cargo/bin/cargo"
rustup=${RUSTUP:-rustup}
command -v "$rustup" >/dev/null 2>&1 || rustup="$HOME/.cargo/bin/rustup"
"$rustup" target add aarch64-unknown-linux-musl >/dev/null
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
  "$cargo" build --release -p factory-cli --target aarch64-unknown-linux-musl \
  --manifest-path "$repo/Cargo.toml" --target-dir "$work/target"
cp "$work/target/aarch64-unknown-linux-musl/release/factory" "$stage/usr/local/bin/factory"

echo "==> herdr $herdr_version and $jq_version (Linux aarch64)"
curl -fsSL -o "$stage/usr/local/bin/herdr" \
  "https://github.com/herdrdev/herdr/releases/download/$herdr_version/herdr-linux-aarch64"
curl -fsSL -o "$stage/usr/local/bin/jq" \
  "https://github.com/jqlang/jq/releases/download/$jq_version/jq-linux-arm64"
chmod 0755 "$stage/usr/local/bin/factory" "$stage/usr/local/bin/herdr" "$stage/usr/local/bin/jq"

cp "$here/managed-settings.json" "$stage/etc/claude-code/managed-settings.json"
cp "$here/claude.json" "$stage/sandbox/.claude.json"
{
  printf '[credential "https://github.com"]\n\thelper =\n\thelper = !/usr/bin/gh auth git-credential\n'
  printf '[credential "https://gist.github.com"]\n\thelper =\n\thelper = !/usr/bin/gh auth git-credential\n'
  if [ -n "$git_name" ]; then printf '[user]\n\tname = %s\n\temail = %s\n' "$git_name" "$git_email"; fi
} > "$stage/etc/gitconfig"

if [ "$mode" = docker ]; then
  echo "==> docker build -t $tag"
  docker build --build-arg "BASE_IMAGE=$base" -t "$tag" -f "$here/Dockerfile" "$stage"
  echo "built $tag -- set openshell.image: $tag"
  exit 0
fi

command -v crane >/dev/null 2>&1 || { echo "rootfs mode needs crane (brew install crane) to flatten $base" >&2; exit 1; }
echo "==> flattening $base (linux/arm64)"
crane export --platform linux/arm64 "$base" "$work/base.tar"

# The overlay as an mtree spec, so every entry gets the owner and mode it
# needs in the guest rather than whoever ran this script.
{
  echo "#mtree"
  for f in factory herdr jq; do
    echo "./usr/local/bin/$f type=file uid=0 gid=0 mode=0755 contents=$stage/usr/local/bin/$f"
  done
  echo "./etc/claude-code type=dir uid=0 gid=0 mode=0755"
  echo "./etc/claude-code/managed-settings.json type=file uid=0 gid=0 mode=0644 contents=$stage/etc/claude-code/managed-settings.json"
  echo "./etc/gitconfig type=file uid=0 gid=0 mode=0644 contents=$stage/etc/gitconfig"
  echo "./sandbox/.claude.json type=file uid=1000 gid=1000 mode=0644 contents=$stage/sandbox/.claude.json"
} > "$work/overlay.mtree"

mkdir -p "$out"
target="$out/factory-agent-rootfs.tar.gz"
echo "==> writing $target"
# Base first, overlay last: an extractor that meets a path twice keeps the
# later entry. The base's own policy file is dropped (see the Dockerfile).
bsdtar -czf "$target.partial" \
  --exclude 'etc/openshell/policy.yaml' --exclude './etc/openshell/policy.yaml' \
  "@$work/base.tar" "@$work/overlay.mtree"
mv "$target.partial" "$target"
echo "built $target -- set openshell.image: $target"
