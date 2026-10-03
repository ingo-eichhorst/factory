//! Pure shaping for the CRA Art. 14 reporting clock (`#157` phase 1,
//! `#170` phase 2): turns a `ReportingClock` (`factory_core::reporting_clock`,
//! `Payload::PolicyClock` in `protocol.rs`, read from `GET /api/policy/clock`)
//! into rows Intake, the Inbox and the L6 Policy tab each draw in their own
//! idiom. Only the 24h early warning and 72h notification exist here -- the
//! 14-day final report is `#157` phases 2-3, whose anchor (the
//! corrective-measure time) has no source yet; it is not invented here.
//!
//! Nothing here decides a deadline's `due`/`overdue`/`met`/`late` state --
//! that is the daemon's, computed once by `reporting_clock::compute` and
//! read verbatim (`item.deadlines[].state`); this file only formats it and,
//! for the Inbox alone, decides how soon "soon" is (a presentation choice,
//! not a deadline computation -- see `DUE_SOON_SECONDS` below). Split
//! dedupe is `#211`'s own, inside `compute` itself: a split chain is
//! already folded to its root before this file ever sees it.
//!
//! No DOM and no fetch -- the same boundary `policy-model.js`/
//! `intake-model.js` keep. `intake.js`, `dashboard.js` and `policy.js` are
//! the only modules that touch `document` or call `api()`, and each reads
//! the clock as its own optional extra: a failed or missing read degrades
//! to "nothing here", never a blank page (`clockRows`/`inboxClockRows`
//! both return `[]` for a `null`/`undefined` clock).

// ------------------------------------------------------------------ items

/// Mirrors `ClockItemRef`'s `Display` (`reporting_clock.rs`): the CLI's own
/// text form, and this file's own map key -- `item` is the wire's tagged
/// enum, `{kind:"finding",scope,vulnerability}` or `{kind:"report",item}`.
export function itemKey(item) {
  return item.kind === "finding" ? `finding:${item.scope}:${item.vulnerability}` : `report:${item.item}`;
}

/// Every clock item, keyed by `itemKey`, over its raw (unshaped) wire form --
/// what `reportDeadlines` looks a report up in without scanning `clock.items`
/// itself. `clock` may be `null`/`undefined` (not fetched, or the read
/// failed): an empty map, same as no clock item ever matching.
export function clockIndex(clock) {
  const map = new Map();
  for (const item of clock?.items || []) map.set(itemKey(item.item), item);
  return map;
}

// -------------------------------------------------------------- vocabulary

export const DEADLINE_LABELS = { early_warning: "24h early warning", notification: "72h notification" };
export function deadlineLabel(kind) {
  return DEADLINE_LABELS[kind] || kind;
}

/// `due`/`overdue`/`met`/`late` (`ClockDeadlineState`), as a badge: `due` is
/// neutral -- nothing is wrong yet, the same reading `.s-stale` gives an
/// aging-but-not-broken control -- `overdue` is a fault, `met` reuses
/// Quality's own `.s-met` (`#107`, the same word and meaning: on time), and
/// `late` is a miss that was eventually put right, drawn like a warning
/// rather than a fire.
const STATE_LABELS = { due: "due", overdue: "overdue", met: "met", late: "met late" };
export function stateLabel(state) {
  return STATE_LABELS[state] || state;
}

// ------------------------------------------------------------------- time

/// Seconds as a short span -- the same steps `operations-model.js`'s
/// `fmtAge` takes (this file draws the same idiom, just for a deadline
/// ahead of now as often as one behind it).
export function fmtSpan(seconds) {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s}s`;
  if (s < 5400) return `${Math.round(s / 60)}m`;
  if (s < 36 * 3600) return `${(s / 3600).toFixed(s < 36000 ? 1 : 0)}h`;
  return `${(s / 86400).toFixed(s < 10 * 86400 ? 1 : 0)}d`;
}

/// The clock's own `now` (`ReportingClock.now`, an absolute instant, unlike
/// the age-in-seconds fields `operations-model.js` carries), advanced by how
/// long ago this answer arrived here -- the same `serverNow` rule
/// (`operations-model.js`): never the browser's clock minus the server's
/// timestamp, so a countdown never drifts from what the daemon actually
/// computed the states against. `clock` must be truthy -- every caller
/// already only reaches this once it has a real clock to show (`clockRows`/
/// `inboxClockRows` return `[]` first, `renderClockSection`'s own ternary
/// guards it); there is no "no clock" reading to fall back to.
export function nowFromClock(clock, elapsedS = 0) {
  return new Date(Date.parse(clock.now) + Math.max(0, elapsedS || 0) * 1000).toISOString();
}

/// "due in 3h" / "overdue by 3h" / "met, 2h to spare" / "met, 2h late" --
/// read against `nowIso`. `state` always wins over this text: a `met`
/// deadline with a submission just after its `due_at` still reads "met, ...
/// late" here (a compliance fact worth keeping visible), never re-derived
/// as `late` -- only the daemon's own `state` decides that word.
export function dueText(deadline, nowIso) {
  const due = Date.parse(deadline.due_at);
  if (deadline.submission) {
    const at = Date.parse(deadline.submission.at);
    return at <= due ? `met, ${fmtSpan((due - at) / 1000)} to spare` : `met, ${fmtSpan((at - due) / 1000)} late`;
  }
  const now = Date.parse(nowIso);
  return now <= due ? `due in ${fmtSpan((due - now) / 1000)}` : `overdue by ${fmtSpan((now - due) / 1000)}`;
}

// ------------------------------------------------------------------- rows

/// One deadline, shaped for drawing: `state` unchanged from the daemon,
/// this file's own label for it, the due time and `dueText` against
/// `nowIso`, and the submission (who, when), if it has one.
export function deadlineRow(deadline, nowIso) {
  return {
    deadline: deadline.deadline,
    label: deadlineLabel(deadline.deadline),
    state: deadline.state,
    stateLabel: stateLabel(deadline.state),
    dueAt: deadline.due_at,
    text: dueText(deadline, nowIso),
    submission: deadline.submission || null,
  };
}

/// One clock item, shaped: its own key and kind, the raw item ref (a
/// finding's `scope`/`vulnerability`, or a report's `item` -- a task id),
/// scope, awareness time, whether it is excluded (a finding only, `null`
/// for a report) or still reported by the newest scan, and its deadlines --
/// empty for an excluded item, the same as the wire.
export function itemRow(item, nowIso) {
  return {
    key: itemKey(item.item),
    kind: item.item.kind,
    ref: item.item,
    scope: item.scope,
    awarenessAt: item.awareness_at,
    excluded: item.excluded || null,
    reportedNow: item.reported_now,
    deadlines: (item.deadlines || []).map((d) => deadlineRow(d, nowIso)),
  };
}

/// Every item, in the clock's own order (already awareness time then
/// identity, `reporting_clock::compute`) -- the Policy tab's own table
/// order. `[]` for a `null`/`undefined` clock (not fetched, or the read
/// failed): the tab still renders, with nothing in this section.
export function clockRows(clock, nowIso) {
  if (!clock) return [];
  return (clock.items || []).map((item) => itemRow(item, nowIso));
}

// --------------------------------------------------------------- intake

/// A confirmed security report's own deadlines, for the Intake card and item
/// modal: `report:<card.id>` if the clock carries it directly, else
/// `report:<card.parent>` -- a split chain's root, one hop up, as far as a
/// board's own cards resolve on their own. `#211`'s clock already folds a
/// deeper chain to its true root inside `compute`; a card more than one
/// split away from that root shows nothing here rather than walking the
/// chain again client-side (`clockIdx` only ever holds root items). `null`
/// when neither matches -- an unconfirmed flag, a report split more than
/// once from its root, or the clock not read (`clockIdx` empty).
export function reportDeadlines(clockIdx, card, nowIso) {
  if (!clockIdx || !card) return null;
  const item = clockIdx.get(`report:${card.id}`) || (card.parent ? clockIdx.get(`report:${card.parent}`) : null);
  return item ? itemRow(item, nowIso) : null;
}

// ---------------------------------------------------------------- inbox

/// How soon a `due` deadline counts as "coming up" for the Inbox. The
/// Inbox is only ever the daemon's own words for what needs a person
/// (`operations-model.js`'s `inboxItems`, read off `OperationsReport`); this
/// is the one judgment this file adds on top of the clock's own `state`,
/// and it is a presentation threshold, not a deadline computation: it never
/// overrides `overdue` (always shown) or `met`/`late` (never shown --
/// nothing left for a person to do), only how wide the `due` window is
/// before it is worth surfacing here. Six hours: short enough that a quiet
/// day never shows a 24-hours-out early warning, long enough that one due
/// by the next working day is not a surprise.
export const DUE_SOON_SECONDS = 6 * 3600;

/// One clock deadline as an Inbox row, in `attentionRows`' own shape
/// (`operations-model.js`) so `inboxItemRow` (`dashboard.js`) draws it
/// unchanged: `tone` (`fault`/`wait`, the same tokens `SEVERITY_TONE`
/// already uses), `age` (the trailing duration `inboxItemRow` draws with
/// `fmtAge` in `.it-age` -- how long overdue, or how long until due, the
/// same number `reason`'s own `dueText` already spells out in `.sub`, here
/// alone in the corner), `title`, `label`, `reason`, `scope`, and either
/// `task_id` (a report -- opens the task, the row's existing click) or
/// `href` (a finding -- no task of its own; `hrefOf(scope)` names where it
/// opens instead, e.g. Dependencies). `titleOf(taskId)` reads a report's
/// own title (`state.tasks` is a report's task); kept a callback so this
/// file never touches `state` itself.
function inboxClockRow(item, deadline, now, titleOf, hrefOf) {
  // Positive once the deadline has passed; `state` alone still decides
  // overdue vs. due -- this is only used for the displayed magnitude and
  // for the presentation's due-soon window.
  const deltaS = (Date.parse(now) - Date.parse(deadline.dueAt)) / 1000;
  const overdue = deadline.state === "overdue";
  const dueSoon = deadline.state === "due" && deltaS >= -DUE_SOON_SECONDS;
  if (!overdue && !dueSoon) return null;
  const isReport = item.kind === "report";
  return {
    key: `clock:${item.key}:${deadline.deadline}`,
    title: isReport ? titleOf(item.ref.item) : item.ref.vulnerability,
    label: `${deadline.label}, ${overdue ? "overdue" : "due soon"}`,
    reason: deadline.text,
    tone: overdue ? "fault" : "wait",
    age: Math.abs(deltaS),
    scope: item.scope,
    task_id: isReport ? item.ref.item : null,
    href: isReport ? null : hrefOf(item.scope),
  };
}

function compareInboxClockRows(a, b) {
  // Overdue first (a fault always outranks a warning) -- the longest
  // overdue, then the soonest due, within each.
  if (a.tone !== b.tone) return a.tone === "fault" ? -1 : 1;
  return a.tone === "fault" ? b.age - a.age : a.age - b.age;
}

export function inboxClockRows(clock, elapsedS, titleOf, hrefOf) {
  if (!clock) return [];
  const now = nowFromClock(clock, elapsedS);
  const rows = [];
  for (const item of clockRows(clock, now)) {
    if (item.excluded) continue;
    for (const deadline of item.deadlines) {
      const row = inboxClockRow(item, deadline, now, titleOf, hrefOf);
      if (row) rows.push(row);
    }
  }
  return rows.sort(compareInboxClockRows);
}
