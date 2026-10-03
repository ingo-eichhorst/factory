import test from "node:test";
import assert from "node:assert/strict";

globalThis.document = { documentElement: { dataset: {} }, getElementById: () => null };
const { findingCard } = await import("../js/dependency-finding.js");

function finding(extra = {}) {
  return {
    id: "CVE-REACH", status: "open", state: "built", severity: "high",
    affected: { name: "serde", bom_ref: "pkg:cargo/serde@1" },
    scan: { attachment: { run_id: "scan-run", attempt: 1, attached_at: "2026-10-01T00:00:00Z" } },
    ...extra,
  };
}

test("the shared finding card exposes escaped reachability and analysis detail without assessing it", () => {
  const row = finding({ reachability: "unreachable <script>", analysis_detail: "entrypoint → parser <img src=x onerror=alert(1)>" });
  const html = findingCard(row);
  assert.match(html, /Reported reachability/);
  assert.match(html, /unreachable &lt;script&gt;/);
  assert.match(html, /Analysis detail/);
  assert.match(html, /parser &lt;img src=x onerror=alert\(1\)&gt;/);
  assert.doesNotMatch(html, /<script>|<img/);
  assert.match(html, /<dt>VEX<\/dt><dd>none<\/dd>/);
  assert.equal(row.status, "open");
});

test("absent reachability stays unknown even when VEX or prose suggests unreachable code", () => {
  const html = findingCard(finding({ vex_state: "not_affected", vex_justification: "code_not_reachable", analysis_detail: "probably unreachable" }));
  assert.match(html, /Reported reachability<\/dt><dd>unknown/);
  assert.match(html, /probably unreachable/);
  assert.match(html, /not_affected · code_not_reachable/);
});

test("historical analysis keeps the document that supplied it, not the newest absence scan", () => {
  const html = findingCard(finding({ status: "resolved", reachability: "unreachable", analysis_scan: {
    tool: "osv-scanner", attachment: { id: "original-document", run_id: "old-run", attempt: 1, attached_at: "2026-09-01T00:00:00Z" },
  } }));
  assert.match(html, /Analysis evidence scan/);
  assert.match(html, /original-document/);
  assert.match(html, /old-run/);
  assert.match(html, /2026-09-01/);
});
