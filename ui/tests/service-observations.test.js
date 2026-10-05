import test from "node:test";
import assert from "node:assert/strict";
import { evidenceCards, matchingAccesses, observedAccesses, observedCell } from "../js/service-observations-model.js";

const access = (changes = {}) => ({ at: "2026-10-04T12:00:00Z", transport: "network", target: "api.github.com:443", disposition: "allowed", process: "/usr/bin/curl(42)", policy: "github", ...changes });
const capture = (accesses, changes = {}) => ({ id: "evidence", run_id: "run", task_id: "task", agent: "curator", sandbox: "factory-run", source: "OpenShell supervisor/proxy OCSF", captured_at: "2026-10-04T12:01:00Z", partial: true, accesses, ...changes });
const evidence = (...captures) => ({ scope: "demo", captures, findings: [] });

test("absent, empty and failed collection all remain unknown, never unused or compliant", () => {
  for (const data of [undefined, evidence(), evidence(capture([], { issue: "log unavailable" }))]) {
    assert.match(observedCell({ transport: "network", endpoints: ["api.github.com:443"] }, data), /unknown/);
    const html = evidenceCards(data);
    assert.match(html, /partial/);
    assert.match(html, /no record does not mean no access/);
    assert.doesNotMatch(html, /service unused|compliant|all access captured/);
  }
});

test("endpoint joining retains ports and agent selection, not URL path or credential payload", () => {
  const data = evidence(capture([access(), access({ target: "api.github.com:8443" })]));
  assert.equal(matchingAccesses({ transport: "network", endpoints: ["https://API.GITHUB.COM/v1"] }, data).length, 1);
  assert.equal(matchingAccesses({ transport: "network", endpoints: ["api.github.com:8443"] }, data).length, 1);
  assert.equal(matchingAccesses({ transport: "network", endpoints: ["api.github.com:443"], agents: ["other"] }, data).length, 0);
  assert.equal(matchingAccesses({ transport: "network", endpoints: ["https://user:token@api.github.com"] }, data).length, 0);
  assert.equal(matchingAccesses({ transport: "file", path: "api.github.com:443" }, data).length, 0);
  assert.match(observedCell({ transport: "socket", path: "/tmp/socket" }, data), /unknown/);
});

test("overlapping snapshots deduplicate only the same run and event, keeping denials separate", () => {
  const data = evidence(capture([access()]), capture([access(), access({ disposition: "denied" })]), capture([access()], { run_id: "other" }));
  assert.equal(observedAccesses(data).length, 3);
  const html = observedCell({ transport: "network", endpoints: ["api.github.com:443"] }, data);
  assert.match(html, /2 allowed/);
  assert.match(html, /1 denied/);
  assert.match(html, /not a verdict/);
});

test("every evidence field is escaped, including source, provenance and collection failures", () => {
  const attack = '<img src=x onerror="alert(1)">';
  const data = evidence(capture([access({ target: attack, process: attack, policy: attack, reason: attack })], { id: attack, agent: attack, task_id: attack, run_id: attack, sandbox: attack, source: attack, issue: attack }));
  data.findings = [attack];
  const html = evidenceCards(data);
  assert.ok(!html.includes(attack));
  assert.match(html, /&lt;img/);
  assert.match(html, /&quot;/);
});

test("a stale-policy denial is labeled a transient policy switch, apart from an ordinary denial, and never dropped", () => {
  const stale = access({ target: "api.anthropic.com:443", disposition: "denied", reason: "policy generation is stale" });
  const ordinary = access({ target: "downloads.claude.ai:443", disposition: "denied" });
  const data = evidence(capture([stale, ordinary]));

  // Never dropped: both rows are still present.
  assert.equal(observedAccesses(data).length, 2);

  // The per-service cell counts the transient refusal apart from a real one.
  const cell = observedCell({ transport: "network", endpoints: ["api.anthropic.com:443", "downloads.claude.ai:443"] }, data);
  assert.match(cell, /1 denied \(policy switch\)/);
  assert.match(cell, /1 denied(?! \(policy switch\))/);

  // The evidence card labels only the stale-policy row as transient.
  const html = evidenceCards(data);
  const [staleCard, ordinaryCard] = html.split("</article>");
  assert.match(staleCard, /policy switch, transient/);
  assert.match(staleCard, /reason policy generation is stale/);
  assert.doesNotMatch(ordinaryCard, /policy switch/);
});

test("a different denial reason stays an ordinary denial, not a policy switch", () => {
  const data = evidence(capture([access({ disposition: "denied", reason: "no matching policy" })]));
  const cell = observedCell({ transport: "network", endpoints: ["api.github.com:443"] }, data);
  assert.match(cell, /1 denied/);
  assert.doesNotMatch(cell, /policy switch/);
  const html = evidenceCards(data);
  assert.doesNotMatch(html, /policy switch/);
  assert.match(html, /reason no matching policy/);
});
