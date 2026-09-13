import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");

test("agent screens are peer header tabs without an Agents wrapper", () => {
  assert.match(page, /id="tab-occupancy"[^>]*>Occupancy<\/button>/);
  assert.match(page, /id="tab-roster"[^>]*>Roster<\/button>/);
  assert.match(page, /id="tab-agent-runtime"[^>]*>Agent-runtime<\/button>/);
  assert.match(page, /id="tab-roles"[^>]*>Roles<\/button>/);
  assert.doesNotMatch(page, /id="tab-agents"/);
  assert.doesNotMatch(page, /id="agent-view"/);
  assert.doesNotMatch(page, /id="view-agents"/);
});

test("L2 Environment is live, with a Sandboxes tab and a Secrets tab", () => {
  assert.doesNotMatch(page, /id="lv-env"[^>]*disabled/);
  assert.match(page, /id="tab-sandboxes"[^>]*>Sandboxes<\/button>/);
  assert.match(page, /id="tab-secrets"[^>]*>Secrets<\/button>/);
  assert.match(page, /id="view-sandboxes"/);
  assert.match(page, /id="view-secrets"/);
});
