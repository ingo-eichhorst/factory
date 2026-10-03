import test from "node:test";
import assert from "node:assert/strict";

import {
  attestBody,
  canRemediate,
  checkTarget,
  closingLinks,
  defaultKnowledgeTag,
  describeCheck,
  findingLabel,
  findingsByKind,
  frameworkCards,
  frameworkTitle,
  gapRows,
  kindLabel,
  looksLikeExpiry,
  notApplicableRows,
  openTaskFor,
  reasonCheckKinds,
  refLinks,
  remediateAction,
  remediateBody,
  statusLabel,
  statusOf,
  taskHref,
} from "../js/policy-model.js";

// A trimmed version of a real `GET /api/policy?scope=demo` answer, captured
// from a throwaway daemon running `examples/policies/cra.yaml` with the root
// declaring `policies: {frameworks: [cra]}` and `demo` tightening
// `cra/annex-i-2-1` to `14d` and marking `cra/annex-i-2-4` not applicable --
// not a shape invented by reading the Rust, but the JSON the daemon actually
// sent (`.report`, after `api()` unwraps `{status, data}`).
const report = {
  scope: "demo",
  rows: [
    {
      scope: "demo",
      statuses: [
        {
          control: "cra/annex-i-2-1",
          title: "Identify and document components (SBOM)",
          kind: "regulation",
          status: "open",
          reasons: [
            "knowledge: tag `control/cra/annex-i-2-1` not found",
            "attestation: none recorded",
            "task: unevaluated",
          ],
        },
        {
          control: "cra/annex-i-2-2",
          title: "Vulnerability handling procedure is documented",
          kind: "regulation",
          status: "attested",
          reasons: ["attestation: `2752b23a` by owner, valid until 2026-10-24 20:54:51 UTC"],
        },
        {
          control: "cra/annex-i-2-3",
          title: "Known exploitable vulnerabilities are tracked and remediated",
          kind: "regulation",
          status: "open",
          reasons: ["task: unevaluated", "attestation: none recorded"],
        },
        {
          control: "cra/annex-i-2-4",
          title: "Security updates are provided without undue delay",
          kind: "regulation",
          status: "not_applicable",
          reasons: [
            "marked not applicable at demo: demo scope ships no updates of its own; the root's process covers it",
          ],
        },
        {
          control: "cra/annex-i-2-5",
          title: "A coordinated vulnerability disclosure policy is published",
          kind: "regulation",
          status: "open",
          reasons: ["knowledge: tag `control/cra/annex-i-2-5` not found"],
        },
      ],
      rollup: [
        {
          framework: "cra",
          counts: { satisfied: 0, attested: 1, stale: 0, open: 3, not_applicable: 1 },
          best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 },
          compliant: false,
        },
      ],
    },
  ],
  rollup: [
    {
      framework: "cra",
      counts: { satisfied: 0, attested: 1, stale: 0, open: 3, not_applicable: 1 },
      best_practice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 },
      compliant: false,
    },
  ],
  not_applicable: [
    {
      control: "cra/annex-i-2-4",
      scope: "demo",
      rationale: "demo scope ships no updates of its own; the root's process covers it",
    },
  ],
  findings: [
    { kind: "loosening_has_no_effect", subject: "demo", detail: "tightens cra/annex-i-2-9 to 30d, which is not shorter than the existing 7d" },
    { kind: "unknown_control", subject: "demo", detail: "tightens cra/annex-i-2-99, which is not an applicable control" },
  ],
  catalogues: [{ framework: "cra", title: "Cyber Resilience Act", kind: "regulation", controls: 5 }],
};

// A real `GET /api/policy/controls/cra/annex-i-2-1?scope=demo` answer's
// `.detail` -- `status` nested, unlike a row's own flattened `ControlStatus`.
const detail = {
  control: "cra/annex-i-2-1",
  title: "Identify and document components (SBOM)",
  kind: "regulation",
  checks: [
    { check: "knowledge" },
    { check: "attestation" },
    { check: "task", task: "sbom-export", max_age: "30d" },
  ],
  maps_to: [],
  max_age: "2w",
  status: {
    status: "open",
    reasons: [
      "knowledge: tag `control/cra/annex-i-2-1` not found",
      "attestation: none recorded",
      "task: unevaluated",
    ],
  },
  attestations: [],
};

test("statusOf normalises the flattened row shape and the nested detail shape alike", () => {
  assert.deepEqual(statusOf(report.rows[0].statuses[0]), {
    status: "open",
    reasons: [
      "knowledge: tag `control/cra/annex-i-2-1` not found",
      "attestation: none recorded",
      "task: unevaluated",
    ],
  });
  assert.deepEqual(statusOf(detail), detail.status);
});

test("statusLabel shortens the one status the wire spells with an underscore", () => {
  assert.equal(statusLabel("not_applicable"), "n/a");
  assert.equal(statusLabel("open"), "open");
  assert.equal(statusLabel("satisfied"), "satisfied");
});

test("kindLabel and frameworkTitle read the wire's own spellings", () => {
  assert.equal(kindLabel("best-practice"), "Best practice");
  assert.equal(kindLabel("regulation"), "Regulation");
  assert.equal(kindLabel("something-new"), "something-new");
  assert.equal(frameworkTitle(report.catalogues, "cra"), "Cyber Resilience Act");
  assert.equal(frameworkTitle(report.catalogues, "dsgvo"), "dsgvo", "an unloaded framework falls back to its id");
});

test("frameworkCards joins the rollup with the catalogue title and kind, and totals best_practice", () => {
  const cards = frameworkCards(report);
  assert.equal(cards.length, 1);
  assert.deepEqual(cards[0], {
    framework: "cra",
    title: "Cyber Resilience Act",
    kind: "regulation",
    counts: { satisfied: 0, attested: 1, stale: 0, open: 3, not_applicable: 1 },
    bestPractice: { satisfied: 0, attested: 0, stale: 0, open: 0, not_applicable: 0 },
    countedTotal: 5,
    bestPracticeTotal: 0,
    compliant: false,
  });
});

test("gapRows is one row per (scope, control) that is open or stale, sorted by control then scope", () => {
  const rows = gapRows(report);
  assert.deepEqual(
    rows.map((r) => r.control),
    ["cra/annex-i-2-1", "cra/annex-i-2-3", "cra/annex-i-2-5"],
    "attested and not_applicable are left out",
  );
  assert.equal(rows[0].scope, "demo");
  assert.equal(rows[0].status, "open");
  assert.deepEqual(rows[0].reasons, report.rows[0].statuses[0].reasons);
});

test("gapRows carries no open task when the row's open_tasks is absent from the wire", () => {
  // `ScopePolicy.open_tasks` is skipped when empty, so the captured answer
  // above has none at all.
  assert.deepEqual(gapRows(report).map((r) => r.openTask), [null, null, null]);
});

test("gapRows names the open remediation task per (scope, control), from that row's own open_tasks", () => {
  // #98: two scopes with the same gap, a task open in only one of them --
  // the other still gets "Create task". A satisfied control's stray entry
  // makes no row of its own.
  const two = {
    rows: [
      {
        scope: "demo",
        statuses: [
          { control: "cra/a", title: "A", kind: "regulation", status: "open", reasons: [] },
          { control: "cra/b", title: "B", kind: "regulation", status: "stale", reasons: [] },
          { control: "cra/c", title: "C", kind: "regulation", status: "satisfied", reasons: [] },
        ],
        open_tasks: { "cra/a": "task-demo-a", "cra/c": "task-demo-c" },
      },
      {
        scope: "other",
        statuses: [{ control: "cra/a", title: "A", kind: "regulation", status: "open", reasons: [] }],
      },
    ],
  };
  assert.deepEqual(
    gapRows(two).map((r) => [r.control, r.scope, r.openTask]),
    [
      ["cra/a", "demo", "task-demo-a"],
      ["cra/a", "other", null],
      ["cra/b", "demo", null],
    ],
  );
});

test("openTaskFor reads the daemon's framework/id key and falls back to null", () => {
  assert.equal(openTaskFor({ open_tasks: { "cra/a": "t1" } }, "cra/a"), "t1");
  assert.equal(openTaskFor({ open_tasks: { "cra/a": "t1" } }, "cra/b"), null);
  assert.equal(openTaskFor({ scope: "demo" }, "cra/a"), null);
  assert.equal(openTaskFor(null, "cra/a"), null);
});

test("notApplicableRows recovers the title from any row's own ControlStatus", () => {
  const rows = notApplicableRows(report);
  assert.equal(rows.length, 1);
  assert.equal(rows[0].control, "cra/annex-i-2-4");
  assert.equal(rows[0].title, "Security updates are provided without undue delay");
  assert.equal(rows[0].scope, "demo");
  assert.match(rows[0].rationale, /ships no updates of its own/);
});

test("notApplicableRows falls back to the control id when no row mentions it", () => {
  const thin = { rows: [], not_applicable: [{ control: "cra/x", scope: "demo", rationale: "r" }] };
  assert.equal(notApplicableRows(thin)[0].title, "cra/x");
});

test("findingsByKind groups by the wire's snake_case kind, and findingLabel translates it", () => {
  const groups = findingsByKind(report.findings);
  assert.deepEqual([...groups.keys()], ["loosening_has_no_effect", "unknown_control"]);
  assert.equal(groups.get("unknown_control").length, 1);
  assert.equal(findingLabel("unknown_control"), "Names a control that is not applicable");
  assert.equal(findingLabel("unknown_daemon_fact"), "Names a daemon fact nothing recognizes");
  assert.equal(findingLabel("unknown_secrets_location"), "Names a secrets location nothing recognizes");
  assert.equal(findingLabel("a_future_kind"), "a_future_kind", "an unknown kind falls back to the raw string");
});

test("reasonCheckKinds reads the check-kind prefix direct_status writes, and ignores a maps_to reason", () => {
  assert.deepEqual(reasonCheckKinds(report.rows[0].statuses[0].reasons), ["knowledge", "attestation", "task"]);
  assert.deepEqual(reasonCheckKinds(["satisfied via cra/annex-i-2-2 (maps_to)"]), []);
  assert.deepEqual(reasonCheckKinds(["no evidence"]), []);
  assert.deepEqual(reasonCheckKinds(report.rows[0].statuses[3].reasons), [], "a not-applicable reason starts with 'marked', not a check kind");
});

test("closingLinks points a knowledge reason at the exact tag node, not just the Knowledge tab", () => {
  const links = closingLinks("demo", report.rows[0].statuses[0].reasons);
  assert.deepEqual(
    links.map((l) => l.label),
    ["Knowledge", "Tasks"],
    "attestation has no link of its own -- it is closed by the Attest form, not a level",
  );
  assert.equal(links[0].href, "#demo/knowledge/tag%3Acontrol/cra/annex-i-2-1", "one hash segment per nodeTail segment, matching knowledgeTail's own encoding");
  assert.match(links[1].href, /^#demo\/tasks$/);
});

test("closingLinks points a daemon reason at L1 Infrastructure", () => {
  const links = closingLinks("demo", ["daemon: `power_assertion` does not hold"]);
  assert.deepEqual(links.map((l) => l.label), ["Infrastructure"]);
  assert.match(links[0].href, /^#demo\/infrastructure$/);
});

test("closingLinks points an attested reason at Tasks (#158)", () => {
  const links = closingLinks("demo", ["attested: feature/tests -- 1 of 1 run(s) within 1w did not pass: run r1 (task t1)"]);
  assert.deepEqual(links.map((l) => l.label), ["Tasks"]);
  assert.match(links[0].href, /^#demo\/tasks$/);
});

test("defaultKnowledgeTag matches ControlRef::default_tag's own construction", () => {
  assert.equal(defaultKnowledgeTag("cra/annex-i-2-1"), "control/cra/annex-i-2-1");
});

test("checkTarget is exact, from the Check itself, not parsed out of a reason", () => {
  const knowledge = checkTarget("demo", "cra/annex-i-2-1", { check: "knowledge" });
  assert.equal(knowledge.label, "Knowledge");
  assert.equal(knowledge.href, "#demo/knowledge/tag%3Acontrol/cra/annex-i-2-1", "no explicit tag falls back to the control's default");

  const tagged = checkTarget("demo", "cra/annex-i-2-1", { check: "knowledge", tag: "custom-tag" });
  assert.equal(tagged.href, "#demo/knowledge/tag%3Acustom-tag");

  assert.equal(checkTarget("demo", "cra/annex-i-2-1", { check: "attestation" }), null);
  const daemon = checkTarget("demo", "cra/annex-i-2-1", { check: "daemon", fact: "x" });
  assert.equal(daemon.label, "Infrastructure");
  assert.match(daemon.href, /^#demo\/infrastructure$/);
  assert.equal(checkTarget("demo", "cra/annex-i-2-1", { check: "gate", dataset: "d" }).label, "Benchmarks");
  assert.equal(checkTarget("demo", "cra/annex-i-2-1", { check: "dependencies" }).label, "Dependencies");
});

test("describeCheck matches the CLI's own describe_check, one line per check kind", () => {
  assert.equal(describeCheck({ check: "knowledge" }), "knowledge: default tag");
  assert.equal(describeCheck({ check: "knowledge", tag: "x" }), "knowledge: tag `x`");
  assert.equal(describeCheck({ check: "attestation" }), "attestation");
  assert.equal(describeCheck({ check: "task", task: "sbom-export", max_age: "30d" }), "task sbom-export (max_age 30d)");
  assert.equal(describeCheck({ check: "task", task: "sbom-export" }), "task sbom-export");
  assert.equal(describeCheck({ check: "gate", dataset: "d", case: "c", max_age: "7d" }), "gate d/c (max_age 7d)");
  assert.equal(describeCheck({ check: "roles", forbid: ["knowledge.write"] }), "roles: forbid knowledge.write");
  assert.equal(describeCheck({ check: "sandbox" }), "sandbox");
  assert.equal(describeCheck({ check: "secrets" }), "secrets");
  assert.equal(describeCheck({ check: "secrets", absent: ["anthropic", "scope_env"] }), "secrets: absent anthropic, scope_env");
  assert.equal(describeCheck({ check: "daemon", fact: "power_assertion" }), "daemon: power_assertion");
  assert.equal(
    describeCheck({ check: "dependencies", sbom_max_age: "30d", built_sbom: true, max_open: { critical: 0, high: 1 }, exploited_open: 0 }),
    "dependencies: SBOM max_age 30d, built SBOM required, critical <= 0, high <= 1, exploited <= 0",
  );
  assert.equal(
    describeCheck({ check: "attested", category: "feature", step: "tests", max_age: "1w" }),
    "attested: feature/tests (max_age 1w)",
  );
});

test("looksLikeExpiry accepts policy::Duration's grammar and the two absolute forms, softly", () => {
  assert.equal(looksLikeExpiry("30d"), true);
  assert.equal(looksLikeExpiry("12w"), true);
  assert.equal(looksLikeExpiry("6h"), true);
  assert.equal(looksLikeExpiry("2027-01-01"), true);
  assert.equal(looksLikeExpiry("2027-01-01T00:00:00Z"), true);
  assert.equal(looksLikeExpiry(""), false);
  assert.equal(looksLikeExpiry("soon"), false);
});

test("canRemediate is true only for open and stale", () => {
  assert.equal(canRemediate("open"), true);
  assert.equal(canRemediate("stale"), true);
  assert.equal(canRemediate("satisfied"), false);
  assert.equal(canRemediate("attested"), false);
  assert.equal(canRemediate("not_applicable"), false);
});

test("remediateBody is the plain {control, scope} pair", () => {
  assert.deepEqual(remediateBody("cra/annex-i-2-1", "demo"), { control: "cra/annex-i-2-1", scope: "demo" });
});

test("remediateAction links an open task instead of offering Create task, and offers nothing without a gap", () => {
  assert.deepEqual(remediateAction("open", "task-1"), { kind: "open", task: "task-1" });
  assert.deepEqual(remediateAction("stale", "task-1"), { kind: "open", task: "task-1" });
  assert.deepEqual(remediateAction("open", null), { kind: "create" });
  assert.deepEqual(remediateAction("stale", undefined), { kind: "create" });
  // No gap, no action -- even if a task somehow still carries the label.
  assert.equal(remediateAction("satisfied", "task-1"), null);
  assert.equal(remediateAction("attested", null), null);
  assert.equal(remediateAction("not_applicable", null), null);
});

test("taskHref is refLinks' own task link, so the two cannot drift apart", () => {
  assert.equal(taskHref("demo", "task-1"), refLinks("demo", [{ kind: "task", id: "task-1" }])[0].href);
  assert.equal(taskHref("demo", "task-1"), "#demo/tasks/task/task-1");
});

test("refLinks opens the task modal for a lone task ref", () => {
  const links = refLinks("demo", [{ kind: "task", id: "task-1" }]);
  // No `/proc/` level segment: `routeHref` only inserts one once `initRail`
  // has told `scopes.js` which level owns which page (`levelForPage`),
  // which nothing in this Node test ever calls -- the same reason
  // `closingLinks`'s own tests above match `href` with a regex instead of
  // asserting the exact string.
  assert.deepEqual(links, [{ kind: "task", id: "task-1", label: "Task", href: "#demo/tasks/task/task-1" }]);
});

test("refLinks pairs a run ref with the sole task ref in the same list", () => {
  const links = refLinks("demo", [
    { kind: "task", id: "task-1" },
    { kind: "run", id: "run-1" },
  ]);
  assert.deepEqual(links, [
    { kind: "task", id: "task-1", label: "Task", href: "#demo/tasks/task/task-1" },
    { kind: "run", id: "run-1", label: "Run", href: "#demo/tasks/task/task-1/run-1" },
  ]);
});

test("refLinks drops a run ref when more than one task ref makes the pairing ambiguous", () => {
  const links = refLinks("demo", [
    { kind: "task", id: "task-1" },
    { kind: "task", id: "task-2" },
    { kind: "run", id: "run-1" },
  ]);
  assert.deepEqual(
    links.map((l) => l.kind),
    ["task", "task"],
    "both tasks still get a link; the ambiguous run does not",
  );
});

test("refLinks deep-links a bench_run and sends workflow_run to the Workflows view", () => {
  const links = refLinks("demo", [
    { kind: "bench_run", id: "bench-1" },
    { kind: "workflow_run", id: "wfrun-1" },
  ]);
  assert.deepEqual(links, [
    { kind: "bench_run", id: "bench-1", label: "Bench run", href: "#demo/benchmarks/runs/bench-1" },
    { kind: "workflow_run", id: "wfrun-1", label: "Workflow run", href: "#demo/workflows" },
  ]);
});

test("refLinks never links an attestation ref -- the modal's own table already shows it", () => {
  assert.deepEqual(refLinks("demo", [{ kind: "attestation", id: "att-1" }]), []);
});

test("attestBody refuses empty evidence or expiry with a sentence, and drops an empty note", () => {
  assert.throws(() => attestBody("cra/annex-i-2-1", "demo", { evidence: "", expires: "30d" }), /Evidence/);
  assert.throws(() => attestBody("cra/annex-i-2-1", "demo", { evidence: "x", expires: " " }), /Expiry/);
  assert.deepEqual(attestBody("cra/annex-i-2-1", "demo", { evidence: " https://x ", expires: "30d", note: "  " }), {
    control: "cra/annex-i-2-1",
    scope: "demo",
    evidence: "https://x",
    expires: "30d",
  });
  assert.deepEqual(attestBody("cra/annex-i-2-1", "demo", { evidence: "x", expires: "30d", note: " published " }), {
    control: "cra/annex-i-2-1",
    scope: "demo",
    evidence: "x",
    expires: "30d",
    note: "published",
  });
});
