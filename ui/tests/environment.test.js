import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { sandboxTag, visibleSandboxes } from "../js/sandboxes.js";
import { visibleCredentials } from "../js/secrets.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");

const CREDENTIALS = [
  { label: "Claude Code credentials", path: "/home/me/.claude/.credentials.json", integration: "anthropic", present: true },
  { label: "SSH private keys", path: "/home/me/.ssh/id_*", integration: "ssh", present: true },
  { label: "alpha .env", path: "/root/projects/alpha/.env", integration: "scope env", present: false, scope: "alpha" },
  { label: "beta .env", path: "/root/projects/beta/.env", integration: "scope env", present: true, scope: "beta" },
];

test("a scope selection narrows the scope-bound rows and keeps the ambient ones", () => {
  const onlyAlpha = name => name === "alpha";
  const rows = visibleCredentials(CREDENTIALS, onlyAlpha);

  // The home rows have no scope to be filtered by, and they are the whole
  // point of the tab: every agent reaches them from every scope.
  assert.deepEqual(rows.map(r => r.label), [
    "Claude Code credentials",
    "SSH private keys",
    "alpha .env",
  ]);
});

test("no selection shows the whole inventory", () => {
  assert.equal(visibleCredentials(CREDENTIALS, () => true).length, 4);
  assert.deepEqual(visibleCredentials(undefined, () => true), []);
});

test("the sandbox rows narrow to the selection the same way", () => {
  const rows = [{ scope: "alpha" }, { scope: "beta" }, { scope: "alpha" }];
  assert.equal(visibleSandboxes(rows, name => name === "alpha").length, 2);
});

test("openshell is tagged enforced, docker and srt declared only, none untagged", () => {
  assert.match(sandboxTag({ sandbox: "openshell", enforced: true }), /enforced/);
  assert.doesNotMatch(sandboxTag({ sandbox: "openshell", enforced: true }), /not enforced/);
  assert.match(sandboxTag({ sandbox: "docker", enforced: false }), /declared, not enforced/);
  // An older daemon's row carries no `enforced` at all: nothing was.
  assert.match(sandboxTag({ sandbox: "srt" }), /declared, not enforced/);
  assert.equal(sandboxTag({ sandbox: "none", enforced: false }), "");
});

test("both notes sit above the table they speak about", () => {
  const view = page.slice(page.indexOf('id="view-secrets"'));
  const body = view.indexOf('id="secrets"');
  for (const id of ["secrets-correction", "secrets-reach", "secrets-boundary"]) {
    assert.ok(
      view.indexOf(id) < body,
      `${id} must render above the table -- its text says "below"`,
    );
  }
});

test("a failed fetch gets its own element and never takes the reachability note's place", () => {
  assert.match(page, /id="secrets-error"[^>]*hidden/);
  assert.match(page, /id="sandboxes-error"[^>]*hidden/);

  state.environment = null;
  state.environmentError = "Failed to fetch";
  // With no answer there is nothing to filter. The reachability sentence is a
  // constant in the view, not a field of the payload, so it is unaffected.
  assert.deepEqual(visibleCredentials(state.environment && state.environment.credentials), []);
  state.environmentError = null;
});

test("an openshell row says whether its sandbox could be made now, and what it needs", async () => {
  const { readinessTag, readinessDetail } = await import("../js/sandboxes.js");
  assert.equal(readinessTag({ sandbox: "docker" }), "", "only openshell has prerequisites the daemon keeps");
  assert.match(readinessTag({ sandbox: "openshell" }), />checking</);
  const needs = {
    state: "needs",
    thing: "the credential for factory-claude from the file ~/.config/factory/secrets/claude-oauth-token (it cannot be read)",
    command: "claude setup-token, then (umask 077; cat > ~/.config/factory/secrets/claude-oauth-token)",
  };
  const tag = readinessTag({ sandbox: "openshell", readiness: needs });
  assert.match(tag, /data-tone="bad"/);
  assert.match(tag, />needs</);
  const detail = readinessDetail(needs);
  assert.match(detail, /needs the credential for factory-claude/);
  assert.match(detail, /<code>claude setup-token, then \(umask 077; cat &gt; ~\/.config/);
  assert.match(readinessTag({ sandbox: "openshell", readiness: { state: "ready" } }), /data-tone="ok"/);
  const ready = readinessDetail({ state: "ready", notes: ["a new image is being built"], expiring: [{ provider: "factory-claude", expires: "2027-01-01", days_left: 9 }] });
  assert.match(ready, /a new image is being built/);
  assert.match(ready, /factory-claude's credential expires on 2027-01-01 \(9 days\)/);
  assert.equal(readinessDetail(undefined), "");
});
