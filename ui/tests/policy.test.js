import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { readHash, setRouter } from "../js/scopes.js";

// `modal.js`, which the view opens its forms and detail in, listens for
// Escape on the document as it loads -- so there has to be one before the
// view is imported, the same requirement `roles.test.js` documents.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const { loadPolicy, renderPolicy } = await import("../js/policy.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

test("L6 Direction is live: the level button carries no disabled/title, and the tab/view pair exists", () => {
  assert.doesNotMatch(page, /id="lv-dir"[^>]*disabled/);
  assert.doesNotMatch(page, /id="lv-dir"[^>]*title="Not built yet"/);
  assert.match(page, /id="tab-policy"[^>]*>Policy<\/button>/);
  assert.match(page, /id="view-policy"/);
  assert.match(page, /evidence is complete[\s\S]*never that anything is certified/);
  // Not anchored at the closing bracket: `dir` grows a trailing "scenarios"
  // (#100) after this pair.
  assert.match(wiring, /dir:\s*\["goals",\s*"policy"/);
  assert.match(wiring, /policy:\s*\{\s*onShow:\s*loadPolicy\s*\}/);
});

test("a policy_changed event and the refresh button both go through the same loader", () => {
  assert.match(wiring, /ev\.type === "policy_changed"/);
  assert.match(wiring, /reloadPolicy\(\)/);
});

test("a task carrying a policy= label appearing or changing status reloads the tab (#98)", () => {
  assert.match(wiring, /const policyTask = ev\.task && ev\.task\.labels && ev\.task\.labels\.policy;/);
  assert.match(
    wiring,
    /state\.tab === "policy" && \(\(ev\.type === "task_created" && policyTask\)[\s\S]*?ev\.type === "task_updated" && policyTask && \(!priorTask \|\| priorTask\.status !== ev\.task\.status\)[\s\S]*?ev\.type === "task_deleted"\)\)\s*\{\s*reloadPolicy\(\);/,
  );
});

test("workflow edits refresh the Policy enforcement preview", () => {
  assert.match(
    wiring,
    /state\.tab === "policy" && \["workflow_created", "workflow_updated", "workflow_deleted"\]\.includes\(ev\.type\)/,
  );
});

test("a link with a level segment and an old bare link both land on the Policy page", () => {
  setRouter({ pages: ["dashboard", "policy", "roles"], redirects: {} });

  globalThis.location = { hash: "#all/dir/policy" };
  assert.equal(readHash().page, "policy");
  delete globalThis.location;

  globalThis.location = { hash: "#all/policy" };
  assert.equal(readHash().page, "policy");
  delete globalThis.location;
});

test("loading asks for the selected scope and renders a card, a gap link, an n/a row and a finding", async () => {
  const elements = {
    "policy-scope-note": { textContent: "" },
    "policy-error": { textContent: "", hidden: true },
    "policy-export": { href: "" },
    "policy-cards": { innerHTML: "" },
    "policy-count": { textContent: "" },
    "policy-empty": { hidden: false },
    "policy-gaps": { innerHTML: "", querySelectorAll: () => [] },
    "policy-no-gaps": { hidden: false },
    "policy-na": { innerHTML: "" },
    "policy-no-na": { hidden: false },
    "policy-findings": { innerHTML: "" },
    "policy-no-findings": { hidden: false },
    "policy-workflow-enforcement": { innerHTML: "" },
    "policy-no-workflow-enforcement": { hidden: false },
  };
  globalThis.document = { ...bare, getElementById: (id) => elements[id] || null };

  const requested = [];
  globalThis.fetch = async (path) => {
    requested.push(path);
    return {
      status: 200,
      statusText: "OK",
      json: async () => ({
        status: "ok",
        data: {
          kind: "policy",
          report: {
            scope: "demo",
            rows: [
              {
                scope: "demo",
                statuses: [
                  {
                    control: "cra/annex-i-2-1",
                    title: "Identify and document components (SBOM)",
                    kind: "regulation",
                    status: "open",
                    reasons: ["knowledge: tag `control/cra/annex-i-2-1` not found"],
                  },
                  {
                    control: "cra/annex-i-2-3",
                    title: "Known exploitable vulnerabilities are tracked and remediated",
                    kind: "regulation",
                    status: "open",
                    reasons: ["attestation: none recorded"],
                  },
                  {
                    control: "cra/annex-i-2-4",
                    title: "Security updates are provided without undue delay",
                    kind: "regulation",
                    status: "not_applicable",
                    reasons: ["marked not applicable at demo: covered at the root"],
                  },
                ],
                rollup: [],
                open_tasks: { "cra/annex-i-2-3": "task-open-3" },
              },
            ],
            rollup: [
              {
                framework: "cra",
                counts: { satisfied: 0, attested: 0, stale: 0, open: 1, not_applicable: 1 },
                best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 },
                compliant: false,
              },
            ],
            not_applicable: [{ control: "cra/annex-i-2-4", scope: "demo", rationale: "covered at the root" }],
            findings: [{ kind: "unknown_control", subject: "demo", detail: "names a control nothing defines" }],
            workflow_enforcement: [{
              workflow: "wf-1", name: "release", scope: "demo", node: "publish",
              step: "review", kind: "review", required_by: ["cra/security-testing"],
            }],
            workflow_findings: [{
              workflow: "wf-1", name: "release", scope: "demo",
              detail: "review publish.review has no independent functionary",
            }],
            catalogues: [{ framework: "cra", title: "Cyber Resilience Act", kind: "regulation", controls: 5 }],
          },
        },
      }),
    };
  };
  state.scope = "demo";

  await loadPolicy();
  // The reporting clock (`#157`/`#170` phase 2) is read alongside the
  // report now, under the same scope query. ("/clock" sorts before "?" in
  // plain string order.)
  assert.deepEqual([...requested].sort(), ["/api/policy/clock?scope=demo", "/api/policy?scope=demo"]);
  assert.match(elements["policy-cards"].innerHTML, /Cyber Resilience Act/);
  assert.match(elements["policy-cards"].innerHTML, /not compliant/);
  assert.match(elements["policy-gaps"].innerHTML, /cra\/annex-i-2-1/);
  assert.match(elements["policy-gaps"].innerHTML, />Knowledge</, "an open knowledge check links to the Knowledge tab");
  assert.match(
    elements["policy-gaps"].innerHTML,
    /data-remediate="cra\/annex-i-2-1"[^>]*data-remediate-scope="demo"/,
    "an open row gets a Create task button, labelled for the same control and scope",
  );
  assert.match(
    elements["policy-gaps"].innerHTML,
    /<a class="pol-task-created" href="#demo\/tasks\/task\/task-open-3">Task open →<\/a>/,
    "#98: a row whose remediation task is already open links to it",
  );
  assert.doesNotMatch(
    elements["policy-gaps"].innerHTML,
    /data-remediate="cra\/annex-i-2-3"/,
    "... and offers no Create task button beside it",
  );
  assert.match(elements["policy-na"].innerHTML, /covered at the root/);
  assert.match(elements["policy-findings"].innerHTML, /names a control nothing defines/);
  assert.match(elements["policy-workflow-enforcement"].innerHTML, /review <b>review<\/b> on publish/);
  assert.match(elements["policy-workflow-enforcement"].innerHTML, /no independent functionary/);
  assert.match(elements["policy-workflow-enforcement"].innerHTML, /required by cra\/security-testing/);
  assert.equal(elements["policy-no-gaps"].hidden, true, "one open row on screen -> the 'nothing open' empty state is hidden");
  assert.equal(elements["policy-export"].href, "/api/policy/export?scope=demo", "the Export link tracks the selected scope");

  globalThis.fetch = async () => ({
    status: 400,
    statusText: "Bad Request",
    json: async () => ({ status: "error", message: "no such scope: gone" }),
  });
  await loadPolicy();
  assert.equal(elements["policy-error"].textContent, "no such scope: gone");
  assert.equal(elements["policy-error"].hidden, false);
  assert.equal(elements["policy-cards"].innerHTML, "", "a failed fetch clears the last answer rather than leaving it stale");

  delete globalThis.fetch;
  globalThis.document = bare;
  state.scope = null;
  renderPolicy(); // no throw with document stubbed back to the bare shim
});

test("the reporting clock section renders a deadline row and an excluded row, independent of the framework board (#157/#170 phase 2)", async () => {
  const elements = {
    "policy-scope-note": { textContent: "" },
    "policy-error": { textContent: "", hidden: true },
    "policy-export": { href: "" },
    "policy-cards": { innerHTML: "" },
    "policy-count": { textContent: "" },
    "policy-empty": { hidden: false },
    "policy-gaps": { innerHTML: "", querySelectorAll: () => [] },
    "policy-no-gaps": { hidden: false },
    "policy-na": { innerHTML: "" },
    "policy-no-na": { hidden: false },
    "policy-findings": { innerHTML: "" },
    "policy-no-findings": { hidden: false },
    "policy-clock": { innerHTML: "" },
    "policy-no-clock": { hidden: false },
  };
  globalThis.document = { ...bare, getElementById: (id) => elements[id] || null };

  const clock = {
    now: "2026-09-24T12:00:00Z",
    items: [
      {
        item: { kind: "report", item: "task-1" },
        scope: "demo",
        awareness_at: "2026-09-20T12:00:00Z",
        reported_now: true,
        deadlines: [{ deadline: "notification", due_at: "2026-09-23T12:00:00Z", state: "overdue", submission: null }],
      },
      {
        item: { kind: "finding", scope: "demo", vulnerability: "CVE-2026-1234" },
        scope: "demo",
        awareness_at: "2026-09-23T12:00:00Z",
        excluded: "not_affected",
        reported_now: false,
        deadlines: [],
      },
    ],
  };
  globalThis.fetch = async (path) => ({
    status: 200,
    statusText: "OK",
    json: async () => ({
      status: "ok",
      data: path.startsWith("/api/policy/clock")
        ? { kind: "policy_clock", clock }
        : { kind: "policy", report: { scope: "demo", rows: [], rollup: [], not_applicable: [], findings: [], catalogues: [] } },
    }),
  });
  state.scope = "demo";
  await loadPolicy();

  const html = elements["policy-clock"].innerHTML;
  assert.match(html, /report task-1/, "a report's own text is its short id, since it carries no title of its own");
  assert.match(html, /72h notification/);
  assert.match(html, /class="badge s-overdue">overdue</);
  assert.match(html, /overdue by/);
  assert.match(html, /CVE-2026-1234/);
  assert.match(html, /excluded: not_affected/);
  assert.equal(elements["policy-no-clock"].hidden, true, "two rows on screen -> the empty state is hidden");

  globalThis.fetch = async () => { throw new Error("offline"); };
  await loadPolicy();
  assert.equal(elements["policy-clock"].innerHTML, "", "a failed clock read clears the last answer, same as the board itself");
  assert.equal(elements["policy-no-clock"].hidden, false);

  delete globalThis.fetch;
  globalThis.document = bare;
  state.scope = null;
});
