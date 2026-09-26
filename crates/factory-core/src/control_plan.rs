//! `#118` v1 (compliant workflows): what a policy control or a quality
//! attribute says a piece of work must pass through, and the evidence that
//! it did.
//!
//! Policy (`policy.rs`) and quality (`quality.rs`) both say what good work
//! looks like, and until this module neither obliged a run to prove it. A
//! control -- or a quality attribute -- may now carry `requires:`, a list of
//! [`Requirement`]s: *this step, for work of these categories*. Folded down
//! the scope chain by the same add-or-tighten rule `policy::applicable`
//! already applies, those requirements become a scope's **control plan**
//! for one category ([`resolve`]). The daemon injects the plan's steps into
//! the concrete work at dispatch -- locked `Gate` nodes after each work node
//! of a workflow (`WorkflowDefinition::inject`), the same injection over an
//! implicit one-node definition for a standalone task -- and a run whose
//! agent reports `done` is only `done` once every required step has left a
//! passing [`StepAttestation`] ([`judge`]).
//!
//! Like the rest of L6 this module is pure: data in, data out, no clock, no
//! store and no process. Running a gate, storing an attestation and
//! settling a run are `factory-daemon`'s job.
//!
//! Gate, independent review and person approval steps are enforced. Gates
//! run in the daemon; review and approval carry a functionary frozen into
//! the run snapshot so a later roster edit cannot change who was required.

use crate::policy::Applied;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The category a task or workflow that names none is planned as -- so that
/// leaving `category` out is never a way around the plan. A catalogue
/// catches uncategorised work with `applies_to: [default]`.
pub const DEFAULT_CATEGORY: &str = "default";

/// In `applies_to`, every category, `default` included.
pub const ANY_CATEGORY: &str = "*";

/// Who a gate's attestation names as having run it. The daemon, never the
/// agent whose work is being judged -- see [`judge`].
pub const GATE_ACTOR: &str = "factory-daemon";

/// The category a piece of work is planned as.
pub fn effective_category(category: Option<&str>) -> &str {
    match category.map(str::trim) {
        Some(c) if !c.is_empty() => c,
        _ => DEFAULT_CATEGORY,
    }
}

/// `[a-z0-9][a-z0-9_-]*` -- a category or a step name. Wider than
/// `dataset::is_slug` by the underscore, because the issue's own step names
/// (`security_scan`, `security-report`) use both.
pub fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Refuse a category that could never match an `applies_to` entry.
pub fn check_category(category: &str) -> Result<(), String> {
    if is_name(category) {
        Ok(())
    } else {
        Err(format!(
            "category {category:?} is not a name: lowercase letters, digits, '-' and '_', starting with a letter or digit"
        ))
    }
}

// ============================================================ requirement

/// What kind of step a requirement names. Derived from the step's name:
/// `review` and `approval` are the two that need a person or another agent;
/// every other name (`tests`, `lint`, `sbom`, `security_scan`, ...) is a gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Gate,
    Review,
    Approval,
}

impl StepKind {
    pub fn of(step: &str) -> Self {
        match step {
            "review" => Self::Review,
            "approval" => Self::Approval,
            _ => Self::Gate,
        }
    }

    /// Every control-plan step is enforced. The kind decides which
    /// coordinator produces its evidence.
    pub fn enforced(self) -> bool {
        true
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gate => "gate",
            Self::Review => "review",
            Self::Approval => "approval",
        }
    }
}

/// One `requires:` entry on a policy control or a quality attribute, exactly
/// as authored:
///
/// ```yaml
/// requires:
///   - { applies_to: [feature, bugfix], step: tests, gate: "cargo test" }
///   - { applies_to: [release], step: sbom, gate: "make sbom", before: publish }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    /// The categories this step is required for; `*` is every category.
    pub applies_to: Vec<String>,
    pub step: String,
    /// The shell command a `gate` step runs, in the run's worktree; exit 0
    /// passes. A gate step without one can never pass -- the plan keeps it
    /// (never drop, never skip) and a finding says why.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    /// For `review`/`approval`: who may produce the evidence (`independent`,
    /// `person`). Parsed and shown; enforced in v2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Ordering between steps of the same plan, by step name (DECLARE's
    /// `precedence`): this step runs before, or after, the named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// How long the gate may run. Defaults to the bench gate runner's own
    /// ten minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
}

impl Requirement {
    pub fn kind(&self) -> StepKind {
        StepKind::of(&self.step)
    }

    pub fn applies(&self, category: &str) -> bool {
        self.applies_to.iter().any(|c| c == ANY_CATEGORY || c == category)
    }

    /// What is wrong with this requirement, as sentences that read on after
    /// the name of whatever declares it. Never a reason to drop it -- see
    /// the module doc comment.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !is_name(&self.step) {
            out.push(format!("requires step {:?}, which is not a step name", self.step));
        }
        if self.applies_to.is_empty() {
            out.push(format!("requires {} for no category; `applies_to` is empty", self.step));
        }
        for c in &self.applies_to {
            if c != ANY_CATEGORY && !is_name(c) {
                out.push(format!("requires {} for {c:?}, which is not a category name", self.step));
            }
        }
        match self.kind() {
            StepKind::Gate => {
                if self.gate.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    out.push(format!(
                        "requires gate step {} with no `gate:` command; a run that needs it can never pass",
                        self.step
                    ));
                }
                if self.by.is_some() {
                    out.push(format!("gives gate step {} a `by:`; only review and approval have one", self.step));
                }
            }
            kind => {
                if self.gate.is_some() {
                    out.push(format!("gives {} step a `gate:` command; only gate steps run one", kind.as_str()));
                }
                if let Some(by) = &self.by {
                    if by != "independent" && by != "person" {
                        out.push(format!("names `by: {by}`, which is not `independent` or `person`"));
                    }
                }
            }
        }
        if self.timeout_seconds == Some(0) {
            out.push(format!("gives step {} a zero timeout", self.step));
        }
        out
    }
}

// =================================================================== plan

/// One step of a resolved plan -- every requirement for the same step name
/// and command folded into one, naming every control that asked for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    /// Unique within the plan: the step name, or `<name>-2`, `-3`, ... when
    /// two controls require the same step name with different commands --
    /// add only, so both run.
    pub id: String,
    pub step: String,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The shortest any requirement asked for -- tighten only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub before: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub after: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// `<framework>/<control>` for a policy control, `quality/<attribute>`
    /// for a quality attribute.
    pub required_by: Vec<String>,
    /// Kept on the wire for v1 snapshots. New plans enforce every kind.
    pub enforced: bool,
}

/// A control marked `n/a` whose requirements would otherwise have applied --
/// the one way a step leaves a plan, listed like any other so it is never a
/// silent drop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waiver {
    pub control: String,
    pub scope: String,
    pub rationale: String,
    pub steps: Vec<String>,
}

/// A scope's control plan for one category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlan {
    pub scope: String,
    pub category: String,
    /// In execution order: `before`/`after` respected, otherwise by id.
    pub steps: Vec<PlanStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waived: Vec<Waiver>,
    /// Anything the plan could not honour as written -- a gate with no
    /// command, an ordering cycle -- in words.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<String>,
}

impl ControlPlan {
    /// The steps the daemon injects and judges.
    pub fn enforced(&self) -> impl Iterator<Item = &PlanStep> {
        self.steps.iter().filter(|s| s.enforced)
    }
}

/// Resolve the plan for `category` at `scope` from what already applies
/// there: the policy controls `policy::applicable` folded down the scope's
/// chain (frameworks only ever added, `n/a` only with a rationale), and the
/// requirements of the quality attributes the scope's quality chain folded
/// (`quality`, as `(source, requirements)` -- `quality::requirements_of`).
/// Add or tighten only, by construction: nothing here can drop a
/// requirement except an `n/a`, which is listed in `waived`.
pub fn resolve(scope: &str, category: &str, policy: &[Applied], quality: &[(String, Vec<Requirement>)]) -> ControlPlan {
    let mut plan = ControlPlan { scope: scope.to_string(), category: category.to_string(), ..Default::default() };

    let mut sources: Vec<(String, &Requirement)> = Vec::new();
    for applied in policy {
        let reqs: Vec<&Requirement> = applied.requires.iter().filter(|r| r.applies(category)).collect();
        if reqs.is_empty() {
            continue;
        }
        let control = applied.control.to_string();
        if let Some(na) = &applied.not_applicable {
            plan.waived.push(Waiver {
                control,
                scope: na.scope.clone(),
                rationale: na.rationale.clone(),
                steps: reqs.iter().map(|r| r.step.clone()).collect(),
            });
            continue;
        }
        sources.extend(reqs.into_iter().map(|r| (control.clone(), r)));
    }
    for (source, reqs) in quality {
        sources.extend(reqs.iter().filter(|r| r.applies(category)).map(|r| (source.clone(), r)));
    }

    // Fold: one step per (name, command).
    let mut steps: Vec<PlanStep> = Vec::new();
    for (source, req) in sources {
        let command = req.gate.as_deref().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
        if let Some(have) = steps.iter_mut().find(|s| s.step == req.step && s.command == command) {
            if !have.required_by.contains(&source) {
                have.required_by.push(source);
            }
            have.timeout_seconds = match (have.timeout_seconds, req.timeout_seconds) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            push_unique(&mut have.before, req.before.as_ref());
            push_unique(&mut have.after, req.after.as_ref());
            if have.by.is_none() {
                have.by = req.by.clone();
            }
            continue;
        }
        let kind = req.kind();
        if kind == StepKind::Gate && command.is_none() {
            plan.findings.push(format!(
                "{source} requires gate step {} with no command; every {category} run here will block on it",
                req.step
            ));
        }
        let mut step = PlanStep {
            id: req.step.clone(),
            step: req.step.clone(),
            kind,
            command,
            timeout_seconds: req.timeout_seconds,
            before: Vec::new(),
            after: Vec::new(),
            by: req.by.clone(),
            required_by: vec![source],
            enforced: kind.enforced(),
        };
        push_unique(&mut step.before, req.before.as_ref());
        push_unique(&mut step.after, req.after.as_ref());
        steps.push(step);
    }

    // Unique ids, deterministic: by (name, command) order.
    steps.sort_by(|a, b| a.step.cmp(&b.step).then(a.command.cmp(&b.command)));
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for step in &mut steps {
        let n = seen.entry(step.step.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            step.id = format!("{}-{}", step.step, n);
        }
        step.required_by.sort();
    }

    let (ordered, cycle) = order(steps);
    if let Some(cycle) = cycle {
        plan.findings.push(format!("the plan's before/after ordering has a cycle through {cycle}; ran in name order instead"));
    }
    plan.steps = ordered;
    plan.findings.sort();
    plan.findings.dedup();
    plan
}

fn push_unique(list: &mut Vec<String>, value: Option<&String>) {
    if let Some(v) = value {
        if !list.contains(v) {
            list.push(v.clone());
        }
    }
}

/// Topological order by `before`/`after` (both by step name), ties by id.
/// A cycle keeps id order and names the steps in it.
fn order(steps: Vec<PlanStep>) -> (Vec<PlanStep>, Option<String>) {
    let n = steps.len();
    let mut edges: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (i, s) in steps.iter().enumerate() {
        for (j, t) in steps.iter().enumerate() {
            if i == j {
                continue;
            }
            if s.before.contains(&t.step) || t.after.contains(&s.step) {
                edges.insert((i, j));
            }
        }
    }
    let mut indegree = vec![0usize; n];
    for &(_, j) in &edges {
        indegree[j] += 1;
    }
    let mut done = vec![false; n];
    let mut out = Vec::with_capacity(n);
    loop {
        let next = (0..n)
            .filter(|&i| !done[i] && indegree[i] == 0)
            .min_by(|&a, &b| steps[a].id.cmp(&steps[b].id));
        let Some(i) = next else { break };
        done[i] = true;
        out.push(i);
        for &(from, to) in &edges {
            if from == i {
                indegree[to] -= 1;
            }
        }
    }
    if out.len() == n {
        let mut steps: Vec<Option<PlanStep>> = steps.into_iter().map(Some).collect();
        return (out.into_iter().map(|i| steps[i].take().expect("each once")).collect(), None);
    }
    let cyclic = (0..n).filter(|&i| !done[i]).map(|i| steps[i].id.clone()).collect::<Vec<_>>().join(", ");
    let mut steps = steps;
    steps.sort_by(|a, b| a.id.cmp(&b.id));
    (steps, Some(cyclic))
}

// ============================================================ attestation

/// One step a particular run must pass before it is `done`, fixed at
/// dispatch -- a catalogue edited while the run works does not change what
/// it is held to, the same rule a workflow run's definition snapshot keeps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequiredStep {
    /// The plan step's id -- what an attestation names.
    pub step: String,
    pub kind: StepKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    /// The declaration that selected the functionary (`independent` or
    /// `person`). Additive so v1 snapshots still deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The concrete agent/person identity frozen at dispatch. Approval uses
    /// `owner`; an unbound independent review stays `None` and blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    /// The gate node this step is, in the workflow run's snapshot -- or in
    /// the implicit one-node definition a standalone task is planned as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationVerdict {
    Pass,
    Fail,
}

impl AttestationVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }
}

/// The evidence one step left for one run: who produced it, what it
/// judged, and what it found. Written once, append-only, and never edited --
/// a second verification of the same run appends new ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepAttestation {
    pub id: String,
    pub run_id: String,
    pub task_id: String,
    pub scope: String,
    pub category: String,
    pub step: String,
    pub kind: StepKind,
    /// Who produced the evidence -- [`GATE_ACTOR`] for a gate.
    pub actor: String,
    pub verdict: AttestationVerdict,
    /// Review findings or a person's decision reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<String>,
    /// Rework round this evidence belongs to. Zero is the first pass.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The last 4 KiB of the gate's combined output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// The directory the gate judged -- the run's worktree, or the scope.
    pub dir: String,
    /// `git rev-parse HEAD` in `dir` when the gate ran, and whether the tree
    /// had uncommitted changes: the state the verdict is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    /// SHA-256 over the exact tracked diff and untracked file contents that
    /// this evidence judged. Additive so older attestations remain readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    pub at: DateTime<Utc>,
}

/// Whether a run's evidence is complete, and if not, why -- in words a
/// person reads in the Inbox.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verification {
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<String>,
}

impl Verification {
    /// One sentence for the run's block reason.
    pub fn reason(&self) -> String {
        let mut parts = Vec::new();
        if !self.failed.is_empty() {
            parts.push(format!("failed: {}", self.failed.join("; ")));
        }
        if !self.missing.is_empty() {
            parts.push(format!("no evidence for: {}", self.missing.join("; ")));
        }
        format!("verification did not pass -- {}", parts.join(" -- "))
    }
}

/// The `done` gate: every enforced required step has an attestation at or
/// after `since` (this verification round), produced by someone other than
/// `executing_agent`, and the newest such one for each step passed.
pub fn judge(
    required: &[RequiredStep],
    attestations: &[StepAttestation],
    executing_agent: &str,
    since: DateTime<Utc>,
) -> Verification {
    let mut out = Verification { passed: true, ..Default::default() };
    for step in required.iter().filter(|s| s.kind.enforced()) {
        let by = if step.required_by.is_empty() {
            String::new()
        } else {
            format!(" (required by {})", step.required_by.join(", "))
        };
        let newest = attestations
            .iter()
            .filter(|a| {
                a.step == step.step
                    && (step.kind != StepKind::Gate || a.at >= since)
                    && a.actor != executing_agent
                    && step.actor.as_deref().is_none_or(|actor| actor == a.actor)
            })
            .max_by_key(|a| a.at);
        match newest {
            None => {
                out.passed = false;
                let why = if step.kind == StepKind::Gate && step.command.is_none() {
                    " -- no gate command is declared for it"
                } else if step.kind == StepKind::Review && step.actor.is_none() {
                    " -- no independent agent is declared in this scope"
                } else {
                    ""
                };
                out.missing.push(format!("{}{by}{why}", step.step));
            }
            Some(a) if a.verdict == AttestationVerdict::Fail => {
                out.passed = false;
                let code = match a.exit_code {
                    Some(code) => format!("exit {code}"),
                    None => "did not finish".to_string(),
                };
                out.failed.push(format!("{} {code}{by}", step.step));
            }
            Some(_) => {}
        }
    }
    out
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{AppliedNotApplicable, ControlRef, Kind};

    fn req(step: &str, applies: &[&str], gate: Option<&str>) -> Requirement {
        Requirement {
            applies_to: applies.iter().map(|s| s.to_string()).collect(),
            step: step.into(),
            gate: gate.map(str::to_string),
            by: None,
            before: None,
            after: None,
            timeout_seconds: None,
        }
    }

    fn applied(fw: &str, id: &str, requires: Vec<Requirement>) -> Applied {
        Applied {
            control: ControlRef::new(fw, id),
            title: id.into(),
            kind: Kind::Regulation,
            maps_to: vec![],
            evidence: vec![],
            max_age: None,
            not_applicable: None,
            remediation: None,
            requires,
        }
    }

    #[test]
    fn no_category_is_the_default_category() {
        assert_eq!(effective_category(None), "default");
        assert_eq!(effective_category(Some("  ")), "default");
        assert_eq!(effective_category(Some("feature")), "feature");
    }

    #[test]
    fn a_requirement_applies_only_to_its_categories_or_to_every_one_with_a_star() {
        let r = req("tests", &["feature"], Some("true"));
        assert!(r.applies("feature"));
        assert!(!r.applies("docs"));
        assert!(req("lint", &["*"], Some("true")).applies("default"));
    }

    #[test]
    fn requirements_for_the_same_step_and_command_fold_into_one_step_naming_both_controls() {
        let policy = vec![
            applied("cra", "security-testing", vec![req("tests", &["feature"], Some("make test"))]),
            applied("iso", "a-8-29", vec![req("tests", &["feature", "bugfix"], Some("make test"))]),
        ];
        let plan = resolve("demo", "feature", &policy, &[]);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].required_by, vec!["cra/security-testing", "iso/a-8-29"]);
    }

    #[test]
    fn the_same_step_with_two_commands_keeps_both_so_nothing_is_dropped() {
        let policy = vec![
            applied("a", "x", vec![req("tests", &["*"], Some("make test"))]),
            applied("b", "y", vec![req("tests", &["*"], Some("cargo test"))]),
        ];
        let plan = resolve("demo", "default", &policy, &[]);
        let ids: Vec<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["tests", "tests-2"]);
    }

    #[test]
    fn a_category_nothing_requires_anything_for_has_an_empty_plan() {
        let policy = vec![applied("cra", "sbom", vec![req("sbom", &["release"], Some("make sbom"))])];
        assert!(resolve("demo", "docs", &policy, &[]).steps.is_empty());
    }

    #[test]
    fn a_control_marked_not_applicable_is_listed_as_waived_not_silently_dropped() {
        let mut waived = applied("cra", "sbom", vec![req("sbom", &["release"], Some("make sbom"))]);
        waived.not_applicable = Some(AppliedNotApplicable { scope: "demo".into(), rationale: "no binaries".into() });
        let plan = resolve("demo", "release", &[waived], &[]);
        assert!(plan.steps.is_empty());
        assert_eq!(plan.waived.len(), 1);
        assert_eq!(plan.waived[0].steps, vec!["sbom"]);
        assert_eq!(plan.waived[0].rationale, "no binaries");
    }

    #[test]
    fn quality_requirements_join_the_same_plan() {
        let plan = resolve(
            "demo",
            "feature",
            &[],
            &[("quality/maintainability.testability".into(), vec![req("tests", &["feature"], Some("true"))])],
        );
        assert_eq!(plan.steps[0].required_by, vec!["quality/maintainability.testability"]);
    }

    #[test]
    fn before_and_after_order_the_steps_and_timeouts_only_tighten() {
        let mut sbom = req("sbom", &["*"], Some("make sbom"));
        sbom.after = Some("tests".into());
        let mut lint = req("lint", &["*"], Some("make lint"));
        lint.before = Some("tests".into());
        let mut t1 = req("tests", &["*"], Some("make test"));
        t1.timeout_seconds = Some(600);
        let mut t2 = t1.clone();
        t2.timeout_seconds = Some(60);
        let policy = vec![applied("a", "x", vec![sbom, t1]), applied("b", "y", vec![lint, t2])];
        let plan = resolve("demo", "default", &policy, &[]);
        let ids: Vec<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["lint", "tests", "sbom"]);
        assert_eq!(plan.steps[1].timeout_seconds, Some(60));
    }

    #[test]
    fn an_ordering_cycle_is_a_finding_and_the_steps_still_run() {
        let mut a = req("a", &["*"], Some("true"));
        a.before = Some("b".into());
        let mut b = req("b", &["*"], Some("true"));
        b.before = Some("a".into());
        let plan = resolve("demo", "default", &[applied("f", "x", vec![a, b])], &[]);
        assert_eq!(plan.steps.len(), 2);
        assert!(plan.findings.iter().any(|f| f.contains("cycle")), "{:?}", plan.findings);
    }

    #[test]
    fn review_and_approval_are_enforced_without_a_gate_command() {
        let mut review = req("review", &["feature"], None);
        review.by = Some("independent".into());
        let plan = resolve("demo", "feature", &[applied("f", "x", vec![review])], &[]);
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.steps[0].enforced);
        assert_eq!(plan.enforced().count(), 1);
        assert!(plan.findings.is_empty(), "{:?}", plan.findings);
    }

    #[test]
    fn a_gate_with_no_command_stays_in_the_plan_with_a_finding() {
        let plan = resolve("demo", "feature", &[applied("f", "x", vec![req("tests", &["feature"], None)])], &[]);
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.steps[0].command.is_none());
        assert!(plan.findings.iter().any(|f| f.contains("no command")), "{:?}", plan.findings);
    }

    #[test]
    fn requirement_problems_name_what_is_wrong() {
        assert!(req("tests", &["feature"], Some("true")).problems().is_empty());
        assert!(!req("tests", &[], Some("true")).problems().is_empty());
        assert!(!req("tests", &["Feature!"], Some("true")).problems().is_empty());
        assert!(!req("tests", &["feature"], None).problems().is_empty());
        assert!(!req("review", &["feature"], Some("x")).problems().is_empty());
    }

    fn required(step: &str) -> RequiredStep {
        RequiredStep {
            step: step.into(),
            kind: StepKind::Gate,
            command: Some("true".into()),
            timeout_seconds: None,
            required_by: vec!["cra/x".into()],
            by: None,
            actor: None,
            node_id: None,
        }
    }

    fn attest(step: &str, verdict: AttestationVerdict, actor: &str, at: DateTime<Utc>) -> StepAttestation {
        StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: "r".into(),
            task_id: "t".into(),
            scope: "demo".into(),
            category: "feature".into(),
            step: step.into(),
            kind: StepKind::Gate,
            actor: actor.into(),
            verdict,
            findings: None,
            round: 0,
            required_by: vec![],
            command: Some("true".into()),
            exit_code: Some(if verdict == AttestationVerdict::Pass { 0 } else { 1 }),
            output: None,
            dir: "/tmp".into(),
            commit: None,
            dirty: None,
            worktree_digest: None,
            node_id: None,
            at,
        }
    }

    #[test]
    fn v1_steps_and_attestations_load_with_empty_v2_functionary_fields() {
        let mut step = serde_json::to_value(required("tests")).unwrap();
        step.as_object_mut().unwrap().remove("by");
        step.as_object_mut().unwrap().remove("actor");
        let step: RequiredStep = serde_json::from_value(step).unwrap();
        assert_eq!(step.by, None);
        assert_eq!(step.actor, None);

        let mut evidence = serde_json::to_value(attest(
            "tests",
            AttestationVerdict::Pass,
            GATE_ACTOR,
            Utc::now(),
        ))
        .unwrap();
        evidence.as_object_mut().unwrap().remove("findings");
        evidence.as_object_mut().unwrap().remove("round");
        evidence.as_object_mut().unwrap().remove("worktree_digest");
        let evidence: StepAttestation = serde_json::from_value(evidence).unwrap();
        assert_eq!(evidence.findings, None);
        assert_eq!(evidence.round, 0);
        assert_eq!(evidence.worktree_digest, None);
    }

    #[test]
    fn done_needs_every_required_step_to_have_passed() {
        let t0 = Utc::now();
        let req = vec![required("tests"), required("lint")];
        let only_tests = vec![attest("tests", AttestationVerdict::Pass, GATE_ACTOR, t0)];
        let v = judge(&req, &only_tests, "claude-code", t0);
        assert!(!v.passed);
        assert_eq!(v.missing.len(), 1);
        assert!(v.missing[0].starts_with("lint"), "{:?}", v.missing);

        let both = vec![
            attest("tests", AttestationVerdict::Pass, GATE_ACTOR, t0),
            attest("lint", AttestationVerdict::Pass, GATE_ACTOR, t0),
        ];
        assert!(judge(&req, &both, "claude-code", t0).passed);
    }

    #[test]
    fn a_failed_step_fails_the_verification_and_says_which() {
        let t0 = Utc::now();
        let v = judge(&[required("tests")], &[attest("tests", AttestationVerdict::Fail, GATE_ACTOR, t0)], "a", t0);
        assert!(!v.passed);
        assert!(v.failed[0].contains("tests exit 1"), "{:?}", v.failed);
        assert!(v.reason().contains("required by cra/x"), "{}", v.reason());
    }

    #[test]
    fn evidence_the_executing_agent_produced_itself_does_not_count() {
        let t0 = Utc::now();
        let v = judge(&[required("tests")], &[attest("tests", AttestationVerdict::Pass, "claude-code", t0)], "claude-code", t0);
        assert!(!v.passed);
    }

    #[test]
    fn only_this_rounds_evidence_counts_and_the_newest_wins() {
        let t0 = Utc::now();
        let earlier = t0 - chrono::Duration::seconds(10);
        let stale_pass = vec![attest("tests", AttestationVerdict::Pass, GATE_ACTOR, earlier)];
        assert!(!judge(&[required("tests")], &stale_pass, "a", t0).passed, "an earlier round's pass is not this round's");
        let later = t0 + chrono::Duration::seconds(1);
        let flipped = vec![
            attest("tests", AttestationVerdict::Fail, GATE_ACTOR, t0),
            attest("tests", AttestationVerdict::Pass, GATE_ACTOR, later),
        ];
        assert!(judge(&[required("tests")], &flipped, "a", t0).passed);
    }

    #[test]
    fn a_review_is_judged_and_must_come_from_its_frozen_functionary() {
        let t0 = Utc::now();
        let mut review = required("review");
        review.kind = StepKind::Review;
        review.actor = Some("checker".into());
        assert!(!judge(&[review.clone()], &[], "maker", t0).passed);
        let wrong = vec![attest("review", AttestationVerdict::Pass, "other", t0)];
        assert!(!judge(&[review.clone()], &wrong, "maker", t0).passed);
        let right = vec![attest("review", AttestationVerdict::Pass, "checker", t0)];
        assert!(judge(&[review], &right, "maker", t0).passed);
    }
}
