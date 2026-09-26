//! Pure helpers for the L1 Backup tab (#116): is the company's state backed
//! up, how recently, and could it actually be restored.
//!
//! The same boundary `infra-model.js` keeps: nothing here touches the DOM or
//! imports a module that does at load time, so `backup.test.js` can import
//! every function below from Node. `backup.js` is the rendering.
//!
//! The daemon decides everything that is a judgement -- the age level, the
//! warnings, which retention rule keeps a snapshot -- and sends it as data.
//! These helpers only put words and colours to it; nothing here guesses a
//! fact the report did not state.

import { MISSING, fmtBytes } from "./infra-model.js";

export { MISSING, fmtBytes };

/// What the root config needs for the page to have anything to show.
/// Mirrors the issue's own example, less `encrypt_to`, which v1 refuses.
export const CONFIG_SNIPPET = `# root .factory/config.yaml
infrastructure:
  backup:
    destination: /Volumes/Backup/factory     # an external disk, NAS mount or synced folder
    schedule: { cron: "0 3 * * *", timezone: Europe/Berlin }
    keep: { daily: 7, weekly: 4, monthly: 6 } # grandfather-father-son
    include_logs: false`;

/// The hero's colour and word for the newest backup's age. `level` is the
/// three-colour scale the page draws with: ok (green), warn (amber), bad
/// (red).
export function ageBadge(report) {
  if (!report || !report.config) return { level: "bad", label: "Not configured" };
  switch (report.age) {
    // "Fresh", not "backed up": the age is the one thing this badge judges.
    // Whether the copy is somewhere a disk failure cannot reach is the
    // warning strip's to say, and it does.
    case "fresh": return { level: "ok", label: "Fresh" };
    case "stale": return { level: "warn", label: "Stale" };
    case "overdue": return { level: "bad", label: "Overdue" };
    default: return { level: "bad", label: "No backup yet" };
  }
}

/// An instant as epoch milliseconds, or `null` -- never 1970 for a missing
/// one, which is what `new Date(null)` would quietly answer.
function ms(value) {
  if (value === null || value === undefined || value === "") return null;
  if (value instanceof Date) return value.getTime();
  if (typeof value === "number") return value;
  const t = new Date(value).getTime();
  return Number.isFinite(t) ? t : null;
}

/// A span of time as the two largest units that say something: `42s`,
/// `17m`, `5h 12m`, `3d 4h`.
export function fmtSpan(seconds) {
  if (typeof seconds !== "number" || !Number.isFinite(seconds) || seconds < 0) return MISSING;
  const s = Math.floor(seconds);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

/// `3h 12m ago`, read against `now` -- the report's own server clock, so
/// every age on the page agrees with the level the daemon decided on.
export function fmtAgo(now, then) {
  const a = ms(now);
  const b = ms(then);
  if (a === null || b === null) return MISSING;
  return `${fmtSpan(Math.max(0, (a - b) / 1000))} ago`;
}

/// `in 5h 12m`, or `now` for a moment already due.
export function fmtIn(now, then) {
  const a = ms(now);
  const b = ms(then);
  if (a === null || b === null) return MISSING;
  const secs = (b - a) / 1000;
  return secs <= 60 ? "now" : `in ${fmtSpan(secs)}`;
}

/// An instant as `2026-09-25 03:00 UTC`. UTC on purpose: the archive names
/// and the CLI spell it the same way, so the three can be matched by eye.
export function fmtWhen(iso) {
  const t = ms(iso);
  if (t === null) return MISSING;
  const d = new Date(t);
  const p = (n) => String(n).padStart(2, "0");
  return `${d.getUTCFullYear()}-${p(d.getUTCMonth() + 1)}-${p(d.getUTCDate())} ${p(d.getUTCHours())}:${p(d.getUTCMinutes())} UTC`;
}

/// The destination in one line: where it is, whether it is really
/// somewhere else, and how much room is left.
export function destinationText(dest) {
  if (!dest) return MISSING;
  if (!dest.exists) return "missing -- not created yet, or the disk is not mounted";
  const device = dest.same_device === true
    ? "same device as the instance"
    : dest.same_device === false ? "another device" : "device unknown";
  const free = typeof dest.free_bytes === "number" ? ` · ${fmtBytes(dest.free_bytes)} free` : "";
  return `${device}${free}`;
}

/// The level the destination line is drawn at: a copy on the same disk is
/// no backup, and a missing destination takes none.
export function destinationLevel(dest) {
  if (!dest || !dest.exists || dest.same_device === true) return "bad";
  if (dest.same_device === false) return "ok";
  return "warn";
}

/// `0 3 * * * (Europe/Berlin)`, or the honest alternative.
export function scheduleText(config) {
  if (!config) return MISSING;
  const s = config.schedule;
  if (!s || !s.cron) return "none -- only when somebody runs one";
  return `${s.cron} (${s.timezone || "UTC"})`;
}

export function keepText(config) {
  const k = config && config.keep;
  if (!k) return MISSING;
  return `${k.daily} daily · ${k.weekly} weekly · ${k.monthly} monthly`;
}

/// Which retention rules keep a snapshot, as a person says it. A snapshot
/// no rule keeps goes at the next backup, and the row says so.
export function keptByText(kept) {
  if (!Array.isArray(kept) || kept.length === 0) return "deleted at the next backup";
  return kept.join(" · ");
}

/// A snapshot's verification cell: never verified is not a failure, but
/// it is not a pass either.
export function verifiedCell(verified) {
  if (!verified) return { level: "none", text: "not verified" };
  return verified.ok
    ? { level: "ok", text: `verified ${fmtWhen(verified.at)}` }
    : { level: "bad", text: `FAILED ${fmtWhen(verified.at)}` };
}

/// The hero's "last verified" line.
export function lastVerifiedText(report) {
  const v = report && report.last_verified;
  if (!v) return "never";
  return `${v.ok ? "passed" : "FAILED"} ${fmtAgo(report.now, v.at)} (${fmtWhen(v.at)})`;
}

/// Files and bytes for an include row: unknown before the first backup this
/// daemon took, `off` for an optional directory not included.
export function includeCount(row) {
  if (!row || !row.included) return "off";
  if (typeof row.files !== "number") return MISSING;
  return `${row.files} file${row.files === 1 ? "" : "s"} · ${fmtBytes(row.bytes)}`;
}

/// What the two buttons may do right now, and why not when they may not.
export function actions(report) {
  if (!report || !report.config) {
    return { run: false, verify: false, why: "configure infrastructure.backup first" };
  }
  if (report.running) {
    return { run: false, verify: false, why: "a backup operation is running" };
  }
  const any = Array.isArray(report.snapshots) && report.snapshots.length > 0;
  return { run: true, verify: any, why: any ? "" : "there is no snapshot to verify yet" };
}

/// A verification's steps, each with the colour its status is drawn in.
export function checkRows(verification) {
  const levels = { ok: "ok", warn: "warn", fail: "bad" };
  return ((verification && verification.checks) || []).map(c => ({
    name: c.name,
    level: levels[c.status] || "warn",
    status: c.status === "fail" ? "FAIL" : c.status,
    detail: c.detail || "",
  }));
}

/// What a failed fetch of `/api/backup` means: a daemon built before the
/// endpoint answers a bare 404 (`api()` turns it into "404 Not Found"),
/// which is a daemon to rebuild, not a fault to paint red.
export function backupFailure(error) {
  const message = error && error.message ? String(error.message) : String(error ?? "");
  return /^404\b/.test(message) ? "unavailable" : "error";
}

/// The events that change what the page shows.
export function isBackupEvent(ev) {
  return !!ev && typeof ev.type === "string" && ev.type.startsWith("backup_");
}
