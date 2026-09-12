import test from "node:test";
import assert from "node:assert/strict";

import { legacyAgentRoute } from "../js/agent-runtime.js";
import { readHash, setRouter } from "../js/scopes.js";

test("the router accepts an old Agents URL and returns its replacement page", () => {
  globalThis.location = { hash: "#all/harn/agents/agent-runtime/task/123" };
  setRouter({
    pages: ["occupancy", "roster", "agent-runtime"],
    redirects: { agents: legacyAgentRoute },
  });

  const route = readHash();

  assert.equal(route.scope, null);
  assert.equal(route.page, "agent-runtime");
  assert.deepEqual(route.tail, ["task", "123"]);
  delete globalThis.location;
});
