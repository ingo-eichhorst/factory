//! The L2 Secrets tab's declared catalogue (#244), pure: how an entry's
//! presence, resolution and expiry read, which rows a scope selection keeps,
//! and the one request body the edit form sends. No DOM, no fetch -- and no
//! value: a row carries metadata and yes/no answers, nothing a source gave.

/// The tag class for an expiry state.
export function expiryClass(state) {
  if (state === "expired") return "bad";
  if (state === "due_soon") return "warn";
  if (state === "ok") return "ok";
  return "";
}

/// `expires 2027-10-04 (365 days)`, `expires today (2026-10-04)`,
/// `expired on 2026-10-01 (3 days ago)`, `never`, `unknown`.
export function expiryText(row) {
  if (!row?.expires) return "unknown";
  if (row.expires === "never") return "never";
  const left = row.days_left;
  if (typeof left !== "number") return `expires ${row.expires}`;
  if (left < 0) return `expired on ${row.expires} (${-left} ${left === -1 ? "day" : "days"} ago)`;
  if (left === 0) return `expires today (${row.expires})`;
  return `expires ${row.expires} (${left} ${left === 1 ? "day" : "days"})`;
}

/// The words for an expiry state, as the tag shows them.
export function stateText(state) {
  return state === "due_soon" ? "due soon" : (state || "unknown");
}

/// For a file: whether it is there and owner-only. Other sources have no
/// presence of their own -- whether they resolve is the whole answer.
export function presenceText(row) {
  if (row.present === undefined || row.present === null) return { text: "n/a", cls: "" };
  if (!row.present) return { text: "absent", cls: "bad" };
  if (row.owner_only === false) return { text: "present, readable by others", cls: "bad" };
  return { text: "present, owner-only", cls: "ok" };
}

/// Whether the source gave a value at the provisioner's last pass -- for a
/// command, it ran; for an env or Keychain item, it was found. Never what.
export function resolutionText(row) {
  if (row.resolves === undefined || row.resolves === null) return { text: "not checked yet", cls: "" };
  if (row.resolves) return { text: "resolves", cls: "ok" };
  return { text: row.reason ? `does not resolve: ${row.reason}` : "does not resolve", cls: "bad" };
}

/// `awesome-herdr / awesome-herdr-curator / factory-claude-1a2b3c4d`.
export function useText(use) {
  return `${use.scope} / ${use.agent} / ${use.provider}`;
}

/// The catalogue is the instance's, not a scope's: an entry survives every
/// selection, the way a credential in the owner's home does. An inline
/// credential belongs to the scope that writes it.
export function visibleUndeclared(rows, contains) {
  return (rows || []).filter(row => contains(row.scope));
}

/// What the edit form starts from.
export function formValues(row) {
  const expires = row?.expires;
  return {
    never: expires === "never",
    date: expires && expires !== "never" ? expires : "",
    renew: row?.renew || "",
    note: row?.note || "",
  };
}

/// The `PUT /api/secrets/<name>` body: the three metadata fields, replaced
/// whole -- an emptied field is `null`, which removes it. There is no field
/// a value could travel in.
export function metadataBody(form) {
  const text = s => {
    const t = String(s || "").trim();
    return t === "" ? null : t;
  };
  let expires = null;
  if (form.never) expires = "never";
  else if (text(form.date)) expires = text(form.date);
  return { expires, renew: text(form.renew), note: text(form.note) };
}

/// A date the form can send: `YYYY-MM-DD`, or nothing.
export function validDate(value) {
  const v = String(value || "").trim();
  if (v === "") return true;
  if (!/^\d{4}-\d{2}-\d{2}$/.test(v)) return false;
  const d = new Date(`${v}T00:00:00Z`);
  return !Number.isNaN(d.getTime()) && d.toISOString().slice(0, 10) === v;
}
