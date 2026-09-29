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
///
/// `openTask` is the remediation task already open for that control in that
/// scope (`ScopePolicy.open_tasks`, `#98`), or `null` -- read off the report
/// so a reload still knows, not only the click that created it.
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
        openTask: openTaskFor(row, status.control),
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
/// `FINDING_LABELS` translates its own -- a kind this map has never heard
/// of (including `ambiguous_check_target`, left as its raw wire spelling)
/// falls back to the raw string rather than hiding it.
const FINDING_LABELS = {
  parse_failed: "Catalogue failed to parse",
  duplicate_control: "Duplicate control id",
  framework_mismatch: "Framework name does not match the file",
  unknown_maps_to: "maps_to names a control nothing defines",
  unknown_framework: "Names a framework with no loaded catalogue",
  unknown_control: "Names a control that is not applicable",
  loosening_has_no_effect: "A tighten did not shorten the freshness window",
  empty_rationale: "n/a declared with no rationale",
  unknown_daemon_fact: "Names a daemon fact nothing recognizes",
  unknown_secrets_location: "Names a secrets location nothing recognizes",
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
/// following a link. `daemon` points at L1 Infrastructure (merged as `#86`/
/// `#87`, the `infrastructure` page under level `infra`) now that it exists.
/// `attested` (`#158`) points at Tasks, same as `task` -- what closes the
/// gap is a run of the named category actually attesting the step, and
/// that is where runs live.
const REMEDIATION = {
  knowledge: { page: "knowledge", label: "Knowledge" },
  task: { page: "tasks", label: "Tasks" },
  workflow: { page: "workflows", label: "Workflows" },
  gate: { page: "benchmarks", label: "Benchmarks" },
  roles: { page: "roles", label: "Roles" },
  sandbox: { page: "sandboxes", label: "Sandboxes" },
  secrets: { page: "secrets", label: "Secrets" },
  dependencies: { page: "dependencies", label: "Dependencies" },
  daemon: { page: "infrastructure", label: "Infrastructure" },
  attested: { page: "tasks", label: "Tasks" },
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

/// A `dependencies` check's line: the declared SBOM's age limit, whether a
/// built SBOM is required, each severity's cap on open findings, and the cap
/// on exploited ones -- whichever it sets.
function describeDependencies(check) {
  const terms = [];
  if (check.sbom_max_age) terms.push(`SBOM max_age ${check.sbom_max_age}`);
  if (check.built_sbom) terms.push("built SBOM required");
  for (const [severity, limit] of Object.entries(check.max_open || {})) {
    terms.push(`${severity} <= ${limit}`);
  }
  if (check.exploited_open !== null && check.exploited_open !== undefined) {
    terms.push(`exploited <= ${check.exploited_open}`);
  }
  return `dependencies: ${terms.join(", ")}`;
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
      return check.absent && check.absent.length ? `secrets: absent ${check.absent.join(", ")}` : "secrets";
    case "daemon":
      return `daemon: ${check.fact}`;
    case "dependencies":
      return describeDependencies(check);
    case "attested":
      return `attested: ${check.category}/${check.step} (max_age ${check.max_age})`;
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

// ------------------------------------------------------------- remediation

/// Whether "Create task" belongs on a row or in the control-detail modal --
/// `open`/`stale` only. A `satisfied`/`attested` control has nothing to
/// remediate and `POST /api/policy/remediate` refuses it outright; `n/a`
/// has no gap to begin with. Kept as one pure predicate rather than an
/// inline `!==`/`!==` at each call site, so a status this file does not
/// know about yet (a future kind) is refused a button by default instead
/// of getting one by accident.
export function canRemediate(statusKind) {
  return statusKind === "open" || statusKind === "stale";
}

/// The body `POST /api/policy/remediate` expects.
export function remediateBody(control, scope) {
  return { control, scope };
}

/// The remediation task already open for `control` in one report row, by
/// the daemon's own `framework/id` key (`ScopePolicy.open_tasks`), or
/// `null`. Absent on the wire when nothing is open, hence the fallback.
export function openTaskFor(scopePolicy, control) {
  const map = (scopePolicy && scopePolicy.open_tasks) || {};
  return map[control] || null;
}

/// What stands where "Create task" would: nothing when the status has no
/// gap (`canRemediate`), a link to the task already open for it, or the
/// button. The daemon still refuses a second task on its own
/// (`policy_remediate`); this only saves a person the click that finds out.
export function remediateAction(statusKind, openTask) {
  if (!canRemediate(statusKind)) return null;
  return openTask ? { kind: "open", task: openTask } : { kind: "create" };
}

// ------------------------------------------------------------ refs' links

/// `app.js`'s router cuts a view's own tail at the first segment equal to
/// its `MODAL` marker ("task") to find an open task (`splitTail`/`tailOf`).
/// Not exported there, so this file keeps its own copy -- the same
/// duplication `bench-model.js`'s and `knowledge-graph.js`'s own `RESERVED`
/// already carry, and for the same reason (see either's header comment). A
/// task id is a UUID, never the literal word `task`, so unlike those two
/// this never needs their `~` escape -- there is no value here a person
/// could have named that.
const TASK_MODAL = "task";

/// A link that opens `taskId` (and `runId`, if given) the way `app.js`'s
/// own router builds that URL: the Tasks page, then the modal tail
/// `tailOf` appends when a task is open. A real `href`, not an `openTask`
/// click handler -- this file stays DOM-free (see the header comment), and
/// a plain link survives being copied, opened in a new tab, or read back
/// after a reload the same way every other route here does.
function taskModalHref(scope, taskId, runId) {
  const tail = [TASK_MODAL, taskId, ...(runId ? [runId] : [])].map(encodeURIComponent).join("/");
  return `${routeHref(scope, "tasks")}/${tail}`;
}

/// A link to one bench run, in the shape `benchmarksTail`/`readBenchmarksTail`
/// (`bench-model.js`) already read and write for the Benchmarks tab's own
/// "Runs" segment: `#<scope>/imp/benchmarks/runs/<id>`.
function benchRunHref(scope, runId) {
  return `${routeHref(scope, "benchmarks")}/runs/${encodeURIComponent(runId)}`;
}

/// A task-modal link for a bare task id -- one `remediate` just created, or
/// one `open_tasks` names -- built by `refLinks` itself so the two can never
/// drift apart (`quality-model.js` exports the same helper).
export function taskHref(scope, taskId) {
  return refLinks(scope, [{ kind: "task", id: taskId }])[0].href;
}

const REF_LABELS = { task: "Task", run: "Run", workflow_run: "Workflow run", bench_run: "Bench run" };

/// One entry per `refs` id worth a direct link, from `ControlStatus.refs`/
/// `PolicyControlDetail.refs` -- exact, from the id `EvidenceRef` already
/// carries, never parsed out of a reason string the way `closingLinks` has
/// to for the check kinds that carry no ref yet. `attestation` is left out
/// on purpose: the control-detail modal's own Attestations table already
/// shows that row in full, so a second link to the same place says nothing
/// a click there does not.
///
/// `task`/`run` open the task modal; `bench_run` deep-links to that run in
/// Benchmarks; `workflow_run` can only reach the Workflows view, not the
/// one run -- `workflowRouteTail` (`workflow-model.js`) needs the
/// *definition's* id to route at all, which no check here ever carries
/// (only the run's own id, `EvidenceRef::workflow_run`).
///
/// A `run` ref is paired with a task id only when `refs` carries exactly
/// one `task` ref -- more than one (a control whose own check and a
/// `maps_to` neighbour's check both name a task) makes which task a run
/// belongs to ambiguous from `refs` alone, and a guess is worse than no
/// link; that run then gets no link, not a wrong one.
export function refLinks(scope, refs) {
  const list = refs || [];
  const taskIds = list.filter((r) => r.kind === "task").map((r) => r.id);
  const soleTask = taskIds.length === 1 ? taskIds[0] : null;

  const links = [];
  for (const ref of list) {
    switch (ref.kind) {
      case "task":
        links.push({ kind: "task", id: ref.id, label: REF_LABELS.task, href: taskModalHref(scope, ref.id) });
        break;
      case "run":
        if (soleTask) links.push({ kind: "run", id: ref.id, label: REF_LABELS.run, href: taskModalHref(scope, soleTask, ref.id) });
        break;
      case "workflow_run":
        links.push({ kind: "workflow_run", id: ref.id, label: REF_LABELS.workflow_run, href: routeHref(scope, "workflows") });
        break;
      case "bench_run":
        links.push({ kind: "bench_run", id: ref.id, label: REF_LABELS.bench_run, href: benchRunHref(scope, ref.id) });
        break;
      default:
        break; // attestation, or a kind this build does not know yet.
    }
  }
  return links;
}
