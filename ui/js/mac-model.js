//! The L1 Mac tab's pure logic (#260): what `/api/host/power-mode`'s report
//! means for the page. Nothing here touches the DOM, so the Node tests
//! import it directly; `mac.js` only puts these answers on screen.
//!
//! The page is one of five states, decided here and nowhere else:
//!
//! - `unavailable` -- the daemon predates the endpoint (a bare 404): a calm
//!   note, not an error.
//! - `unsupported` -- not macOS, or a Mac whose `pmset -g cap` offers no
//!   energy mode: "not supported", and no control at all.
//! - `read-only`  -- the mode is shown, the control is drawn disabled, and the
//!   sudoers rule with its install command is shown: the one step that
//!   stays a person's.
//! - `editable`   -- the control is live; a click is the confirmation.
//! - `error`      -- the request itself failed.
//!
//! *Mixed* is not a sixth state but a reading inside the last two: AC and
//! battery disagree, so no segment is lit and both are shown.

/// The three modes, in the order the control draws them. `pmset` is the
/// number the daemon maps each to -- shown, never sent: the wire carries
/// only `mode`.
export const MODES = [
  { mode: "automatic", label: "Automatic", pmset: 0 },
  { mode: "high_performance", label: "High performance", pmset: 2 },
  { mode: "energy_saving", label: "Energy saving", pmset: 1 },
];

const LABELS = Object.fromEntries(MODES.map(m => [m.mode, m.label]));

/// A mode's name for a person; `--` for one this page does not know.
export function modeLabel(mode) {
  return LABELS[mode] || "--";
}

/// `"unavailable"` for a daemon too old to serve the endpoint (the API
/// answers a bare 404 with no envelope), `"error"` for anything else.
export function macFailure(error) {
  const message = error && error.message ? String(error.message) : String(error ?? "");
  return /^404\b/.test(message) ? "unavailable" : "error";
}

/// Which of the five states the page is in.
export function macState(report, { unavailable = false, error = null } = {}) {
  if (unavailable) return "unavailable";
  if (!report) return error ? "error" : "loading";
  if (!report.applicable || !(report.supported || []).length) return "unsupported";
  return report.can_change ? "editable" : "read-only";
}

/// AC and battery as the page reads them. `mixed` only when both are
/// reported and they disagree -- a Mac with no battery is never mixed.
/// `current` is the one mode to light, `null` when mixed or unread.
export function reading(report) {
  const ac = report ? report.ac ?? null : null;
  const battery = report ? report.battery ?? null : null;
  const mixed = ac !== null && battery !== null && ac !== battery;
  return { ac, battery, mixed, current: mixed ? null : (ac ?? battery) };
}

/// The headline: the mode, `Mixed`, `Not supported`, or `Unknown`.
export function headline(report) {
  const state = macState(report);
  if (state === "unsupported") return report && !report.applicable ? "Not applicable" : "Not supported";
  const { mixed, current } = reading(report);
  if (mixed) return "Mixed";
  return current ? modeLabel(current) : "Unknown";
}

/// Why the control looks the way it does, in one line.
export function stateText(report) {
  switch (macState(report)) {
    case "unsupported":
      return report && !report.applicable
        ? "The power mode is a macOS setting; this host is not macOS."
        : "This Mac offers no energy mode: pmset -g cap lists neither lowpowermode nor highpowermode.";
    case "read-only":
      return "Read-only: Factory may not change the power mode until the sudoers rule below is installed.";
    case "editable":
      return "Choosing a mode sets it at once for every power source (pmset -a powermode).";
    default:
      return "";
  }
}

/// The source rows: AC, and battery when this Mac has one.
export function sourceRows(report) {
  const { ac, battery } = reading(report);
  const rows = [{ source: "AC power", mode: ac, label: modeLabel(ac) }];
  if (battery !== null) rows.push({ source: "Battery", mode: battery, label: modeLabel(battery) });
  return rows;
}

/// One segment per mode, `null` when there should be no control at all.
/// A segment is lit when it is the current mode (never while mixed), and
/// enabled only when the page is editable, the host offers the mode, the
/// rule permits it, and nothing is already being set.
export function segments(report, { busy = null } = {}) {
  const state = macState(report);
  if (state !== "editable" && state !== "read-only") return null;
  const { current } = reading(report);
  const supported = report.supported || [];
  const permitted = report.permitted || [];
  return MODES.map(({ mode, label, pmset }) => {
    let why = "";
    if (!supported.includes(mode)) why = "this Mac does not offer it";
    else if (!permitted.includes(mode)) why = "the sudoers rule is not installed";
    else if (busy) why = `setting ${modeLabel(busy)}…`;
    return {
      mode,
      label,
      on: current === mode,
      disabled: why !== "",
      title: why || `pmset -a powermode ${pmset}`,
    };
  });
}

/// The body `POST /api/host/power-mode` takes. Refuses anything but one of
/// the three names, so the page never even sends one.
export function setBody(mode) {
  if (!LABELS[mode]) throw new Error(`${JSON.stringify(mode)} is not a power mode`);
  return JSON.stringify({ mode });
}

/// Whether the rule and its install command belong on the page: whenever
/// the host offers a mode the rule does not yet permit.
export function needsRule(report) {
  return macState(report) === "read-only";
}

/// The install as steps a person runs one at a time, `{ text, command }`,
/// each command in a block of its own -- the same wording as the daemon's
/// `SudoersRule::install_steps`, which the CLI prints. Three cases:
///
/// - the daemon's own user is not an administrator and one is known:
///   `su - <admin>` *alone*, then the install pasted in that shell, then
///   `exit`. Pasted together, `su`'s password prompt swallows the typed-ahead
///   lines and opens a shell that ran nothing, with no error;
/// - the daemon's own user is an administrator: today's one step;
/// - no administrator was found: the same step, "from an administrator
///   account".
///
/// Backticks in `text` mark code; `mac.js` draws them as `<code>`.
export function installSteps(sudoers) {
  const s = sudoers || {};
  const admins = Array.isArray(s.admins) ? s.admins.filter(a => typeof a === "string" && a) : [];
  const checked = "This checks the rule with `visudo -cf` before installing it, root-owned and read-only";
  if (!s.user_is_admin && admins.length) {
    const admin = admins[0];
    return [
      { text: `Run this alone and enter ${admin}'s password:`, command: `su - ${admin}` },
      { text: "Then, in that shell, paste:", command: s.install || "" },
      { text: "Then `exit`, and Refresh.", command: null },
    ];
  }
  if (s.user_is_admin) return [{ text: `${checked}:`, command: s.install || "" }];
  return [{ text: `${checked}. Run it from an administrator account:`, command: s.install || "" }];
}

/// Whether the install starts by switching to another account -- the
/// separate `su` step, after which the whole-configuration check line is
/// left out (pasted after the install, a `sudo` prompt would swallow it).
export function switchesAccount(sudoers) {
  return installSteps(sudoers).length > 1;
}

/// The journaled changes, newest first, as `{ at, by, text }`.
export function changeRows(report) {
  return ((report && report.changes) || [])
    .slice()
    .reverse()
    .map(c => ({ at: c.at, by: c.by, text: c.message || `${modeLabel(c.to)} by ${c.by}` }));
}
