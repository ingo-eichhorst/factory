import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/doctor.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");
const scan = readFileSync(new URL("../../examples/factory-dependency-scan.sh", import.meta.url), "utf8");
const workflow = readFileSync(new URL("../../workflows/factory-dependency-scan.yaml", import.meta.url), "utf8");

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

test("both Doctor modules are compiled into the daemon", () => {
  assert.match(served, /"js\/doctor\.js"/);
  assert.match(served, /"js\/doctor-model\.js"/);
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
