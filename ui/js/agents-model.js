//! The roster's standing-instance arithmetic (#329), kept away from the DOM:
//! how many instances a standing agent keeps, how many are up, which buttons
//! apply, and the one request the − / + controls send. Since #330 also what
//! each instance's own Terminal, Connect and Stop / Start do, and what the page
//! says before it stops one somebody would lose something by.

const LIVE = new Set(["ready", "starting"]);

/// Every instance the daemon has on record for a roster row. A daemon from
/// before instances sends none; the row itself is then the one instance.
export function instancesOf(agent) {
  if (Array.isArray(agent?.instances) && agent.instances.length) return agent.instances;
  if (!agent || !agent.id) return [];
  return [{
    id: agent.id, instance: 1, state: agent.state, attach: agent.attach, error: agent.error,
    session: agent.session, started_at: agent.started_at,
  }];
}

/// `{ wanted, up, several, label }` for a standing agent; `null` for a task
/// agent, which keeps no standing session at all.
export function instanceSummary(agent) {
  if (!agent || agent.lifetime === "task") return null;
  const instances = instancesOf(agent);
  const wanted = Number.isInteger(agent.wanted) && agent.wanted > 0 ? agent.wanted : 1;
  const up = instances.filter(i => LIVE.has(i.state)).length;
  // A stopped one stays stopped until somebody starts it (#330): say so on the
  // count, not only on its own line.
  const stopped = instances.filter(i => i.state === "stopped").length;
  return {
    wanted,
    up,
    stopped,
    // One standing session is the row it always was: no count, no list.
    several: wanted > 1 || instances.length > 1,
    label: `instances ${up}/${wanted} up` + (stopped ? ` · stopped ${stopped} of ${instances.length}` : ""),
  };
}

/// Whether an instance is up: what decides between its Stop and its Start.
export function isLive(instance) {
  return LIVE.has(instance?.state);
}

/// The buttons one instance's line offers (#330). Terminal and Connect need a
/// live session; Connect also needs a command a shell can run -- a shell pane
/// has no named agent, and the daemon then sends neither.
export function instanceActions(instance) {
  const live = isLive(instance);
  return {
    terminal: live,
    connect: live && Boolean(instance.connect || instance.attach),
    stop: live,
    start: !live,
  };
}

/// What Connect shows, in order: Factory's own command, then the runtime's
/// line as the fallback. Both come from the daemon; nothing is assembled here.
export function connectLines(instance) {
  const lines = [];
  if (instance?.connect) lines.push({ label: "connect", command: instance.connect });
  if (instance?.attach) lines.push({ label: "herdr", command: instance.attach });
  return lines;
}

/// `30s`, `12m`, `3h 5m`, `2d 4h`: how long ago `iso` was, as of `now` (ms).
export function ago(iso, now = Date.now()) {
  const secs = Math.max(0, Math.round((now - new Date(iso).getTime()) / 1000));
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ${Math.floor((secs % 3600) / 60)}m`;
  return `${Math.floor(secs / 86400)}d ${Math.floor((secs % 86400) / 3600)}h`;
}

/// The words after an instance's state: how long it has been up and what its
/// runtime last said it was doing, or who stopped it and when.
export function instanceDetail(instance, now = Date.now()) {
  const parts = [];
  if (instance.state === "stopped" && instance.stopped_by) {
    parts.push(`stopped by ${instance.stopped_by}` + (instance.stopped_at ? ` ${ago(instance.stopped_at, now)} ago` : ""));
  }
  if (isLive(instance) && instance.started_at) parts.push(`up ${ago(instance.started_at, now)}`);
  if (isLive(instance) && instance.activity) parts.push(instance.activity);
  return parts.join(" · ");
}

/// The body of `POST /api/agents/start` for one instance's Start.
export function startInstancePayload(scope, name, instance) {
  const n = Number(instance);
  if (!Number.isInteger(n) || n < 1) throw new Error("Instances are counted from 1.");
  return { scope: String(scope ?? ""), name: String(name ?? ""), instance: n };
}

/// What the page asks before stopping, from `/api/agents/stop-check` answers:
/// `null` when nobody would lose anything, otherwise a title and one line per
/// instance naming who is attached or what it is in the middle of.
export function stopPrompt(checks) {
  const costly = (checks || []).filter(c => c && c.live && Array.isArray(c.reasons) && c.reasons.length);
  if (!costly.length) return null;
  const all = (checks || []).filter(c => c && c.live);
  return {
    title: all.length > 1 ? `Stop ${all.length} instances?` : `Stop ${costly[0].id}?`,
    lines: costly.map(c => `${c.id}: ${c.reasons.map(r => r.detail).join("; ")}`),
  };
}

/// One instance by its id, wherever the roster has it: the scope, the agent
/// row and the instance itself. A daemon from before instances is matched on
/// the row's own id.
export function findInstance(scopes, id) {
  for (const scope of scopes || []) {
    for (const agent of scope.agents || []) {
      const instance = instancesOf(agent).find(i => i.id === id);
      if (instance) return { scope: scope.name, agent, instance };
    }
  }
  return null;
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
