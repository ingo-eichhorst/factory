#!/bin/sh
# Factory's built/running dependency evidence. Use `built` in the release
# node and `running` on the installed ~/.local/bin binaries. Production and
# storage remain ordinary task callbacks; the daemon never invokes a scanner.
set -eu

mode="${1:-}"
case "$mode" in
  built|running) ;;
  *) echo "usage: $0 built|running" >&2; exit 2 ;;
esac

factory_bin="${FACTORY_BIN:-factory}"
scan_dir="$(mktemp -d "${TMPDIR:-/tmp}/factory-product-scan.XXXXXX")"
trap 'rm -rf "$scan_dir"' EXIT HUP INT TERM

for tool in syft grype jq; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 1; }
done

identity_from_binary() {
  "$1" --version | sed -n 's/^[^ ]* \([^ ]*\) (\([^)]*\))$/\1 \2/p'
}

if [ "$mode" = built ]; then
  command -v cargo-auditable >/dev/null || { echo "cargo-auditable is required" >&2; exit 1; }
  git_sha="$(git rev-parse HEAD)"
  FACTORY_GIT_SHA="$git_sha" cargo auditable build --workspace --release
  cli="target/release/factory"
  daemon="target/release/factory-daemon"
  phase=build
else
  install_dir="${FACTORY_INSTALL_DIR:-${HOME:?HOME is not set}/.local/bin}"
  cli="$install_dir/factory"
  daemon="$install_dir/factory-daemon"
  phase=operations
fi

test -x "$cli" || { echo "$cli is not an executable" >&2; exit 1; }
test -x "$daemon" || { echo "$daemon is not an executable" >&2; exit 1; }

cli_identity="$(identity_from_binary "$cli")"
daemon_identity="$(identity_from_binary "$daemon")"
test -n "$cli_identity" || { echo "$cli has no embedded Factory release identity" >&2; exit 1; }
test "$cli_identity" = "$daemon_identity" || {
  echo "installed factory and factory-daemon identities differ" >&2
  exit 1
}
version="${cli_identity%% *}"
git_sha="${cli_identity#* }"
test "$git_sha" != unknown || { echo "the binaries have no embedded git commit" >&2; exit 1; }

cli_sbom="$scan_dir/cli.cdx.json"
daemon_sbom="$scan_dir/daemon.cdx.json"
sbom="$scan_dir/$mode-sbom.cdx.json"
vulnerabilities="$scan_dir/$mode-vulnerabilities.cdx.json"

# Syft reads cargo-auditable's embedded dependency data from each exact binary;
# jq folds the pair into one product SBOM and adds Factory's release key.
syft "file:$cli" -o "cyclonedx-json=$cli_sbom"
syft "file:$daemon" -o "cyclonedx-json=$daemon_sbom"
jq -s --arg phase "$phase" --arg version "$version" --arg git_sha "$git_sha" '
  . as $documents
  | ("pkg:generic/factory@" + $version) as $product_ref
  | [$documents[].metadata.component."bom-ref"] as $binary_refs
  | .[0]
  | .metadata.timestamp = (now | todateiso8601)
  | .metadata.lifecycles = [{phase: $phase}]
  | .metadata.component = {
      type: "application", name: "factory", version: $version,
      "bom-ref": $product_ref,
      properties: [{name: "factory:git-sha", value: $git_sha}]
    }
  | .components = ([$documents[].metadata.component, $documents[].components[]?] | unique_by(."bom-ref"))
  | .dependencies = ([{ref: $product_ref, dependsOn: $binary_refs}, $documents[].dependencies[]?] | unique_by(.ref))
  | del(.vulnerabilities)
' "$cli_sbom" "$daemon_sbom" > "$sbom"

grype "sbom:$sbom" -o "cyclonedx-json=$vulnerabilities"
"$factory_bin" task attach --kind sbom "$sbom"
"$factory_bin" task attach --kind vulnerabilities "$vulnerabilities"
