import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const script = fileURLToPath(new URL("../../examples/dependency-scan.sh", import.meta.url));
const ref = "pkg:cargo/demo-lib@1.2.3";

for (const status of [0, 1, 127, 128]) {
  test(`reachability shell workflow handles OSV exit ${status} without false evidence`, (t) => {
    const root = mkdtempSync(join(tmpdir(), "factory-reachability-shell-"));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const bin = join(root, "bin"); mkdirSync(bin);
    writeFileSync(join(root, "sbom.json"), JSON.stringify({ bomFormat: "CycloneDX", specVersion: "1.6", components: [{ "bom-ref": ref, purl: ref }] }));
    writeFileSync(join(root, "vulns.json"), JSON.stringify({ bomFormat: "CycloneDX", specVersion: "1.6", vulnerabilities: [{ id: "CVE-TEST", affects: [{ ref }] }] }));
    writeFileSync(join(root, "osv.json"), JSON.stringify({ results: [{ packages: [{
      package: { name: "demo-lib", version: "1.2.3", ecosystem: "crates.io" },
      groups: [{ ids: ["CVE-TEST"], experimental_analysis: { "CVE-TEST": { called: false } } }],
    }] }] }));
    const mocks = {
      syft: '#!/bin/sh\ncp "$QA_ROOT/sbom.json" "${3#cyclonedx-json=}"\n',
      grype: '#!/bin/sh\ncp "$QA_ROOT/vulns.json" "${3#cyclonedx-json=}"\n',
      "osv-scanner": '#!/bin/sh\nprintf "%s\\n" "$@" > "$QA_ROOT/osv-argv"\ncp "$QA_ROOT/osv.json" "${5#--output-file=}"\nexit "$QA_STATUS"\n',
      factory: '#!/bin/sh\nif [ "$1" = dependencies ]; then printf "{}\\n"; else cp "$5" "$QA_ROOT/attached-$4.json"; fi\n',
    };
    for (const [name, body] of Object.entries(mocks)) writeFileSync(join(bin, name), body, { mode: 0o755 });
    const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, FACTORY_BIN: join(bin, "factory"), FACTORY_SCOPE: "demo", QA_ROOT: root, QA_STATUS: String(status) };
    for (const key of ["FACTORY_TOKEN", "FACTORY_TASK_TOKEN", "FACTORY_ROOT", "FACTORY_SOCKET"]) delete env[key];
    const result = spawnSync("sh", [script, "--reachability"], { cwd: root, env, encoding: "utf8" });
    assert.equal(result.status, status <= 1 ? 0 : status, result.stderr);
    assert.deepEqual(readFileSync(join(root, "osv-argv"), "utf8").trim().split("\n").slice(0, 4), ["scan", "source", "--call-analysis=all", "--format=json"]);
    if (status <= 1) {
      const sbom = JSON.parse(readFileSync(join(root, "attached-sbom.json"), "utf8"));
      assert.deepEqual(sbom.metadata.lifecycles, [{ phase: "pre-build" }]);
      const vuln = JSON.parse(readFileSync(join(root, "attached-vulnerabilities.json"), "utf8")).vulnerabilities[0];
      assert.equal(JSON.parse(vuln.properties.find((p) => p.name === "factory:reachability").value)[ref], "unreachable");
      assert.equal(vuln.analysis, undefined);
    } else {
      assert.equal(existsSync(join(root, "attached-sbom.json")), false);
      assert.equal(existsSync(join(root, "attached-vulnerabilities.json")), false);
      assert.match(result.stderr, /no evidence attached/);
    }
  });
}
