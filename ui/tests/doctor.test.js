import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/doctor.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
const scan = readFileSync(new URL("../../examples/factory-dependency-scan.sh", import.meta.url), "utf8");
const workflow = readFileSync(new URL("../../workflows/factory-dependency-scan.yaml", import.meta.url), "utf8");

const bareDocument = {
  documentElement: { dataset: {} },
  getElementById: () => null,
};
globalThis.document = bareDocument;
const { state } = await import("../js/core.js");
const { renderDoctor } = await import("../js/doctor.js");

test("Doctor is the L1 tab between Infrastructure and Backup", () => {
  assert.match(page, /id="tab-infrastructure"[^>]*>Infrastructure<\/button>\s*<button id="tab-doctor" hidden>Doctor<\/button>\s*<button id="tab-backup" hidden>Backup<\/button>/);
  assert.match(page, /id="view-doctor"/);
  assert.match(app, /infra: \["infrastructure", "doctor", "backup"\]/);
  assert.match(app, /doctor: \{ onShow: startDoctor, onHide: stopAgentPoll \}/);
});

test("Doctor has calm 404 and separate error elements", () => {
  assert.match(page, /id="doctor-unavailable"[^>]*hidden/);
  assert.match(page, /id="doctor-error"[^>]*hidden/);
  assert.match(view, /api\("\/api\/doctor"\)/);
});

test("Doctor and its shared finding renderer are compiled into the daemon", () => {
  assert.match(served, /"js\/doctor\.js"/);
  assert.match(served, /"js\/doctor-model\.js"/);
  assert.match(served, /"js\/dependency-finding\.js"/);
});

test("Doctor keeps every detailed field of a running finding visible", () => {
  const elements = {
    "doctor-unavailable": { hidden: false },
    "doctor-error": { hidden: false, textContent: "" },
    doctor: { hidden: true, innerHTML: "" },
  };
  globalThis.document = {
    ...bareDocument,
    getElementById: (id) => elements[id] || null,
  };
  state.doctorUnavailable = false;
  state.doctorError = null;
  state.doctor = {
    now: "2026-09-26T12:00:00Z",
    status: "current",
    built: null,
    running: null,
    findings: [{
      id: "CVE-DETAIL-123",
      state: "running",
      status: "assessed",
      severity: "critical",
      ratings: [{
        severity: "critical",
        source: "NVD",
        method: "CVSSv31",
        score: 9.8,
        vector: "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H",
      }],
      affected: {
        bom_ref: "pkg:cargo/openssl@3.0.0",
        name: "openssl",
        version: "3.0.0",
        path: ["factory-daemon@0.1.0", "openssl@3.0.0"],
      },
      scan: {
        tool: "grype",
        tool_version: "0.80.0",
        scan_time: "2026-09-26T11:00:00Z",
        attachment: {
          run_id: "run-installed-123",
          attempt: 3,
          attached_at: "2026-09-26T11:00:00Z",
        },
      },
      fixed_version: "3.0.1",
      vex_state: "not_affected",
      vex_justification: "code_not_reachable",
      vex_response: ["will_not_fix"],
      kev: true,
      euvd: true,
      epss: 0.91,
    }],
  };

  renderDoctor();

  const html = elements.doctor.innerHTML;
  for (const detail of [
    "CVE-DETAIL-123",
    "NVD",
    "CVSSv31",
    "9.8",
    "CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H",
    "pkg:cargo/openssl@3.0.0",
    "factory-daemon@0.1.0 → openssl@3.0.0",
    "3.0.1",
    "not_affected · code_not_reachable · will_not_fix",
    "KEV",
    "EUVD",
    "EPSS 0.91",
  ]) {
    assert.match(html, new RegExp(detail.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), detail);
  }

  state.doctor = null;
  globalThis.document = bareDocument;
});

test("the checked-in workflow builds auditable releases and scans the two installed binaries", () => {
  assert.match(workflow, /id: built[\s\S]*factory-dependency-scan\.sh built/);
  assert.match(workflow, /id: running[\s\S]*factory-dependency-scan\.sh running/);
  assert.match(scan, /cargo auditable build --workspace --release/);
  assert.match(scan, /\.local\/bin/);
  assert.match(scan, /cli="\$install_dir\/factory"/);
  assert.match(scan, /daemon="\$install_dir\/factory-daemon"/);
  assert.match(scan, /factory:git-sha/);
  assert.match(scan, /task attach --kind sbom/);
  assert.match(scan, /task attach --kind vulnerabilities/);
});
