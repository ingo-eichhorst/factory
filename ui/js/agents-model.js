//! The roster's standing-instance arithmetic (#329), kept away from the DOM:
//! how many instances a standing agent keeps, how many are up, which buttons
//! apply, and the one request the − / + controls send.

const LIVE = new Set(["ready", "starting"]);

/// Every instance the daemon has on record for a roster row. A daemon from
/// before instances sends none; the row itself is then the one instance.
export function instancesOf(agent) {
  if (Array.isArray(agent?.instances) && agent.instances.length) return agent.instances;
  if (!agent || !agent.id) return [];
  return [{ id: agent.id, instance: 1, state: agent.state, attach: agent.attach, error: agent.error }];
}

/// `{ wanted, up, several, label }` for a standing agent; `null` for a task
/// agent, which keeps no standing session at all.
export function instanceSummary(agent) {
  if (!agent || agent.lifetime === "task") return null;
  const instances = instancesOf(agent);
  const wanted = Number.isInteger(agent.wanted) && agent.wanted > 0 ? agent.wanted : 1;
  const up = instances.filter(i => LIVE.has(i.state)).length;
  return {
    wanted,
    up,
    // One standing session is the row it always was: no count, no list.
    several: wanted > 1 || instances.length > 1,
    label: `instances ${up}/${wanted} up`,
  };
}

/// The ids Stop takes down: every live instance, highest first.
export function liveIds(agent) {
  return instancesOf(agent)
    .filter(i => LIVE.has(i.state) && i.id)
    .sort((a, b) => b.instance - a.instance)
    .map(i => i.id);
}

/// Whether Start has anything to do: fewer instances up than wanted.
export function canStart(agent) {
  const summary = instanceSummary(agent);
  return Boolean(summary) && summary.up < summary.wanted;
}

/// The body of `POST /api/agents/scale` for one click of − (-1) or + (+1).
/// Relative, so two people pressing + at once get two more rather than one.
export function scalePayload(scope, name, by, wanted) {
  const target = String(scope ?? "").trim();
  const agent = String(name ?? "");
  if (!target || !agent) throw new Error("Choose a standing agent to scale.");
  const step = Number(by);
  if (!Number.isInteger(step) || step === 0) throw new Error("Scale by a whole number of instances.");
  if (Number.isInteger(wanted) && wanted + step < 1) {
    throw new Error("A standing agent keeps at least one instance; Stop takes it down.");
  }
  return { scope: target, name: agent, by: step };
}
