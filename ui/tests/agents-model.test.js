import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  ago, canRename, canStart, connectLines, findInstance, instanceActions, instanceDetail, instanceSummary, instancesOf,
  isLive, liveIds, renameNotice, renamePayload, scalePayload, startInstancePayload, stopPrompt,
} from "../js/agents-model.js";

const view = readFileSync(new URL("../js/agents.js", import.meta.url), "utf8");
const served = readFileSync(new URL("../../crates/factory-daemon/src/ui.rs", import.meta.url), "utf8");

const three = {
  name: "claude", lifetime: "permanent", id: "root/claude", state: "ready", wanted: 3,
  instances: [
    { id: "root/claude", instance: 1, state: "ready" },
    { id: "root/claude#2", instance: 2, state: "starting" },
    { id: "root/claude#3", instance: 3, state: "gone" },
  ],
};

test("a standing agent with several instances says how many are up of how many it keeps", () => {
  assert.deepEqual(instanceSummary(three), { wanted: 3, up: 2, stopped: 0, several: true, label: "instances 2/3 up" });
  assert.equal(canStart(three), true, "one of three is down");
  assert.deepEqual(liveIds(three), ["root/claude#2", "root/claude"], "Stop takes the highest first");
});

test("one standing session is the row it always was", () => {
  const one = { name: "watcher", lifetime: "permanent", id: "demo/watcher", state: "ready", wanted: 1,
    instances: [{ id: "demo/watcher", instance: 1, state: "ready" }] };
  assert.equal(instanceSummary(one).several, false);
  assert.equal(canStart(one), false);
  // A daemon from before instances sends neither `wanted` nor `instances`.
  const old = { name: "watcher", lifetime: "temporary", id: "demo/watcher", state: "stopped" };
  assert.deepEqual(instancesOf(old).map(i => i.id), ["demo/watcher"]);
  assert.deepEqual(instanceSummary(old), { wanted: 1, up: 0, stopped: 1, several: false, label: "instances 0/1 up · stopped 1 of 1" });
  assert.equal(canStart(old), true);
});

test("a draining surplus instance still shows, and still counts as up", () => {
  const draining = { ...three, wanted: 1, instances: [
    { id: "root/claude", instance: 1, state: "ready" },
    { id: "root/claude#2", instance: 2, state: "ready", surplus: true },
  ] };
  assert.deepEqual(instanceSummary(draining), { wanted: 1, up: 2, stopped: 0, several: true, label: "instances 2/1 up" });
});

test("a task agent keeps no standing session", () => {
  assert.equal(instanceSummary({ name: "codex", lifetime: "task" }), null);
  assert.equal(canStart({ name: "codex", lifetime: "task" }), false);
  assert.deepEqual(liveIds({ name: "codex", lifetime: "task" }), []);
});

test("− and + send one relative step, never below one instance", () => {
  assert.deepEqual(scalePayload(" root ", "claude", 1, 3), { scope: "root", name: "claude", by: 1 });
  assert.deepEqual(scalePayload("root", "claude", -1, 3), { scope: "root", name: "claude", by: -1 });
  assert.throws(() => scalePayload("root", "claude", -1, 1), /at least one instance/);
  assert.throws(() => scalePayload("", "claude", 1, 1), /standing agent/);
  assert.throws(() => scalePayload("root", "claude", 0, 1), /whole number/);
});

// ------------------------------------------------- one instance at a time (#330)

const NOW = Date.parse("2026-10-21T12:00:00Z");

/// Two standing `shell` sessions of one declaration, as `/api/agents` sends
/// them: #1 up and being watched, #2 stopped by the owner. A shell pane is no
/// named agent, so neither carries a command to attach with.
const twoShells = {
  name: "watcher", adapter: "shell", lifetime: "permanent", id: "demo/watcher", state: "ready", wanted: 2,
  deletable: true,
  instances: [
    { id: "demo/watcher", instance: 1, state: "ready", session: "p1", started_at: "2026-10-21T09:55:00Z",
      activity: "idle", attended: "its terminal was open in Factory 3s ago" },
    { id: "demo/watcher#2", instance: 2, state: "stopped", stopped_by: "the owner", stopped_at: "2026-10-21T11:59:20Z" },
  ],
};

test("two shell instances: each has its own buttons, and a stopped one shows Start, not Stop", () => {
  const [one, two] = instancesOf(twoShells);
  assert.deepEqual(instanceActions(one), { terminal: true, connect: false, stop: true, start: false },
    "a shell pane has nothing to connect with: the page offers Terminal instead");
  assert.deepEqual(instanceActions(two), { terminal: false, connect: false, stop: false, start: true });
  assert.equal(isLive(one), true);
  assert.equal(isLive(two), false);
  assert.deepEqual(liveIds(twoShells), ["demo/watcher"], "the row's Stop takes only what is up");
  assert.equal(canStart(twoShells), true, "one of two is down");
  assert.equal(instanceSummary(twoShells).label, "instances 1/2 up · stopped 1 of 2");
});

test("each instance says how long it has been up and what it is doing, or who stopped it", () => {
  const [one, two] = instancesOf(twoShells);
  assert.equal(instanceDetail(one, NOW), "up 2h 5m · idle");
  assert.equal(instanceDetail(two, NOW), "stopped by the owner 40s ago");
  assert.equal(ago("2026-10-19T08:00:00Z", NOW), "2d 4h");
  assert.equal(ago("2026-10-21T11:48:00Z", NOW), "12m");
});

test("Start on one instance names it; the ids come from the daemon, never from the page", () => {
  assert.deepEqual(startInstancePayload("demo", "watcher", "2"), { scope: "demo", name: "watcher", instance: 2 });
  assert.throws(() => startInstancePayload("demo", "watcher", 0), /counted from 1/);
  assert.equal(findInstance([{ name: "demo", agents: [twoShells] }], "demo/watcher#2").instance.instance, 2);
  assert.equal(findInstance([{ name: "demo", agents: [twoShells] }], "demo/watcher").scope, "demo");
  assert.equal(findInstance([{ name: "demo", agents: [twoShells] }], "demo/watcher#3"), null);
  // An old daemon's row is its own one instance.
  const old = { name: "w", lifetime: "permanent", id: "demo/w", state: "ready", attach: "herdr agent attach x" };
  assert.equal(findInstance([{ name: "demo", agents: [old] }], "demo/w").instance.attach, "herdr agent attach x");
  // The view never assembles an id or an attach command of its own.
  assert.doesNotMatch(view, /`\$\{[^}]*\}#\$\{/, "no `${name}#${n}` in the page");
  assert.doesNotMatch(view, /factory agent attach/, "the connect command is the daemon's");
});

test("Connect shows Factory's command first and the runtime's own line as the fallback", () => {
  const named = { id: "demo/claude#2", instance: 2, state: "ready",
    connect: "factory agent attach 'demo/claude#2'", attach: "herdr --session factory agent attach factory-demo-claude-2-ab12cd34" };
  assert.deepEqual(connectLines(named), [
    { label: "connect", command: "factory agent attach 'demo/claude#2'" },
    { label: "herdr", command: "herdr --session factory agent attach factory-demo-claude-2-ab12cd34" },
  ]);
  assert.equal(instanceActions(named).connect, true);
  assert.deepEqual(connectLines(instancesOf(twoShells)[0]), []);
});

test("stopping asks first only when somebody would lose something, and names who or what", () => {
  assert.equal(stopPrompt([{ id: "demo/watcher", instance: 1, live: true }]), null);
  assert.equal(stopPrompt([]), null);
  assert.equal(stopPrompt([{ id: "demo/watcher#2", live: false, reasons: [] }]), null, "nothing to stop");
  const attached = { id: "demo/watcher", instance: 1, live: true, reasons: [
    { why: "attached", detail: "its terminal was open in Factory 3s ago" },
    { why: "working", detail: "it is working" },
  ] };
  assert.deepEqual(stopPrompt([attached]), {
    title: "Stop demo/watcher?",
    lines: ["demo/watcher: its terminal was open in Factory 3s ago; it is working"],
  });
  // The row's Stop over several: one prompt, naming only the ones that cost.
  assert.deepEqual(stopPrompt([attached, { id: "demo/watcher#2", instance: 2, live: true }]), {
    title: "Stop 2 instances?",
    lines: ["demo/watcher: its terminal was open in Factory 3s ago; it is working"],
  });
});

test("the roster asks the daemon before it stops one, in the page, and acts on one instance", () => {
  const code = view.split("\n").filter((l) => !l.trim().startsWith("//")).join("\n");
  assert.match(code, /\/api\/agents\/stop-check/);
  assert.match(code, /data-act="stop-confirm"/);
  assert.match(code, /data-instance=/, "an instance's Start names it");
  assert.match(code, /data-act="connect"/);
  // The confirmation is the page's own, never the browser's.
  const stopping = code.slice(code.indexOf('act === "stop"'), code.indexOf('act === "scale"'));
  assert.doesNotMatch(stopping, /(?<![.\w])(alert|confirm|prompt)\(/);
  assert.match(served, /"js\/agents-model\.js"/);
});

// ------------------------------------------------------------- rename (#351)

test("Rename is offered exactly where Delete is: a local declaration, its scope selected", () => {
  const local = { name: "pi", deletable: true };
  assert.equal(canRename(local, "projects/factory", "projects/factory"), true);
  assert.equal(canRename(local, "", "projects/factory"), false, "no scope selected");
  assert.equal(canRename(local, "projects/other", "projects/factory"), false);
  assert.equal(canRename({ name: "foreman", deletable: false }, "projects/factory", "projects/factory"), false, "a foreman");
  assert.equal(canRename({ name: "claude-code", declared: false }, "projects/factory", "projects/factory"), false, "undeclared");
  assert.equal(canRename(null, "a", "a"), false);
});

test("a new name is checked as the daemon checks it, and an unchanged one sends nothing", () => {
  assert.deepEqual(renamePayload(" projects/factory ", "Codex Builder", "  reviewer "), {
    scope: "projects/factory", from: "Codex Builder", to: "reviewer",
  });
  assert.deepEqual(renamePayload("demo", "pi", "Lead Reviewer").to, "Lead Reviewer", "spaces inside are fine");
  assert.equal(renamePayload("demo", "pi", " pi "), null, "unchanged");
  assert.throws(() => renamePayload("demo", "pi", "   "), /needs a name/);
  for (const bad of ["a/b", "pi#2", "two\nlines", "tab\there", "bell\u0007"]) {
    assert.throws(() => renamePayload("demo", "pi", bad), /cannot contain/, JSON.stringify(bad));
  }
  assert.throws(() => renamePayload("", "pi", "x"), /Select one scope/);
  assert.throws(() => renamePayload("demo", "", "x"), /Choose an agent/);
});

test("after a rename the roster says what moved and what still names the old agent", () => {
  const notice = renameNotice({
    scope: "factory", from: "builder", to: "implementer", changed: true,
    tasks: ["t1", "t2"],
    sessions: { moved: [], started: ["factory/implementer"] },
    files: ["workflows/github-issue.yaml"],
    workflows: ["workflow github-issue (w1: triage, implement)"],
  });
  assert.deepEqual(notice, {
    title: "builder is now implementer",
    lines: [
      "2 waiting tasks moved to implementer.",
      "Starting again as factory/implementer.",
      "Still names builder, not rewritten: workflows/github-issue.yaml",
      "Still names builder, not rewritten: workflow github-issue (w1: triage, implement)",
    ],
    stale: true,
  });
  assert.deepEqual(renameNotice({ from: "pi", to: "scratch", changed: true, tasks: ["t"] }).lines, ["1 waiting task moved to scratch."]);
  assert.equal(renameNotice({ from: "pi", to: "pi", changed: false }), null);
  assert.equal(renameNotice(null), null);
});

test("the roster offers Rename beside Delete, opens its dialog and keeps the answer on the page", () => {
  assert.match(view, /canRename\(a, state\.scope, s\.name\)/);
  assert.match(view, /data-act="rename"/);
  assert.match(view, /\/api\/agents\/rename/);
  assert.match(view, /renameNotice\(/);
  // Start task… names the agent, not its harness: a renamed `pi` row must not
  // hand its task to the bare `pi` adapter.
  assert.match(view, /data-act="task" data-scope="\$\{esc\(s\.name\)\}" data-agent="\$\{esc\(a\.name\)\}"/);
});
