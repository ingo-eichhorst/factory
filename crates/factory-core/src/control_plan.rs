//! Compatibility composition outside the ladder: L6 declarations are adapted
//! to L5 sources; the sole L5 compiler returns L4 execution gates.
use crate::policy::Applied;
pub use factory_assurance::control_plan::{PlanSource, PlanWaiver, Requirement};
pub use factory_process::control_plan::*;

pub fn resolve(
    scope: &str,
    category: &str,
    policy: &[Applied],
    quality: &[(String, Vec<Requirement>)],
) -> ControlPlan {
    let sources: Vec<PlanSource> = policy
        .iter()
        .map(|applied| PlanSource {
            source: applied.control.to_string(),
            requires: applied.requires.clone(),
            not_applicable: applied.not_applicable.as_ref().map(|na| PlanWaiver {
                scope: na.scope.clone(),
                rationale: na.rationale.clone(),
            }),
        })
        .collect();
    factory_assurance::control_plan::resolve(scope, category, &sources, quality)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{AppliedNotApplicable, ControlRef, Kind};
    use chrono::{DateTime, Utc};

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
            applied(
                "cra",
                "security-testing",
                vec![req("tests", &["feature"], Some("make test"))],
            ),
            applied(
                "iso",
                "a-8-29",
                vec![req("tests", &["feature", "bugfix"], Some("make test"))],
            ),
        ];
        let plan = resolve("demo", "feature", &policy, &[]);
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(
            plan.steps[0].required_by,
            vec!["cra/security-testing", "iso/a-8-29"]
        );
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
        let policy = vec![applied(
            "cra",
            "sbom",
            vec![req("sbom", &["release"], Some("make sbom"))],
        )];
        assert!(resolve("demo", "docs", &policy, &[]).steps.is_empty());
    }

    #[test]
    fn a_control_marked_not_applicable_is_listed_as_waived_not_silently_dropped() {
        let mut waived = applied(
            "cra",
            "sbom",
            vec![req("sbom", &["release"], Some("make sbom"))],
        );
        waived.not_applicable = Some(AppliedNotApplicable {
            scope: "demo".into(),
            rationale: "no binaries".into(),
        });
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
            &[(
                "quality/maintainability.testability".into(),
                vec![req("tests", &["feature"], Some("true"))],
            )],
        );
        assert_eq!(
            plan.steps[0].required_by,
            vec!["quality/maintainability.testability"]
        );
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
        let policy = vec![
            applied("a", "x", vec![sbom, t1]),
            applied("b", "y", vec![lint, t2]),
        ];
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
        assert!(
            plan.findings.iter().any(|f| f.contains("cycle")),
            "{:?}",
            plan.findings
        );
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
        let plan = resolve(
            "demo",
            "feature",
            &[applied("f", "x", vec![req("tests", &["feature"], None)])],
            &[],
        );
        assert_eq!(plan.steps.len(), 1);
        assert!(plan.steps[0].command.is_none());
        assert!(
            plan.findings.iter().any(|f| f.contains("no command")),
            "{:?}",
            plan.findings
        );
    }

    #[test]
    fn requirement_problems_name_what_is_wrong() {
        assert!(req("tests", &["feature"], Some("true"))
            .problems()
            .is_empty());
        assert!(!req("tests", &[], Some("true")).problems().is_empty());
        assert!(!req("tests", &["Feature!"], Some("true"))
            .problems()
            .is_empty());
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

    fn attest(
        step: &str,
        verdict: AttestationVerdict,
        actor: &str,
        at: DateTime<Utc>,
    ) -> StepAttestation {
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
            exit_code: Some(if verdict == AttestationVerdict::Pass {
                0
            } else {
                1
            }),
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
        let v = judge(
            &[required("tests")],
            &[attest("tests", AttestationVerdict::Fail, GATE_ACTOR, t0)],
            "a",
            t0,
        );
        assert!(!v.passed);
        assert!(v.failed[0].contains("tests exit 1"), "{:?}", v.failed);
        assert!(v.reason().contains("required by cra/x"), "{}", v.reason());
    }

    #[test]
    fn evidence_the_executing_agent_produced_itself_does_not_count() {
        let t0 = Utc::now();
        let v = judge(
            &[required("tests")],
            &[attest("tests", AttestationVerdict::Pass, "claude-code", t0)],
            "claude-code",
            t0,
        );
        assert!(!v.passed);
    }

    #[test]
    fn only_this_rounds_evidence_counts_and_the_newest_wins() {
        let t0 = Utc::now();
        let earlier = t0 - chrono::Duration::seconds(10);
        let stale_pass = vec![attest(
            "tests",
            AttestationVerdict::Pass,
            GATE_ACTOR,
            earlier,
        )];
        assert!(
            !judge(&[required("tests")], &stale_pass, "a", t0).passed,
            "an earlier round's pass is not this round's"
        );
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
