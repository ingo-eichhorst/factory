import test from "node:test";
import assert from "node:assert/strict";

import {
  affectedLabel,
  credentialState,
  exploitSignals,
  findingCounts,
  pathLabel,
  serviceTarget,
  shapeFindings,
  visibleDocuments,
  visibleFindings,
} from "../js/dependencies-model.js";

test("findings keep daemon statuses and sort urgent severities first", () => {
  const rows = shapeFindings([
    { id: "CVE-3", status: "resolved", severity: "critical", affected: { name: "c" } },
    { id: "CVE-2", status: "open", severity: "medium", affected: { name: "b" } },
    { id: "CVE-1", status: "open", severity: "critical", affected: { name: "a" } },
    { id: "CVE-4", status: "assessed", severity: "high", affected: { name: "d" } },
  ]);
  assert.deepEqual(rows.map((r) => r.id), ["CVE-1", "CVE-2", "CVE-4", "CVE-3"]);
  assert.deepEqual(findingCounts(rows), { open: 2, assessed: 1, resolved: 1, stale: 0 });
});

test("component paths, exploit signals and service targets are display-only shaping", () => {
  const affected = { name: "serde", version: "1.0.0", path: ["factory@0.1", "serde@1.0.0"] };
  assert.equal(affectedLabel(affected), "serde@1.0.0");
  assert.equal(pathLabel(affected), "factory@0.1 → serde@1.0.0");
  assert.deepEqual(exploitSignals({ kev: true, euvd: false, epss: 0.42 }), ["KEV", "EPSS 0.42"]);
  assert.equal(serviceTarget({ endpoints: ["bank.test:443"] }), "bank.test:443");
  assert.equal(serviceTarget({ path: "/tmp/drop" }), "/tmp/drop");
});

test("unknown credential presence never becomes absent", () => {
  assert.equal(credentialState({ credential: "keychain:bank" }), "unchecked");
  assert.equal(credentialState({ credential: "github", credential_present: false }), "absent");
  assert.equal(credentialState({ credential: "github", credential_present: true }), "present");
  assert.equal(credentialState({}), "none");
});

test("Factory running evidence is shown only by L1 Doctor", () => {
  const rows = [{ state: "declared" }, { state: "built" }, { state: "running" }];
  assert.deepEqual(visibleDocuments(rows, "factory").map((row) => row.state), ["declared", "built"]);
  assert.deepEqual(visibleFindings(rows, "factory").map((row) => row.state), ["declared", "built"]);
  assert.deepEqual(visibleDocuments(rows, "demo"), rows);
  assert.deepEqual(visibleFindings(rows, "demo"), rows);
});
