//! Pure helpers for the L1 Infrastructure tab: what the machine underneath
//! everything is, and which AI account each agent's harness pays through.
//!
//! The same boundary `bench-model.js` keeps for Benchmarks: nothing here
//! touches the DOM or imports a module that does at load time (`modal.js`
//! does), so `infrastructure.test.js` can import every function below from
//! Node. `infrastructure.js` is the rendering, and only it reaches for `$`.
//!
//! Every host fact on the wire may be `null` -- a fact the daemon could not
//! read is left out, never a failed request -- so every formatter here takes
//! `null` and answers "—" rather than "NaN" or "0".

import { routeHref } from "./scopes.js";

/// What a missing fact is drawn as, everywhere on the page.
export const MISSING = "—";

/// The block the empty state shows: exactly what the root config would need
/// for the provider layer to have anything in it. Mirrors the issue's own
/// example so the page and the documentation say the same thing.
export const PROVIDERS_SNIPPET = `# root .factory/config.yaml
infrastructure:
  providers:
    - name: claude-max
      vendor: anthropic
      kind: subscription          # subscription | api-key
      plan: Max 20x               # free text, optional
      harnesses: [claude-code]    # default binding: every agent on these harnesses
    - name: openrouter
      vendor: openrouter
      kind: api-key
      env: OPENROUTER_API_KEY     # the variable's NAME; its value is never read
      harnesses: [pi, opencode]`;

/// A fact as text, or "—" when the daemon could not read it.
export function fact(v) {
  if (v === null || v === undefined || v === "") return MISSING;
  if (typeof v === "number" && !Number.isFinite(v)) return MISSING;
  return String(v);
}

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"];

/// Bytes the way the machine's own "About" window says them: powers of 1024,
/// one decimal below ten and none above -- 64 GB of memory, a 1.8 TB disk.
export function fmtBytes(n) {
  if (typeof n !== "number" || !Number.isFinite(n) || n < 0) return MISSING;
  let v = n;
  let i = 0;
  while (v >= 1024 && i < UNITS.length - 1) { v /= 1024; i++; }
  if (i === 0) return `${n} B`;
  const shown = v < 10 ? Math.round(v * 10) / 10 : Math.round(v);
  return `${shown} ${UNITS[i]}`;
}

/// A span of seconds as the two largest units that say something:
/// `42s`, `17m`, `5h 12m`, `10d 0h`.
export function fmtUptime(seconds) {
  if (typeof seconds !== "number" || !Number.isFinite(seconds) || seconds < 0) return MISSING;
  const s = Math.floor(seconds);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

/// The one-, five- and fifteen-minute load averages, `uptime`-style. A
/// missing triple is "—"; a missing member of one is "—" in its place, so the
/// other two still say what they know.
export function fmtLoad(load) {
  if (!Array.isArray(load) || load.length === 0) return MISSING;
  return load
    .map(x => (typeof x === "number" && Number.isFinite(x) ? x.toFixed(2) : MISSING))
    .join(" · ");
}

/// How long the daemon has been up, from the `started_at` it reports and the
/// clock the caller passes in -- the wire carries the instant, not a span, so
/// the page counts on its own between refreshes. `null` for a start it cannot
/// read, or one in the future (a skewed clock is not a negative uptime).
export function daemonUptime(startedAt, now = Date.now()) {
  if (!startedAt) return null;
  const t = new Date(startedAt).getTime();
  if (!Number.isFinite(t)) return null;
  const secs = (now instanceof Date ? now.getTime() : now) - t;
  return secs < 0 ? null : Math.floor(secs / 1000);
}

/// Used share of the disk, as a whole percentage 0..100, or `null` when the
/// disk, its size or its free space is unknown -- a bar drawn from a guess
/// would say something the daemon did not.
export function diskPercent(disk) {
  if (!disk) return null;
  const { total_bytes: total, free_bytes: free } = disk;
  if (typeof total !== "number" || typeof free !== "number" || !(total > 0)) return null;
  const pct = Math.round(((total - free) / total) * 100);
  return Math.min(100, Math.max(0, pct));
}

/// How worried the disk bar should look: `ok` below 80%, `warn` to 90%,
/// `full` past it. `null` when there is no percentage to judge.
export function diskLevel(pct) {
  if (pct === null || pct === undefined) return null;
  if (pct >= 90) return "full";
  if (pct >= 80) return "warn";
  return "ok";
}

/// Agents grouped by scope, in the order the daemon listed them -- the same
/// scope-path order the roster uses, so the two pages read alike.
export function groupAgentsByScope(agents) {
  const groups = [];
  const index = new Map();
  for (const a of agents || []) {
    const scope = a && a.scope ? a.scope : "";
    if (!index.has(scope)) {
      index.set(scope, groups.length);
      groups.push({ scope, agents: [] });
    }
    groups[index.get(scope)].agents.push(a);
  }
  return groups;
}

/// Narrow agent rows to the rail's scope selection. Providers themselves are
/// instance-wide -- one account serves every scope -- so it is only the agents
/// listed under them that the rail narrows, the same way `sandboxes.js`
/// narrows its rows and `secrets.js` leaves the home-directory ones alone.
export function visibleAgents(agents, contains) {
  const keep = contains || (() => true);
  return (agents || []).filter(a => keep(a.scope));
}

/// True when no provider is declared -- the page then shows the config
/// snippet instead of an empty provider layer.
export function isEmptyProviders(payload) {
  return !payload || !Array.isArray(payload.providers) || payload.providers.length === 0;
}

/// The badge text a provider's kind is drawn with. Only the two kinds the
/// config accepts are named; anything else is shown as it came.
export function kindBadge(kind) {
  if (kind === "subscription") return "subscription";
  if (kind === "api-key") return "api-key";
  return fact(kind);
}

/// Where an agent's name links to: the roster, with the agent's scope
/// selected on the rail -- the roster has no per-agent anchor, so its scope
/// is the narrowest place a link can land, the same way `roles.js` links a
/// scope to its Roles view.
export function agentHref(scope) {
  return routeHref(scope, "roster");
}

/// The interfaces the daemon listens on, in one line: `cli · http 1.2.3.4:8791`.
export function fmtInterfaces(interfaces) {
  if (!Array.isArray(interfaces) || interfaces.length === 0) return MISSING;
  return interfaces
    .map(i => (i && i.bind ? `${fact(i.kind)} ${i.bind}` : fact(i && i.kind)))
    .join(" · ");
}

/// The runtime, and the herdr session when there is one: `herdr · session factory`.
export function fmtRuntime(daemon) {
  if (!daemon) return MISSING;
  const runtime = fact(daemon.runtime);
  return daemon.herdr_session ? `${runtime} · session ${daemon.herdr_session}` : runtime;
}

/// What a failed fetch of `/api/infrastructure` means. A daemon built before
/// the endpoint existed answers an unknown route with axum's bare 404 -- no
/// JSON body -- which `api()` in core.js turns into "404 Not Found". That is
/// not a fault to paint red; it is a daemon that has not been rebuilt yet.
/// Anything else is a real error.
export function infraFailure(error) {
  const message = error && error.message ? String(error.message) : String(error ?? "");
  return /^404\b/.test(message) ? "unavailable" : "error";
}
