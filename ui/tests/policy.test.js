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
  assert.match(wiring, /dir:\s*\["policy"\]/);
  assert.match(wiring, /policy:\s*\{\s*onShow:\s*loadPolicy\s*\}/);
});

test("a policy_changed event and the refresh button both go through the same loader", () => {
  assert.match(wiring, /ev\.type === "policy_changed"/);
  assert.match(wiring, /reloadPolicy\(\)/);
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
  };
  globalThis.document = { ...bare, getElementById: (id) => elements[id] || null };

  let requested;
  globalThis.fetch = async (path) => {
    requested = path;
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
                    control: "cra/annex-i-2-4",
                    title: "Security updates are provided without undue delay",
                    kind: "regulation",
                    status: "not_applicable",
                    reasons: ["marked not applicable at demo: covered at the root"],
                  },
                ],
                rollup: [],
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
            catalogues: [{ framework: "cra", title: "Cyber Resilience Act", kind: "regulation", controls: 5 }],
          },
        },
      }),
    };
  };
  state.scope = "demo";

  await loadPolicy();
  assert.equal(requested, "/api/policy?scope=demo");
  assert.match(elements["policy-cards"].innerHTML, /Cyber Resilience Act/);
  assert.match(elements["policy-cards"].innerHTML, /not compliant/);
  assert.match(elements["policy-gaps"].innerHTML, /cra\/annex-i-2-1/);
  assert.match(elements["policy-gaps"].innerHTML, />Knowledge</, "an open knowledge check links to the Knowledge tab");
  assert.match(
    elements["policy-gaps"].innerHTML,
    /data-remediate="cra\/annex-i-2-1"[^>]*data-remediate-scope="demo"/,
    "an open row gets a Create task button, labelled for the same control and scope",
  );
  assert.match(elements["policy-na"].innerHTML, /covered at the root/);
  assert.match(elements["policy-findings"].innerHTML, /names a control nothing defines/);
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
