//! Pure shaping logic for the L6 Policy tab -- everything that turns a
//! `PolicyReport`/`PolicyControlDetail` (see `factory_core::policy` and
//! `Payload::Policy`/`Payload::PolicyControl` in `protocol.rs`) into rows a
//! table can draw, plus the links to the level that can close a gap.
//!
//! Nothing here touches `document`: `policy.js` is the only module that
//! draws anything, so this one is safe to import in a Node test with no DOM
//! at all -- the same boundary `bench-model.js`/`knowledge-graph.js` already
//! keep. It does import `routeHref` from `scopes.js` and `nodeTail` from
//! `knowledge-graph.js`; neither touches `document` either (`bench-model.js`
//! already imports `inScope` from the same `scopes.js` for the same reason).
//!
//! Two wire shapes that look alike are not: `ControlStatus` (inside
//! `PolicyReport.rows[].statuses`) flattens its `Status` -- `status` and
//! `reasons` are top-level fields, because `protocol.rs` marks that field
//! `#[serde(flatten)]`. `PolicyControlDetail.status` is **not** flattened --
//! it is nested, `detail.status.status` / `detail.status.reasons`. Confirmed
//! against a running daemon (`GET /api/policy`, `GET /api/policy/controls/…`),
//! not just read off the Rust. `statusOf` below normalises both.

import { routeHref } from "./scopes.js";
import { nodeTail } from "./knowledge-graph.js";

// ------------------------------------------------------------------ status

/// `ControlStatus` (flattened: `{control, title, kind, status, reasons}`) or
/// `PolicyControlDetail.status` (nested: `{status, reasons}`) -- either way,
/// the `{status, reasons}` pair a badge and a reasons list need.
export function statusOf(x) {
  if (!x) return { status: "open", reasons: [] };
  if (typeof x.status === "object" && x.status) return x.status;
  return { status: x.status, reasons: x.reasons || [] };
}

/// The wire spells the fifth status `not_applicable`; every label on this
/// page says `n/a` instead, the shorter word the ADR itself uses.
export function statusLabel(kind) {
  return kind === "not_applicable" ? "n/a" : kind;
}

// --------------------------------------------------------------- catalogues

/// `Kind` is written `best-practice` (a hyphen, matching the ADR's own
/// catalogue spelling) on a control or catalogue; `FrameworkRollup` counts
/// the same thing under the field `best_practice` (an underscore, an
/// ordinary Rust identifier). Two spellings of one word, both real.
const KIND_LABELS = { regulation: "Regulation", standard: "Standard", "best-practice": "Best practice" };
export function kindLabel(kind) {
  return KIND_LABELS[kind] || kind;
}

export function frameworkTitle(catalogues, framework) {
  const c = (catalogues || []).find((c) => c.framework === framework);
  return c ? c.title : framework;
}

function countsTotal(counts) {
  if (!counts) return 0;
  return (counts.satisfied || 0) + (counts.attested || 0) + (counts.stale || 0) + (counts.open || 0) + (counts.not_applicable || 0);
}

/// One card per framework in `report.rollup` -- already the whole subtree's
/// worst-across-scopes aggregate (`policy::worst_across_scopes`, computed by
/// the daemon), joined with `report.catalogues` for the title and framework
/// `kind` a rollup alone does not carry. Framework order is whatever the
/// server sent (`BTreeMap`, so already alphabetical); this never re-sorts it.
export function frameworkCards(report) {
  const catalogues = report.catalogues || [];
  return (report.rollup || []).map((r) => {
    const cat = catalogues.find((c) => c.framework === r.framework);
    return {
      framework: r.framework,
      title: cat ? cat.title : r.framework,
      kind: cat ? cat.kind : null,
      counts: r.counts,
      bestPractice: r.best_practice,
      countedTotal: countsTotal(r.counts),
      bestPracticeTotal: countsTotal(r.best_practice),
      compliant: r.compliant,
    };
  });
}

// --------------------------------------------------------------------- gaps

/// One row per `(scope, control)` where that scope's own evaluation is
/// `open` or `stale` -- not `policy::worst_across_scopes`, which the CLI's
/// status board uses for its OPEN/STALE sections: that folds every scope
/// into one row per control and loses which scope has the gap, and both the
/// remediation link and `GET /api/policy/controls/…` need a concrete scope
/// (unlike `GET /api/policy`, its `scope` query is required). Showing one
/// row per scope also says plainly when a control is open in three project
/// scopes and satisfied at the root -- three places to go fix it, not one.
export function gapRows(report) {
  const rows = [];
  for (const row of report.rows || []) {
    for (const status of row.statuses || []) {
      const s = statusOf(status);
      if (s.status !== "open" && s.status !== "stale") continue;
      rows.push({
        scope: row.scope,
        control: status.control,
        title: status.title,
        kind: status.kind,
        status: s.status,
        reasons: s.reasons,
      });
    }
  }
  rows.sort((a, b) => a.control.localeCompare(b.control) || a.scope.localeCompare(b.scope));
  return rows;
}

/// Every control marked `n/a` somewhere in the asked-about subtree, with the
/// title `report.not_applicable` itself does not carry -- `NotApplicableEntry`
/// is `{control, scope, rationale}` alone, so the title is recovered from
/// any row's own `ControlStatus` for that control (present in every row,
/// whatever its status, `n/a` included).
export function notApplicableRows(report) {
  const titles = new Map();
  for (const row of report.rows || []) {
    for (const status of row.statuses || []) {
      if (!titles.has(status.control)) titles.set(status.control, status.title);
    }
  }
  return (report.not_applicable || [])
    .map((na) => ({
      control: na.control,
      title: titles.get(na.control) || na.control,
      scope: na.scope,
      rationale: na.rationale,
    }))
    .sort((a, b) => a.control.localeCompare(b.control) || a.scope.localeCompare(b.scope));
}

// ---------------------------------------------------------------- findings

/// The wire's `FindingKind` values (`#[serde(rename_all = "snake_case")]` in
/// `factory_core::policy`), translated the way `knowledge.js`'s
/// `FINDING_LABELS` translates its own seven -- a kind this map has never
/// heard of falls back to the raw string rather than hiding it.
const FINDING_LABELS = {
  parse_failed: "Catalogue failed to parse",
  duplicate_control: "Duplicate control id",
  framework_mismatch: "Framework name does not match the file",
  unknown_maps_to: "maps_to names a control nothing defines",
  unknown_framework: "Names a framework with no loaded catalogue",
  unknown_control: "Names a control that is not applicable",
  loosening_has_no_effect: "A tighten did not shorten the freshness window",
  empty_rationale: "n/a declared with no rationale",
};
export function findingLabel(kind) {
  return FINDING_LABELS[kind] || kind;
}

export function findingsByKind(findings) {
  const groups = new Map();
  for (const f of findings || []) {
    if (!groups.has(f.kind)) groups.set(f.kind, []);
    groups.get(f.kind).push(f);
  }
  return groups;
}

// --------------------------------------------------------- closing the gap

/// `<root>/<id>` is `ControlRef::default_tag`'s own construction --
/// `format!("control/{}/{}", framework, id)` -- and `control` here is
/// already the wire's `framework/id` string, so this is exactly that
/// concatenation, never re-derived from two separate fields.
export function defaultKnowledgeTag(control) {
  return `control/${control}`;
}

/// A `knowledge: tag \`x\` …` reason always carries the resolved tag --
/// default or explicit -- inside backticks (`direct_status` in `policy.rs`
/// formats it that way whichever it was), so this never needs the control's
/// own default separately.
function knowledgeTagFromReason(reason) {
  const m = /^knowledge: tag `([^`]+)`/.exec(reason || "");
  return m ? m[1] : null;
}

/// Which check kind a status's own reason strings mention. `ControlStatus`
/// carries no `Check` list of its own -- only `PolicyControlDetail.checks`
/// does -- so this is the one signal the summary table has: `direct_status`
/// in `factory_core::policy` prefixes every reason with `Check::kind_name()`
/// followed by `: ` (`"knowledge: tag …"`, `"task: unevaluated"`, and so on),
/// except a `maps_to` reason (`"satisfied via cra/x (maps_to)"`), which this
/// regex does not match and is deliberately not linked anywhere. Coupled to
/// that exact string shape on purpose -- `ui/tests/policy-model.test.js`
/// pins it against the real reason strings `direct_status` writes, so a
/// wording change there fails a test here rather than silently going quiet.
const REASON_KIND_RE = /^([a-z]+):\s/;
export function reasonCheckKinds(reasons) {
  const kinds = [];
  for (const r of reasons || []) {
    const m = REASON_KIND_RE.exec(r);
    if (m && !kinds.includes(m[1])) kinds.push(m[1]);
  }
  return kinds;
}

/// Where each check kind's own gap is closed. `attestation` is left out on
/// purpose -- it is closed right here, with the Attest form below, not by
/// following a link -- and so is `daemon`, since L1 Infrastructure is not
/// built yet (`#87` is a different tab; this one stays inert until it is).
const REMEDIATION = {
  knowledge: { page: "knowledge", label: "Knowledge" },
  task: { page: "tasks", label: "Tasks" },
  workflow: { page: "workflows", label: "Workflows" },
  gate: { page: "benchmarks", label: "Benchmarks" },
  roles: { page: "roles", label: "Roles" },
  sandbox: { page: "sandboxes", label: "Sandboxes" },
  secrets: { page: "secrets", label: "Secrets" },
};

/// A link to the exact tag node in the Knowledge graph, so following it lands
/// on the one page missing rather than just the tab -- `routeHref` has no
/// tail parameter, so this appends `nodeTail`'s own segments (from
/// `knowledge-graph.js`, which `knowledge.js`'s `readKnowledgeTail` reads the
/// same way a typed or reloaded link would) the way `knowledgeTail`/
/// `readKnowledgeTail` themselves already read and write that hash.
function knowledgeHref(scope, tag) {
  const tail = nodeTail(`tag:${tag}`).map(encodeURIComponent).join("/");
  return `${routeHref(scope, "knowledge")}/${tail}`;
}

/// The gap table's own links, from a row's reasons alone (see
/// `reasonCheckKinds`). One link per recognised check kind the reasons
/// mention, `knowledge` pointed at the exact tag rather than just the tab.
export function closingLinks(scope, reasons) {
  const links = [];
  for (const kind of reasonCheckKinds(reasons)) {
    if (kind === "knowledge") {
      const tag = reasons.map(knowledgeTagFromReason).find(Boolean);
      links.push({ label: "Knowledge", href: tag ? knowledgeHref(scope, tag) : routeHref(scope, "knowledge") });
      continue;
    }
    const target = REMEDIATION[kind];
    if (target) links.push({ label: target.label, href: routeHref(scope, target.page) });
  }
  return links;
}

/// The control detail modal's own per-check link -- exact, from
/// `PolicyControlDetail.checks`' own `Check` values, never parsed out of a
/// reason string the way `closingLinks` has to.
export function checkTarget(scope, control, check) {
  if (check.check === "knowledge") {
    const tag = check.tag || defaultKnowledgeTag(control);
    return { label: "Knowledge", href: knowledgeHref(scope, tag) };
  }
  const target = REMEDIATION[check.check];
  return target ? { label: target.label, href: routeHref(scope, target.page) } : null;
}

/// The evidence list's own line for one check -- `describe_check` in
/// `factory-cli/src/main.rs`, ported rather than duplicated by accident: the
/// CLI's `factory policy show` and this page should read the same check the
/// same way.
export function describeCheck(check) {
  switch (check.check) {
    case "knowledge":
      return check.tag ? `knowledge: tag \`${check.tag}\`` : "knowledge: default tag";
    case "attestation":
      return "attestation";
    case "task":
      return `task ${check.task}${check.max_age ? ` (max_age ${check.max_age})` : ""}`;
    case "workflow":
      return `workflow ${check.workflow}${check.max_age ? ` (max_age ${check.max_age})` : ""}`;
    case "gate":
      return `gate ${check.dataset}${check.case ? `/${check.case}` : ""}${check.max_age ? ` (max_age ${check.max_age})` : ""}`;
    case "roles":
      return `roles: forbid ${(check.forbid || []).join(", ") || "(nothing named)"}`;
    case "sandbox":
      return "sandbox";
    case "secrets":
      return "secrets";
    case "daemon":
      return `daemon: ${check.fact}`;
    default:
      return check.check;
  }
}

// -------------------------------------------------------------- attestation

/// `30d`/`12w`, a bare date, or a full RFC3339 timestamp -- `policy::Duration`'s
/// own grammar plus the two absolute forms `policy::parse_expiry` accepts. A
/// soft check only: the server is the one place that grammar is parsed for
/// real (`#78`'s CLI and this page's `POST` both hand it the raw string), so
/// this never blocks a submission on its own -- see `attestBody`, which does
/// not call it.
const EXPIRY_DURATION_RE = /^\d+[hdw]$/;
const EXPIRY_DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
export function looksLikeExpiry(text) {
  const t = String(text ?? "").trim();
  if (!t) return false;
  if (EXPIRY_DURATION_RE.test(t) || EXPIRY_DATE_RE.test(t)) return true;
  return !Number.isNaN(Date.parse(t));
}

/// The body `POST /api/policy/attestations` expects. Throws a sentence a
/// person can act on for the one thing worth catching before the round
/// trip -- an empty required field; the expiry grammar itself is the
/// server's alone to refuse, the same restraint `roleDefinePayload` shows
/// for a role name pattern the daemon also re-checks.
export function attestBody(control, scope, values) {
  const evidence = String(values.evidence ?? "").trim();
  if (!evidence) throw new Error("Evidence -- a pointer to what shows this is met -- is required.");
  const expires = String(values.expires ?? "").trim();
  if (!expires) throw new Error("Expiry is required: 30d, 12w, a date, or a full timestamp.");
  const note = String(values.note ?? "").trim();
  const body = { control, scope, evidence, expires };
  if (note) body.note = note;
  return body;
}
