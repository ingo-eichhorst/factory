//! L5 is the sole compiler of policy and quality requirements into a plan.
use factory_kernel::StepKind;
use factory_process::control_plan::{is_name, ControlPlan, PlanStep, Waiver, ANY_CATEGORY};
#[cfg(test)]
use factory_process::control_plan::ControlPlanExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanWaiver {
    pub scope: String,
    pub rationale: String,
}

/// Producer-adapted declaration data; no L6 policy type or status read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanSource {
    pub source: String,
    pub requires: Vec<Requirement>,
    pub not_applicable: Option<PlanWaiver>,
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
        self.applies_to
            .iter()
            .any(|c| c == ANY_CATEGORY || c == category)
    }

    /// What is wrong with this requirement, as sentences that read on after
    /// the name of whatever declares it. Never a reason to drop it -- see
    /// the module doc comment.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !is_name(&self.step) {
            out.push(format!(
                "requires step {:?}, which is not a step name",
                self.step
            ));
        }
        if self.applies_to.is_empty() {
            out.push(format!(
                "requires {} for no category; `applies_to` is empty",
                self.step
            ));
        }
        for c in &self.applies_to {
            if c != ANY_CATEGORY && !is_name(c) {
                out.push(format!(
                    "requires {} for {c:?}, which is not a category name",
                    self.step
                ));
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
                    out.push(format!(
                        "gives gate step {} a `by:`; only review and approval have one",
                        self.step
                    ));
                }
            }
            kind => {
                if self.gate.is_some() {
                    out.push(format!(
                        "gives {} step a `gate:` command; only gate steps run one",
                        kind.as_str()
                    ));
                }
                if let Some(by) = &self.by {
                    if by != "independent" && by != "person" {
                        out.push(format!(
                            "names `by: {by}`, which is not `independent` or `person`"
                        ));
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

/// Resolve the plan for `category` at `scope` from what already applies
/// there: the policy controls `policy::applicable` folded down the scope's
/// chain (frameworks only ever added, `n/a` only with a rationale), and the
/// requirements of the quality attributes the scope's quality chain folded
/// (`quality`, as `(source, requirements)` -- `quality::requirements_of`).
/// Add or tighten only, by construction: nothing here can drop a
/// requirement except an `n/a`, which is listed in `waived`.
pub fn resolve(
    scope: &str,
    category: &str,
    policy: &[PlanSource],
    quality: &[(String, Vec<Requirement>)],
) -> ControlPlan {
    let mut plan = ControlPlan {
        scope: scope.to_string(),
        category: category.to_string(),
        ..Default::default()
    };

    let mut sources: Vec<(String, &Requirement)> = Vec::new();
    for applied in policy {
        let reqs: Vec<&Requirement> = applied
            .requires
            .iter()
            .filter(|r| r.applies(category))
            .collect();
        if reqs.is_empty() {
            continue;
        }
        let control = applied.source.clone();
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
        sources.extend(
            reqs.iter()
                .filter(|r| r.applies(category))
                .map(|r| (source.clone(), r)),
        );
    }

    // Fold: one step per (name, command).
    let mut steps: Vec<PlanStep> = Vec::new();
    for (source, req) in sources {
        let command = req
            .gate
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string);
        if let Some(have) = steps
            .iter_mut()
            .find(|s| s.step == req.step && s.command == command)
        {
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
        return (
            out.into_iter()
                .map(|i| steps[i].take().expect("each once"))
                .collect(),
            None,
        );
    }
    let cyclic = (0..n)
        .filter(|&i| !done[i])
        .map(|i| steps[i].id.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let mut steps = steps;
    steps.sort_by(|a, b| a.id.cmp(&b.id));
    (steps, Some(cyclic))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(step: &str, gate: Option<&str>) -> Requirement {
        Requirement {
            applies_to: vec!["feature".into()],
            step: step.into(),
            gate: gate.map(str::to_string),
            by: None,
            before: None,
            after: None,
            timeout_seconds: None,
        }
    }

    #[test]
    fn policy_and_quality_are_compiled_once_into_the_same_generic_step() {
        let mut declared = req("tests", Some("cargo test"));
        declared.timeout_seconds = Some(90);
        let source = PlanSource {
            source: "house/tested".into(),
            requires: vec![declared],
            not_applicable: None,
        };
        let mut quality = req("tests", Some("cargo test"));
        quality.timeout_seconds = Some(30);
        let plan = resolve(
            "demo",
            "feature",
            &[source],
            &[("quality/verified".into(), vec![quality])],
        );
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.steps[0].timeout_seconds, Some(30));
        assert_eq!(
            plan.steps[0].required_by,
            ["house/tested", "quality/verified"]
        );
        assert!(plan.steps[0].enforced);
    }

    #[test]
    fn a_policy_waiver_cannot_drop_the_quality_requirement_or_other_categories() {
        let source = PlanSource {
            source: "house/tested".into(),
            requires: vec![req("tests", Some("true"))],
            not_applicable: Some(PlanWaiver {
                scope: "demo".into(),
                rationale: "owner rationale".into(),
            }),
        };
        let plan = resolve(
            "demo",
            "feature",
            &[source.clone()],
            &[("quality/verified".into(), vec![req("tests", Some("true"))])],
        );
        assert_eq!(plan.waived.len(), 1);
        assert_eq!(plan.waived[0].control, "house/tested");
        assert_eq!(plan.steps[0].required_by, ["quality/verified"]);
        let other = resolve("demo", "docs", &[source], &[]);
        assert!(other.steps.is_empty());
        assert!(other.waived.is_empty());
    }

    #[test]
    fn missing_commands_and_cycles_remain_enforced_findings() {
        let mut first = req("tests", None);
        first.before = Some("lint".into());
        let mut second = req("lint", Some("true"));
        second.before = Some("tests".into());
        let plan = resolve(
            "demo",
            "feature",
            &[],
            &[("quality/x".into(), vec![first, second])],
        );
        assert_eq!(plan.enforced().count(), 2);
        assert!(plan
            .findings
            .iter()
            .any(|finding| finding.contains("cycle")));
        assert!(plan
            .findings
            .iter()
            .any(|finding| finding.contains("no command")));
    }
}
