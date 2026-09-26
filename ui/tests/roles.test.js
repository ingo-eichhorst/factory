import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { state } from "../js/core.js";
import { legacyAgentRoute } from "../js/agent-runtime.js";
import { readHash, setRouter } from "../js/scopes.js";

// `modal.js`, which the view opens its form in, listens for Escape on the
// document as it loads -- so there has to be one before the view is imported.
const bare = { addEventListener() {}, getElementById: () => null };
globalThis.document = bare;
const {
  grantGroups,
  layerTree,
  loadRoles,
  originBadge,
  reachText,
  roleCard,
  roleDefinePayload,
  rolesHref,
} = await import("../js/roles.js");

const page = readFileSync(new URL("../index.html", import.meta.url), "utf8");
const wiring = readFileSync(new URL("../js/app.js", import.meta.url), "utf8");

// The vocabulary as the daemon serves it, in `Grant::ALL` order.
const grants = [
  { name: "task.create", describe: "create tasks", group: "Tasks" },
  { name: "task.edit", describe: "change tasks", group: "Tasks" },
  { name: "task.report", describe: "report on tasks", group: "Tasks" },
  { name: "agent.start", describe: "start agents", group: "Agents" },
  { name: "run.input", describe: "type into a run's session", group: "Runs" },
  // `#172`: a role's grants can include a group the daemon has never sent
  // before -- the view groups by whatever `group` says, not a fixed list.
  { name: "intake.triage", describe: "start a triage run on an intake item", group: "Intake" },
];

const here = { kind: "scope", scope: "projects/demo" };
const reviewer = {
  name: "reviewer",
  describe: "reviews, and may also open follow-up tasks",
  grants: ["task.create", "task.edit", "task.report"],
  reach: "own",
  origin: here,
  overrides: { kind: "scope", scope: "projects" },
  held_by: [{ name: "critic", given: false }, { name: "helper", given: true }],
};
const runner = {
  name: "runner",
  describe: "starts the work that is already on the board",
  grants: [],
  reach: "scope",
  origin: { kind: "scope", scope: "projects" },
};
const worker = { name: "worker", describe: "reads the board", grants: ["task.edit"], reach: "own", origin: { kind: "builtin" } };

test("the Roles tab sits after Agent-runtime and says roles are guard-rails", () => {
  assert.match(page, /id="tab-agent-runtime"[^>]*>Agent-runtime<\/button>\s*<button id="tab-roles"[^>]*>Roles<\/button>/);
  assert.match(page, /id="view-roles"/);
  assert.match(page, /guard-rails, not a security boundary/);
  assert.match(wiring, /harn: \["occupancy", "roster", "agent-runtime", "roles"\]/);
});

test("a roles link survives a reload, and an old Agents link still finds it", () => {
  setRouter({ pages: ["occupancy", "roster", "agent-runtime", "roles"], redirects: { agents: legacyAgentRoute } });
  globalThis.location = { hash: rolesHref("projects/demo") };
  const route = readHash();
  assert.equal(route.scope, "projects/demo");
  assert.equal(route.page, "roles");
  assert.deepEqual(legacyAgentRoute(["roles"]), { page: "roles", tail: [] });
  assert.match(rolesHref("all"), /^#%61ll\//, "a scope called all is not the keyword");
  delete globalThis.location;
});

test("the badge says where a role was written, relative to the scope on screen", () => {
  assert.deepEqual(originBadge(worker, here), { text: "built-in", cls: "builtin", here: false });
  assert.equal(originBadge(reviewer, here).text, "defined here · overrides projects");
  assert.equal(originBadge({ ...reviewer, overrides: null }, here).text, "defined here");
  const inherited = originBadge(runner, here);
  assert.equal(inherited.text, "inherited from projects");
  assert.equal(inherited.link, "projects");
  assert.equal(originBadge({ ...runner, origin: { kind: "instance" } }, here).text, "instance");
  assert.equal(
    originBadge({ ...runner, origin: { kind: "instance" } }, { kind: "instance" }).text,
    "defined here",
    "the root scope writes the instance's roles",
  );
});

test("reach is said in words, with the rule grants cannot express", () => {
  assert.match(reachText("own"), /only its own work/);
  assert.match(reachText("own"), /never whose it is/);
  assert.equal(reachText("scope", "projects/demo"), "everything in projects/demo, never past it");
});

test("a card shows what a role may not do as well as what it may", () => {
  const groups = grantGroups(reviewer, grants);
  assert.deepEqual(groups.map(g => g.group), ["Tasks", "Agents", "Runs", "Intake"]);
  assert.equal(groups[1].items[0].allowed, false);
  // A group is rendered whenever the vocabulary carries it, whether or not
  // this particular role holds anything in it (`#172`'s Intake group, for a
  // reviewer that holds none of it).
  assert.equal(groups[3].group, "Intake");
  assert.equal(groups[3].items[0].allowed, false);

  const card = roleCard(reviewer, grants, { scope: "projects/demo", writes: here, actions: true });
  assert.match(card, /✓<\/span> create tasks/);
  assert.match(card, /✗<\/span> start agents/);
  assert.match(card, /✗<\/span> start a triage run on an intake item/);
  assert.match(card, /critic/);
  assert.match(card, /helper <span class="tag"[^>]*>given<\/span>/);
  assert.match(card, /data-role-act="edit"/);
  assert.match(card, /data-role-act="delete"/);
  assert.doesNotMatch(card, /data-role-act="override"/);

  const inherited = roleCard(runner, grants, { scope: "projects/demo", writes: here, actions: true });
  assert.match(inherited, /data-role-act="override"/);
  assert.doesNotMatch(inherited, /data-role-act="delete"/);
  assert.match(inherited, /href="#projects\/(harn\/)?roles"/);
  assert.match(inherited, /nobody/);

  const shipped = roleCard(worker, grants, { scope: "projects/demo", writes: here, actions: true });
  assert.doesNotMatch(shipped, /data-role-act/, "what ships is not edited here");
});

test("All scopes shows the inheritance: what ships and the instance once, then scopes nested by path", () => {
  const html = layerTree({
    grants,
    layers: [
      { origin: { kind: "builtin" }, roles: [worker] },
      { origin: { kind: "instance" }, roles: [] },
      { origin: { kind: "scope", scope: "projects" }, roles: [{ ...runner, overrides: null }] },
      { origin: here, parent: "projects", roles: [reviewer] },
    ],
  });
  assert.match(html, /Built in/);
  assert.match(html, /defines no roles of its own/);
  assert.match(html, /style="--d:0"[\s\S]*projects[\s\S]*style="--d:1"[\s\S]*projects\/demo/);
  assert.match(html, /replaces projects's<\/span> <strong>reviewer/);
  assert.match(html, /adds<\/span> <strong>runner/);
});

test("the define payload refuses what the daemon would refuse, with a sentence", () => {
  assert.throws(() => roleDefinePayload(null, { name: "x" }), /Select one scope/);
  assert.throws(() => roleDefinePayload("demo", { name: "has space" }), /no spaces/);
  assert.throws(() => roleDefinePayload("demo", { name: "worker" }), /built in/);
  assert.throws(() => roleDefinePayload("demo", { name: "triager" }), /built in/);
  assert.deepEqual(
    roleDefinePayload("demo", { name: " reviewer ", describe: "", grants: ["task.edit", "task.edit"], reach: "scope" }, true),
    { scope: "demo", name: "reviewer", role: { grants: ["task.edit"], reach: "scope" }, replace: true },
  );
});

test("loading asks for the selected scope and shows a failure instead of the last answer", async () => {
  const elements = {
    roles: { innerHTML: "", querySelectorAll: () => [] },
    "roles-new": { disabled: false, title: "" },
  };
  globalThis.document = { ...bare, getElementById: id => elements[id] || null };
  let requested;
  globalThis.fetch = async path => {
    requested = path;
    return {
      status: 200,
      statusText: "OK",
      json: async () => ({
        status: "ok",
        data: { kind: "roles", board: { grants, scope: "projects/demo", writes: here, roles: [reviewer, runner, worker], layers: [] } },
      }),
    };
  };
  state.scope = "projects/demo";
  state.scopes = [];

  await loadRoles();
  assert.equal(requested, "/api/roles?scope=projects%2Fdemo");
  assert.match(elements.roles.innerHTML, /every role may read the board/i);
  assert.match(elements.roles.innerHTML, /defined here · overrides projects/);
  assert.equal(elements["roles-new"].disabled, false);

  globalThis.fetch = async () => ({
    status: 400,
    statusText: "Bad Request",
    json: async () => ({ status: "error", message: "no such scope: gone" }),
  });
  await loadRoles();
  assert.match(elements.roles.innerHTML, /no such scope: gone/);
  assert.doesNotMatch(elements.roles.innerHTML, /reviewer/);

  state.scope = null;
  await loadRoles();
  assert.equal(elements["roles-new"].disabled, true, "nothing to write into with All scopes selected");

  delete globalThis.fetch;
  globalThis.document = bare;
});
