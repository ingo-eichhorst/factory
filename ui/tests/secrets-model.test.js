import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  expiryClass, expiryText, formValues, metadataBody, presenceText, resolutionText, stateText, useText,
  validDate, visibleUndeclared,
} from "../js/secrets-model.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");

const CLAUDE = {
  name: "claude-oauth-token", kind: "token", from: "file",
  source: "the file ~/.config/factory/secrets/claude-oauth-token",
  present: true, owner_only: true, resolves: true,
  expires: "2027-10-04", days_left: 365, state: "ok",
  renew: "claude setup-token, then (umask 077; cat > ~/.config/factory/secrets/claude-oauth-token)",
  used_by: [{ scope: "awesome-herdr", agent: "awesome-herdr-curator", provider: "factory-claude-1a2b3c4d" }],
};
const GITHUB = {
  name: "github-gh-login", kind: "token", from: "command", source: "the command `gh auth token`",
  resolves: true, expires: "never", state: "never", renew: "gh auth login", used_by: [],
};

test("an entry's expiry reads as the acceptance says", () => {
  assert.equal(expiryText(CLAUDE), "expires 2027-10-04 (365 days)");
  assert.equal(expiryText(GITHUB), "never");
  assert.equal(expiryText({ state: "unknown" }), "unknown");
  assert.equal(expiryText({ expires: "2026-10-05", days_left: 1 }), "expires 2026-10-05 (1 day)");
  assert.equal(expiryText({ expires: "2026-10-04", days_left: 0 }), "expires today (2026-10-04)");
  assert.equal(expiryText({ expires: "2026-10-01", days_left: -3 }), "expired on 2026-10-01 (3 days ago)");
  assert.deepEqual(["ok", "due_soon", "expired", "never", "unknown"].map(expiryClass), ["ok", "warn", "bad", "", ""]);
  assert.equal(stateText("due_soon"), "due soon");
  assert.equal(stateText(undefined), "unknown");
});

test("presence is a file's alone, and resolution never says what came back", () => {
  assert.deepEqual(presenceText(CLAUDE), { text: "present, owner-only", cls: "ok" });
  assert.deepEqual(presenceText({ present: true, owner_only: false }), { text: "present, readable by others", cls: "bad" });
  assert.deepEqual(presenceText({ present: false }), { text: "absent", cls: "bad" });
  assert.deepEqual(presenceText(GITHUB), { text: "n/a", cls: "" });
  assert.deepEqual(resolutionText(GITHUB), { text: "resolves", cls: "ok" });
  assert.deepEqual(resolutionText({}), { text: "not checked yet", cls: "" });
  assert.deepEqual(resolutionText({ resolves: false, reason: "it failed (exit status: 1)" }),
    { text: "does not resolve: it failed (exit status: 1)", cls: "bad" });
});

test("used by names scope, agent and provider", () => {
  assert.equal(useText(CLAUDE.used_by[0]), "awesome-herdr / awesome-herdr-curator / factory-claude-1a2b3c4d");
  const rows = [{ scope: "a" }, { scope: "b" }];
  assert.deepEqual(visibleUndeclared(rows, s => s === "b"), [{ scope: "b" }]);
  assert.deepEqual(visibleUndeclared(undefined, () => true), []);
});

test("the edit form sends the three metadata fields and nothing else", () => {
  assert.deepEqual(formValues(CLAUDE), { never: false, date: "2027-10-04", renew: CLAUDE.renew, note: "" });
  assert.deepEqual(formValues(GITHUB).never, true);
  assert.deepEqual(metadataBody({ never: false, date: "2027-11-01", renew: " gh auth login ", note: "" }),
    { expires: "2027-11-01", renew: "gh auth login", note: null });
  assert.deepEqual(metadataBody({ never: true, date: "2027-11-01", renew: "", note: "x" }),
    { expires: "never", renew: null, note: "x" });
  assert.deepEqual(Object.keys(metadataBody({})).sort(), ["expires", "note", "renew"]);
  assert.equal(metadataBody({}).expires, null, "an emptied date clears it: unknown");
  assert.ok(validDate("2027-10-04") && validDate(""));
  assert.ok(!validDate("2027-02-30") && !validDate("next year"));
});

test("the tab has a place for the catalogue, the undeclared and the changes", () => {
  for (const id of ["secrets-catalogue", "noCatalogue", "secrets-undeclared", "secrets-undeclared-section", "secrets-changes"]) {
    assert.ok(page.includes(`id="${id}"`), id);
  }
});
