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
# rust-lld and a digest-pinned Zig C cross-compiler), herdr's and jq's Linux builds,
# Claude Code's managed settings, a git config that uses gh for GitHub, and a
# Claude Code home that has finished onboarding and trusts /sandbox/work.
#
# Environment:
#   BASE_IMAGE      default: the pinned community-base digest below
#   HERDR_VERSION   default v0.9.3      (herdrdev/herdr release tag)
#   JQ_VERSION      default jq-1.8.2    (jqlang/jq release tag)
#   HERDR_SHA256 / JQ_SHA256   required with non-default versions; otherwise
#                   the official release asset digests below are checked
#   ZIG_VERSION     default 0.14.1; downloaded for this host, never installed
#   ZIG_SHA256      required for a custom version; defaults below come from
#                   https://ziglang.org/download/index.json
#   GIT_USER_NAME / GIT_USER_EMAIL   the commit identity inside the sandbox;
#                   default: this host's `git config user.name/user.email`
#   IMAGE_TAG       default factory-agent:latest            (docker mode)
#   OUT             default ~/.local/share/factory/openshell (rootfs mode)
#   FACTORY_SOURCE  the Factory checkout the CLI is built from;
#                   default: the one this script sits in
#   FACTORY_TARGET_DIR  cargo's target directory for that build; default: a
#                   temporary one. Never inside the checkout's own target/,
#                   which a running daemon may be executing from.
#
# The daemon runs this itself (#234) for an agent whose openshell: block
# names no image: it writes the copy it carries to a directory of its own
# and sets FACTORY_SOURCE, FACTORY_TARGET_DIR and OUT.
set -eu

mode=${1:-auto}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "${FACTORY_SOURCE:-$here/../..}" && pwd)
base=${BASE_IMAGE:-ghcr.io/nvidia/openshell-community/sandboxes/base@sha256:aeef1c63f00e2913ea002ccb3aaf925f338b5c5d70e63576f0d95c16a138044e}
herdr_version=${HERDR_VERSION:-v0.9.3}
jq_version=${JQ_VERSION:-jq-1.8.2}
zig_version=${ZIG_VERSION:-0.14.1}
if [ "$herdr_version" = v0.9.3 ]; then
  herdr_sha256=${HERDR_SHA256:-4de7aa3e25678812e92960de64f7c2aaa1bca1f0f80a3c5e559837e231e1f5c0}
else
  herdr_sha256=${HERDR_SHA256:?HERDR_SHA256 is required for a custom HERDR_VERSION}
fi
if [ "$jq_version" = jq-1.8.2 ]; then
  jq_sha256=${JQ_SHA256:-8b85c817833814ddca00a144c33705546355afccf0cf39b188f3cdb48b852309}
else
  jq_sha256=${JQ_SHA256:?JQ_SHA256 is required for a custom JQ_VERSION}
fi
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

verify_digest() {
  actual=$(shasum -a 256 "$1" | cut -d ' ' -f 1)
  [ "$actual" = "$2" ] || { echo "SHA-256 mismatch for $1" >&2; exit 1; }
}

# The CLI now links bundled SQLite. rust-lld can link its objects, but
# cannot compile its C source or supply a target libc's headers (#272).
# Stage a pinned, host-native Zig; its cc/ar produce aarch64 musl objects
# without installing or changing any host toolchain or global configuration.
case "$(uname -s):$(uname -m)" in
  Darwin:arm64|Darwin:aarch64)
    zig_host=aarch64-macos
    zig_default_sha256=39f3dc5e79c22088ce878edc821dedb4ca5a1cd9f5ef915e9b3cc3053e8faefa ;;
  Darwin:x86_64)
    zig_host=x86_64-macos
    zig_default_sha256=b0f8bdfb9035783db58dd6c19d7dea89892acc3814421853e5752fe4573e5f43 ;;
  Linux:aarch64|Linux:arm64)
    zig_host=aarch64-linux
    zig_default_sha256=f7a654acc967864f7a050ddacfaa778c7504a0eca8d2b678839c21eea47c992b ;;
  Linux:x86_64)
    zig_host=x86_64-linux
    zig_default_sha256=24aeeec8af16c381934a6cd7d95c807a8cb2cf7df9fa40d359aa884195c4716c ;;
  *) echo "Factory image cross-build supports arm64/x86_64 macOS and Linux hosts" >&2; exit 1 ;;
esac
if [ "$zig_version" = 0.14.1 ]; then
  zig_sha256=${ZIG_SHA256:-$zig_default_sha256}
else
  zig_sha256=${ZIG_SHA256:?ZIG_SHA256 is required for a custom ZIG_VERSION}
fi
echo "==> Zig $zig_version ($zig_host) for bundled C dependencies"
curl --proto '=https' --proto-redir '=https' --tlsv1.2 --connect-timeout 30 --max-time 1800 -fsSL -o "$work/zig.tar.xz" \
  "https://ziglang.org/download/$zig_version/zig-$zig_host-$zig_version.tar.xz"
verify_digest "$work/zig.tar.xz" "$zig_sha256"
mkdir -p "$work/zig"
tar -xf "$work/zig.tar.xz" -C "$work/zig" --strip-components=1
# Fixed wrapper text keeps spaces and shell metacharacters in temp paths
# out of generated code. The path is supplied only to this cargo child.
printf '%s\n' '#!/bin/sh' \
  '# cc-rs recognizes clang and adds the Rust triple; Zig uses a three-part target.' \
  'for argument do' \
  '  shift' \
  '  case "$argument" in --target=aarch64-unknown-linux-musl) ;; *) set -- "$@" "$argument" ;; esac' \
  'done' \
  'exec "$FACTORY_IMAGE_ZIG_BIN" cc -target aarch64-linux-musl "$@"' > "$work/cc"
printf '%s\n' '#!/bin/sh' 'exec "$FACTORY_IMAGE_ZIG_BIN" ar "$@"' > "$work/ar"
chmod 0755 "$work/cc" "$work/ar"

echo "==> Factory CLI for aarch64 Linux (static, musl, linked by rust-lld)"
cargo=${CARGO:-cargo}
command -v "$cargo" >/dev/null 2>&1 || cargo="$HOME/.cargo/bin/cargo"
rustup=${RUSTUP:-rustup}
command -v "$rustup" >/dev/null 2>&1 || rustup="$HOME/.cargo/bin/rustup"
"$rustup" target add aarch64-unknown-linux-musl >/dev/null
target_dir=${FACTORY_TARGET_DIR:-$work/target}
FACTORY_IMAGE_ZIG_BIN="$work/zig/zig" \
  ZIG_LOCAL_CACHE_DIR="$work/zig-local-cache" ZIG_GLOBAL_CACHE_DIR="$work/zig-global-cache" \
  CC_aarch64_unknown_linux_musl="$work/cc" AR_aarch64_unknown_linux_musl="$work/ar" \
  CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
  "$cargo" build --release -p factory-cli --target aarch64-unknown-linux-musl \
  --manifest-path "$repo/Cargo.toml" --target-dir "$target_dir"
cp "$target_dir/aarch64-unknown-linux-musl/release/factory" "$stage/usr/local/bin/factory"

echo "==> herdr $herdr_version and $jq_version (Linux aarch64)"
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o "$stage/usr/local/bin/herdr" \
  "https://github.com/herdrdev/herdr/releases/download/$herdr_version/herdr-linux-aarch64"
curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL -o "$stage/usr/local/bin/jq" \
  "https://github.com/jqlang/jq/releases/download/$jq_version/jq-linux-arm64"
verify_digest "$stage/usr/local/bin/herdr" "$herdr_sha256"
verify_digest "$stage/usr/local/bin/jq" "$jq_sha256"
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
  docker build --platform linux/arm64 --build-arg "BASE_IMAGE=$base" -t "$tag" -f "$here/Dockerfile" "$stage"
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
