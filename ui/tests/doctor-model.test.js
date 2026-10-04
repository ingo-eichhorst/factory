import test from "node:test";
import assert from "node:assert/strict";

import {
  doctorFailure,
  findingSummary,
  gatewaySummary,
  identityText,
  scanAge,
  statusSummary,
} from "../js/doctor-model.js";

const document = {
  state: "running",
  sbom: {
    identity: { version: "0.1.0", git_sha: "0123456789abcdef" },
    scan_time: "2026-09-25T12:00:00Z",
    attachment: { attached_at: "2026-09-25T12:01:00Z" },
  },
};

test("the daemon's three statuses map to presentation without being re-derived", () => {
  assert.equal(statusSummary("current").level, "ok");
  assert.equal(statusSummary("behind").level, "warn");
  assert.equal(statusSummary("missing").level, "bad");
});

test("identity and scan age expose version, commit and server-clock age", () => {
  assert.equal(identityText(document), "v0.1.0 · 0123456789abcdef");
  assert.equal(identityText(null), "identity not recorded");
  assert.equal(scanAge("2026-09-25T14:00:00Z", document), "2h ago");
  assert.equal(scanAge("bad", document), "scan time unknown");
});

test("running finding counts stay display-only", () => {
  assert.deepEqual(findingSummary([{ status: "open" }, { status: "assessed" }, { status: "resolved" }]), {
    open: 1,
    assessed: 1,
    total: 3,
  });
});

test("an absent endpoint is calm while other failures are errors", () => {
  assert.equal(doctorFailure(new Error("404 Not Found")), "unavailable");
  assert.equal(doctorFailure(new Error("500 Internal Server Error")), "error");
});

test("an OpenShell gateway says whether it answers and when Factory last started it", () => {
  const up = gatewaySummary({ gateway: "openshell", status: "connected", server: "https://localhost:17670", version: "0.1.2", started_at: "2026-10-04T07:00:00Z", started_with: "launchctl kickstart gui/502/sh.brew.openshell" });
  assert.equal(up.level, "ok");
  assert.equal(up.facts, "https://localhost:17670 · v0.1.2");
  assert.match(up.started, /started by Factory at 2026-10-04T07:00:00Z with launchctl kickstart/);
  const down = gatewaySummary({ gateway: "openshell", status: "unreachable", detail: "connection refused" });
  assert.deepEqual([down.level, down.label, down.started, down.detail], ["bad", "unreachable", null, "connection refused"]);
});
