//! Pure form rules for adding a declaration from the roster. Kept away from
//! the DOM so scope targeting and argument ordering can be tested directly.

export function argumentsFromLines(text) {
  return String(text ?? "")
    .split("\n")
    .map(value => value.trim())
    .filter(Boolean);
}

export function agentConfigurePayload(scope, values) {
  const target = String(scope ?? "").trim();
  if (!target) throw new Error("Select one scope before adding an agent.");
  const harness = String(values.harness ?? "").trim();
  if (!harness) throw new Error("Choose an agent harness.");
  const name = String(values.name ?? "").trim();
  if (name.includes("/")) throw new Error("An agent name cannot contain /.");
  const lifetime = String(values.lifetime ?? "task");
  if (!["task", "temporary", "permanent"].includes(lifetime)) {
    throw new Error("Choose a valid lifetime.");
  }
  const role = String(values.role ?? "worker").trim();
  if (!role) throw new Error("Choose a role.");
  const sandbox = String(values.sandbox ?? "none");
  if (!["none", "docker", "srt"].includes(sandbox)) {
    throw new Error("Choose a valid sandbox.");
  }

  const agent = {
    name: name || null,
    harness,
    lifetime,
    role,
    args: argumentsFromLines(values.arguments),
    sandbox,
  };
  if (lifetime !== "task") agent.autostart = Boolean(values.autostart);
  return { scope: target, agent };
}

export function agentDeletePayload(scope, name) {
  const target = String(scope ?? "").trim();
  if (!target) throw new Error("Select one scope before deleting an agent.");
  const agent = String(name ?? "");
  if (!agent) throw new Error("Choose an agent declaration to delete.");
  return { scope: target, name: agent };
}
