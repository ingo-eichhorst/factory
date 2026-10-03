#!/bin/sh
# Declared-state example for one ordinary shell task in a dependency-scan
# workflow. Syft and Grype are examples chosen by this scope, not Factory
# dependencies and not a scanner adapter.
set -eu

case "${1:-}" in
  "" ) with_reachability=false ;;
  --reachability ) with_reachability=true ;;
  * ) echo "usage: $0 [--reachability]" >&2; exit 2 ;;
esac
script_dir="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"

factory_bin="${FACTORY_BIN:-factory}"
scan_dir="$(mktemp -d "${TMPDIR:-/tmp}/factory-dependency-scan.XXXXXX")"
trap 'rm -rf "$scan_dir"' EXIT HUP INT TERM

raw_sbom="$scan_dir/syft.cdx.json"
sbom="$scan_dir/declared-sbom.cdx.json"
vulnerabilities="$scan_dir/vulnerabilities.cdx.json"
vex="$scan_dir/authored-vex.cdx.json"

command -v syft >/dev/null || { echo "syft is required" >&2; exit 1; }
command -v grype >/dev/null || { echo "grype is required" >&2; exit 1; }
command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }
if [ "$with_reachability" = true ]; then
  command -v osv-scanner >/dev/null || { echo "osv-scanner v2 is required for call analysis" >&2; exit 1; }
  command -v node >/dev/null || { echo "node is required for the reachability evidence helper" >&2; exit 1; }
fi

syft dir:. -o "cyclonedx-json=$raw_sbom"
jq '.metadata.lifecycles = [{"phase":"pre-build"}] | del(.vulnerabilities)' \
  "$raw_sbom" > "$sbom"

# Keep the authored judgments available to a scanner or conversion step. Grype
# does not consume CycloneDX VEX directly, so this minimal example only saves
# the standard document; a real scope workflow can convert it before scanning.
"$factory_bin" dependencies vex "${FACTORY_SCOPE:?FACTORY_SCOPE is not set}" > "$vex"
grype "sbom:$sbom" -o "cyclonedx-json=$vulnerabilities"

if [ "$with_reachability" = true ]; then
  osv="$scan_dir/osv.json"
  enriched="$scan_dir/reachability.cdx.json"
  # 1 means findings, not a broken scanner. Every other nonzero result is
  # refused: an unavailable/failed analyzer must not fabricate a pass.
  osv_status=0
  osv-scanner scan source --call-analysis=all --format=json --output-file="$osv" . || osv_status=$?
  case "$osv_status" in
    0|1 ) ;;
    * ) echo "OSV-Scanner failed with exit $osv_status; no evidence attached" >&2; exit "$osv_status" ;;
  esac
  node "$script_dir/dependency-reachability.mjs" "$sbom" "$vulnerabilities" "$osv" > "$enriched"
  vulnerabilities="$enriched"
fi

"$factory_bin" task attach --kind sbom "$sbom"
"$factory_bin" task attach --kind vulnerabilities "$vulnerabilities"
