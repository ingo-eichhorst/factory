// A task's schedule, between the wire and the task form. Pure, so it can be
// tested without a page.
//
// On the wire a cron schedule with no timezone is the bare expression,
// `{cron: "0 7 * * 1"}`, exactly as before timezones existed; one that names
// a timezone is `{cron: {expr, timezone}}`. Both are read here, and the bare
// form is the one written whenever there is no timezone to carry.

function cronParts(s) {
  if (!s || !s.cron) return null;
  if (typeof s.cron === "string") return { expr: s.cron, timezone: "" };
  return { expr: s.cron.expr || "", timezone: s.cron.timezone || "" };
}

/// What goes back in the form's schedule box.
export function scheduleText(s) {
  if (!s) return "";
  const cron = cronParts(s);
  if (cron) return cron.expr;
  if (s.every) return `every ${s.every.seconds}s`;
  return "";
}

/// What goes back in the form's timezone box. Blank is UTC.
export function scheduleZone(s) {
  return cronParts(s)?.timezone || "";
}

/// How a schedule reads in a list: `cron 0 9 * * 1 (Europe/Berlin)`.
export function scheduleLabel(s) {
  if (!s) return "manual";
  const cron = cronParts(s);
  if (cron) return cron.timezone ? `cron ${cron.expr} (${cron.timezone})` : `cron ${cron.expr}`;
  if (s.every) return `every ${s.every.seconds}s`;
  return "scheduled";
}

/// The form's two boxes back into a schedule. Whether the timezone is a
/// real one is the daemon's to say, where every way of setting a schedule
/// is checked alike; an interval with one is refused here, because no
/// timezone changes an interval and the box would only mislead.
export function parseSchedule(text, zone = "") {
  const v = text.trim();
  const timezone = (zone || "").trim();
  if (!v) {
    if (timezone) throw new Error("A timezone needs a cron schedule to go with it.");
    return null;
  }
  const m = v.match(/^every\s+(\d+)\s*(s|sec|secs|seconds|m|min|mins|minutes|h|hrs?|hours?)?$/i);
  if (m) {
    if (timezone) throw new Error("An `every` schedule is an interval, and no timezone changes it; leave the timezone blank.");
    const n = parseInt(m[1], 10);
    const u = (m[2] || "s").toLowerCase();
    const mult = u.startsWith("h") ? 3600 : (u.startsWith("m") ? 60 : 1);
    return { every: { seconds: n * mult } };
  }
  const expr = v.replace(/^cron\s+/i, "");
  return timezone ? { cron: { expr, timezone } } : { cron: expr };
}
