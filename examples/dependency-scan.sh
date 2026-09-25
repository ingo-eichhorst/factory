#!/bin/sh
# Declared-state example for one ordinary shell task in a dependency-scan
# workflow. Syft and Grype are examples chosen by this scope, not Factory
# dependencies and not a scanner adapter.
set -eu

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

syft dir:. -o "cyclonedx-json=$raw_sbom"
jq '.metadata.lifecycles = [{"phase":"pre-build"}] | del(.vulnerabilities)' \
  "$raw_sbom" > "$sbom"

# Keep the authored judgments available to a scanner or conversion step. Grype
# does not consume CycloneDX VEX directly, so this minimal example only saves
# the standard document; a real scope workflow can convert it before scanning.
"$factory_bin" dependencies vex "${FACTORY_SCOPE:?FACTORY_SCOPE is not set}" > "$vex"
grype "sbom:$sbom" -o "cyclonedx-json=$vulnerabilities"

"$factory_bin" task attach --kind sbom "$sbom"
"$factory_bin" task attach --kind vulnerabilities "$vulnerabilities"
