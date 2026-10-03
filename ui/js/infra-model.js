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

/// When a rate-limit window resets, relative to `now`: "resets in 2h 10m".
/// `null` when the time is missing or unreadable -- never a guess.
export function resetsIn(resetsAt, now = Date.now()) {
  if (!resetsAt) return null;
  const t = new Date(resetsAt).getTime();
  if (!Number.isFinite(t)) return null;
  const secs = (t - (now instanceof Date ? now.getTime() : now)) / 1000;
  if (secs <= 0) return "reset passed";
  const m = Math.max(1, Math.ceil(secs / 60));
  if (m < 60) return `resets in ${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `resets in ${h}h ${m % 60}m`;
  return `resets in ${Math.floor(h / 24)}d ${h % 24}h`;
}

/// A provider's rate-limit window as an honest bar model. Unknown readings
/// have no percentage and stale/apportioned observations keep those labels.
export function providerWindowView(window, now = Date.now()) {
  if (!window) return { name: "window", percent: null, tone: "unknown", detail: "unknown" };
  const name = window.window_minutes === 300
    ? "5-hour"
    : (window.window_minutes === 10080 ? "weekly" : `${fact(window.window_minutes)}-minute`);
  const used = typeof window.used_percent === "number" && Number.isFinite(window.used_percent)
    ? Math.min(100, Math.max(0, window.used_percent))
    : null;
  const labels = [];
  if (used === null) labels.push(window.unknown || "usage unknown");
  else labels.push(`${Math.round(used * 10) / 10}% used`);
  const resets = resetsIn(window.resets_at, now);
  if (resets) labels.push(resets);
  if (window.stale) labels.push(window.sample_time_estimated ? "stale · sample time not reported" : "stale");
  if (window.attribution) labels.push(window.attribution);
  else if (window.attribution_unknown) labels.push(`attribution unknown — ${window.attribution_unknown}`);
  if (window.attribution_quality) labels.push(window.attribution_quality);
  if (typeof window.trend_percent === "number" && Number.isFinite(window.trend_percent)) {
    labels.push(`${window.trend_percent >= 0 ? "+" : ""}${window.trend_percent.toFixed(1)} points`);
  }
  return {
    name,
    percent: used,
    tone: used === null ? "unknown" : (window.stale ? "stale" : "known"),
    detail: labels.join(" · "),
  };
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

// ------------------------------------------------------------------ harnesses

/// A harness's state as the page says it (#131). `unprobed` is not a fault:
/// nothing asked for that harness since the daemon started, and the page
/// never probes anything to fill itself in.
export const HARNESS_STATES = {
  healthy: "starts",
  unhealthy: "does not start",
  unprobed: "not checked yet",
};

export function harnessStateLabel(state) {
  return HARNESS_STATES[state] || fact(state);
}

/// The one line under a harness's name: what it answered, why it is down,
/// or that nothing has asked for it yet.
export function harnessDetail(row) {
  if (!row) return MISSING;
  if (row.state === "healthy") return row.version ? String(row.version) : "answered --version";
  if (row.state === "unhealthy") return row.reason ? String(row.reason) : "its probe failed";
  return "probed before the next dispatch to it";
}

/// The unhealthy ones first -- they are the reason to open the page -- then
/// the rest in the order the daemon listed them.
export function sortHarnesses(rows) {
  const list = Array.isArray(rows) ? rows.slice() : [];
  return list
    .map((row, i) => ({ row, i }))
    .sort((a, b) => (b.row.state === "unhealthy") - (a.row.state === "unhealthy") || a.i - b.i)
    .map(x => x.row);
}
