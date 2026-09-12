import test from "node:test";
import assert from "node:assert/strict";

import { agentConfigurePayload, agentDeletePayload, argumentsFromLines } from "../js/agent-config.js";

test("CLI arguments keep their order and one-line boundaries", () => {
  assert.deepEqual(argumentsFromLines("--model\nlocal model\n\n--approve"), [
    "--model",
    "local model",
    "--approve",
  ]);
});

test("a standing declaration targets exactly one scope and carries every field", () => {
  assert.deepEqual(agentConfigurePayload(" model-lab ", {
    name: " reviewer ",
    harness: " pi ",
    lifetime: "permanent",
    role: "foreman",
    autostart: true,
    arguments: "--model\nlocal",
  }), {
    scope: "model-lab",
    agent: {
      name: "reviewer",
      harness: "pi",
      lifetime: "permanent",
      role: "foreman",
      autostart: true,
      args: ["--model", "local"],
    },
  });
});

test("a task declaration omits inapplicable autostart and may use the harness as its name", () => {
  const payload = agentConfigurePayload("demo", {
    name: "",
    harness: "codex",
    lifetime: "task",
    role: "worker",
    autostart: true,
  });
  assert.equal(payload.agent.name, null);
  assert.equal("autostart" in payload.agent, false);
});

test("an all-scopes target and names containing a path separator are rejected", () => {
  assert.throws(() => agentConfigurePayload(null, { harness: "pi" }), /Select one scope/);
  assert.throws(() => agentConfigurePayload("demo", {
    name: "other/reviewer",
    harness: "pi",
  }), /cannot contain/);
});

test("deletion targets one exact scope and declaration", () => {
  assert.deepEqual(agentDeletePayload(" model-lab ", "Codex Builder"), {
    scope: "model-lab",
    name: "Codex Builder",
  });
  assert.throws(() => agentDeletePayload("", "reviewer"), /Select one scope/);
  assert.throws(() => agentDeletePayload("demo", ""), /Choose an agent/);
});
