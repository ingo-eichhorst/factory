import test from "node:test";
import assert from "node:assert/strict";

import { state } from "../js/core.js";
import {
  legacyAgentRoute,
  loadRuntimeConnections,
  runtimeConnectionCard,
  visibleRuntimeConnections,
} from "../js/agent-runtime.js";

const healthy = {
  runtime: "herdr",
  source: "builtin",
  description: "one workspace per task",
  scopes: ["factory", "model-lab"],
  checked_at: new Date().toISOString(),
  state: "healthy",
  session: "factory",
  endpoint: "/tmp/herdr.sock",
  client: { version: "0.8.0", protocol: 19 },
  server: { version: "0.8.1", protocol: 19 },
  compatible: true,
  restart_needed: false,
  capabilities: ["live_handoff"],
};

test("old Agents routes resolve to the new peer pages", () => {
  assert.deepEqual(legacyAgentRoute([]), { page: "occupancy", tail: [] });
  assert.deepEqual(legacyAgentRoute(["roster", "task", "123"]), {
    page: "roster",
    tail: ["task", "123"],
  });
  assert.deepEqual(legacyAgentRoute(["agent-runtime"]), {
    page: "agent-runtime",
    tail: [],
  });
});

test("a grouped connection keeps only scopes inside the rail selection", () => {
  const visible = visibleRuntimeConnections(
    [healthy, { ...healthy, runtime: "other", scopes: ["elsewhere"] }],
    scope => scope === "model-lab",
  );
  assert.equal(visible.length, 1);
  assert.equal(visible[0].runtime, "herdr");
  assert.deepEqual(visible[0].scopes, ["model-lab"]);
});

test("healthy and error cards render connection facts without duplicating scopes", () => {
  const card = runtimeConnectionCard(healthy);
  assert.match(card, /Connected and compatible/);
  assert.match(card, /0\.8\.0/);
  assert.match(card, /protocol 19/);
  assert.match(card, /factory · model-lab/);
  assert.equal((card.match(/<article/g) || []).length, 1);

  const error = runtimeConnectionCard({
    ...healthy,
    state: "error",
    error: "could not read status",
    client: null,
    server: null,
  });
  assert.match(error, /could not obtain/);
  assert.match(error, /could not read status/);
});

test("refresh fetches the diagnostic, renders it, and releases the button", async () => {
  const elements = {
    "runtime-refresh": { disabled: false },
    "agent-runtime": { innerHTML: "" },
  };
  globalThis.document = { getElementById: id => elements[id] || null };
  let requested;
  globalThis.fetch = async path => {
    requested = path;
    return {
      status: 200,
      statusText: "OK",
      json: async () => ({ status: "ok", data: { kind: "runtime_connections", runtimes: [healthy] } }),
    };
  };
  state.scope = null;

  await loadRuntimeConnections();

  assert.equal(requested, "/api/agent-runtime");
  assert.equal(elements["runtime-refresh"].disabled, false);
  assert.match(elements["agent-runtime"].innerHTML, /herdr/);
  assert.match(elements["agent-runtime"].innerHTML, /healthy/);

  globalThis.fetch = async () => ({
    status: 500,
    statusText: "Server Error",
    json: async () => ({ status: "error", message: "runtime probe unavailable" }),
  });
  await loadRuntimeConnections();
  assert.match(elements["agent-runtime"].innerHTML, /runtime probe unavailable/);
  assert.doesNotMatch(elements["agent-runtime"].innerHTML, /Connected and compatible/);

  delete globalThis.fetch;
  delete globalThis.document;
});
