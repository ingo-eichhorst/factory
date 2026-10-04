import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  MODES,
  changeRows,
  headline,
  installStep,
  macFailure,
  macState,
  modeLabel,
  needsRule,
  reading,
  segments,
  setBody,
  sourceRows,
  stateText,
} from "../js/mac-model.js";

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const app = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");
const view = readFileSync(new URL("../js/mac.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");

const SUDOERS = {
  path: "/etc/sudoers.d/factory-pmset",
  user: "factory",
  rule: "factory ALL=(root) NOPASSWD: /usr/bin/pmset -a powermode 0, /usr/bin/pmset -a powermode 1, /usr/bin/pmset -a powermode 2",
  install: "f=\"$(mktemp)\" && printf '%s\\n' '...' > \"$f\" && sudo visudo -cf \"$f\" && sudo install -m 0440 -o root -g wheel \"$f\" /etc/sudoers.d/factory-pmset; rm -f \"$f\"",
  check: "sudo visudo -c",
};

// The wire shape of `GET /api/host/power-mode`'s report on the issue's host
// today: both modes offered, Automatic on AC and battery, no sudoers rule.
const TODAY = {
  applicable: true,
  supported: ["automatic", "high_performance", "energy_saving"],
  ac: "automatic",
  battery: "automatic",
  permitted: [],
  can_change: false,
  sudoers: SUDOERS,
  notes: [],
  changes: [],
};

const WITH_RULE = { ...TODAY, permitted: [...TODAY.supported], can_change: true };

test("the three modes map to pmset's own numbers, in the control's order", () => {
  assert.deepEqual(MODES.map(m => [m.mode, m.pmset]), [["automatic", 0], ["high_performance", 2], ["energy_saving", 1]]);
  assert.equal(modeLabel("high_performance"), "High performance");
  assert.equal(modeLabel("turbo"), "--");
});

test("without the sudoers rule the page is read-only: the mode is shown, the control disabled, the rule shown", () => {
  assert.equal(macState(TODAY), "read-only");
  assert.equal(headline(TODAY), "Automatic");
  assert.match(stateText(TODAY), /^Read-only/);
  assert.ok(needsRule(TODAY));
  const segs = segments(TODAY);
  assert.deepEqual(segs.map(s => s.label), ["Automatic", "High performance", "Energy saving"]);
  assert.ok(segs.every(s => s.disabled), "every segment is disabled");
  assert.deepEqual(segs.map(s => s.on), [true, false, false], "the current mode is still lit");
  assert.ok(segs.every(s => s.title === "the sudoers rule is not installed"));
});

test("with the rule the control is live and nothing is shown to install", () => {
  assert.equal(macState(WITH_RULE), "editable");
  assert.ok(!needsRule(WITH_RULE));
  const segs = segments(WITH_RULE);
  assert.ok(segs.every(s => !s.disabled));
  assert.equal(segs[1].title, "pmset -a powermode 2");
  // While one POST is in flight, nothing else can be clicked.
  assert.ok(segments(WITH_RULE, { busy: "high_performance" }).every(s => s.disabled));
});

test("AC and battery that disagree are mixed: both shown, no segment lit", () => {
  const mixed = { ...WITH_RULE, ac: "high_performance", battery: "energy_saving" };
  assert.deepEqual(reading(mixed), { ac: "high_performance", battery: "energy_saving", mixed: true, current: null });
  assert.equal(headline(mixed), "Mixed");
  assert.ok(segments(mixed).every(s => !s.on));
  assert.deepEqual(sourceRows(mixed), [
    { source: "AC power", mode: "high_performance", label: "High performance" },
    { source: "Battery", mode: "energy_saving", label: "Energy saving" },
  ]);
});

test("a Mac with no battery is never mixed and shows only AC", () => {
  const desktop = { ...WITH_RULE, ac: "high_performance", battery: null };
  assert.equal(reading(desktop).mixed, false);
  assert.equal(headline(desktop), "High performance");
  assert.deepEqual(sourceRows(desktop).map(r => r.source), ["AC power"]);
});

test("a mode the host does not offer is disabled on its own, not the whole control", () => {
  const air = { ...WITH_RULE, supported: ["automatic", "energy_saving"], permitted: ["automatic", "energy_saving"] };
  const segs = segments(air);
  assert.deepEqual(segs.map(s => s.disabled), [false, true, false]);
  assert.equal(segs[1].title, "this Mac does not offer it");
});

test("not macOS, or no energy mode, is unsupported with no control at all", () => {
  const linux = { ...TODAY, applicable: false, supported: [], ac: null, battery: null };
  assert.equal(macState(linux), "unsupported");
  assert.equal(headline(linux), "Not applicable");
  assert.equal(segments(linux), null);
  assert.ok(!needsRule(linux));
  const none = { ...TODAY, supported: [] };
  assert.equal(macState(none), "unsupported");
  assert.equal(headline(none), "Not supported");
  assert.match(stateText(none), /lowpowermode/);
  assert.equal(segments(none), null);
});

test("a daemon older than the endpoint is unavailable, not an error", () => {
  assert.equal(macFailure(new Error("404 Not Found")), "unavailable");
  assert.equal(macFailure(new Error("invalid request: no")), "error");
  assert.equal(macState(null, { unavailable: true }), "unavailable");
  assert.equal(macState(null, { error: "boom" }), "error");
  assert.equal(macState(null), "loading");
});

test("the POST body is one of three names and nothing else", () => {
  assert.equal(setBody("energy_saving"), '{"mode":"energy_saving"}');
  for (const bad of ["3", 3, "auto; rm", "", null, undefined, "Automatic"]) {
    assert.throws(() => setBody(bad), /not a power mode/, String(bad));
  }
});

test("changes read newest first", () => {
  const report = {
    ...WITH_RULE,
    changes: [
      { at: "2026-10-04T10:00:00Z", by: "the owner", to: "high_performance", message: "power mode: Automatic -> High performance by the owner" },
      { at: "2026-10-04T11:00:00Z", by: "the owner", to: "automatic", message: "" },
    ],
  };
  assert.deepEqual(changeRows(report).map(r => r.text), [
    "Automatic by the owner",
    "power mode: Automatic -> High performance by the owner",
  ]);
  assert.deepEqual(changeRows(null), []);
});

test("the tab sits in L1 after Backup, with its unavailable note and error element", () => {
  assert.match(page, /<button id="tab-backup" hidden>Backup<\/button>\s*<button id="tab-mac" hidden>Mac<\/button>/);
  assert.match(app, /infra: \["infrastructure", "doctor", "environments", "backup", "mac", "dates"\]/);
  const block = page.slice(page.indexOf('<div id="view-mac" hidden>'));
  assert.ok(block.length > 0);
  assert.match(block, /id="mac-unavailable" hidden>[\s\S]*?<code>\/api\/host\/power-mode<\/code>/);
  assert.match(block, /<p class="env-note bad" id="mac-error" hidden><\/p>/);
  assert.match(block, /id="mac-refresh"/);
  assert.match(app, /mac: \{ onShow: startMac, onHide: stopAgentPoll \}/);
  assert.doesNotMatch(app, /setInterval\(refreshMac/, "never polled: every read asks sudo -n -l");
});

test("both modules are served, or the browser 404s them", () => {
  assert.match(served, /"js\/mac\.js"/);
  assert.match(served, /"js\/mac-model\.js"/);
});

test("the view sends only setBody's answer and draws no confirm of its own", () => {
  assert.match(view, /body: setBody\(mode\)/);
  assert.doesNotMatch(view, /confirm\(|alert\(|scrim\(/, "the click is the confirmation");
});

test("the install step names an administrator when the daemon's user is not one", () => {
  // This host: the daemon runs as `factory`, which cannot sudo; `ingo` is the admin.
  const notAdmin = installStep({ ...SUDOERS, admins: ["ingo"], user_is_admin: false });
  assert.equal(notAdmin.kind, "admin");
  assert.equal(notAdmin.su, "su - ingo");
  assert.match(notAdmin.lead, /^Run as an administrator \(ingo\)/);
  assert.match(notAdmin.lead, /factory cannot use sudo itself/);
  const two = installStep({ ...SUDOERS, admins: ["ingo", "ada"], user_is_admin: false });
  assert.equal(two.su, "su - ingo");
  assert.match(two.lead, /\(ingo or ada\)/);
});

test("the install step keeps today's wording when the daemon's user is an admin", () => {
  const self = installStep({ ...SUDOERS, user: "ingo", admins: ["ingo"], user_is_admin: true });
  assert.deepEqual(self, { kind: "self", su: null, lead: "" });
});

test("with no administrator found the install step says from an administrator account", () => {
  for (const s of [{ ...SUDOERS, admins: [], user_is_admin: false }, SUDOERS, null]) {
    const step = installStep(s);
    assert.equal(step.kind, "unknown");
    assert.equal(step.su, null);
    assert.match(step.lead, /from an administrator account/);
  }
});

test("the rule card is an L1 card whose commands wrap inside it", () => {
  assert.match(view, /<article class="infra-card mac-rule"/);
  assert.match(view, /infra-snippet mac-cmd/);
  const css = readFileSync(new URL("../app.css", import.meta.url), "utf8");
  assert.match(css, /\.mac-cmd code \{[^}]*white-space: pre-wrap/);
});
