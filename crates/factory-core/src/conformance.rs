//! `#158`: what a run's attestations say about whether it conformed
//! to its own control plan, shared by the `attested` policy check
//! (`policy::Check::Attested`) and the conformance_rate.<category>,
//! gate_fail_rate and review_reject_rate registry metrics -- one pure place that reuses
//! `control_plan::judge` rather than re-deriving "did this run pass" a
//! second time.
//!
//! [`AttestedRun`] is resolved once, by the daemon
//! (`Engine::attested_runs` in `factory-daemon/src/verification.rs`), from
//! a finished run's own `required_steps` (frozen at dispatch,
//! `Run::required_steps`) and every attestation it collected
//! (`PolicyStore::step_attestations_for`) -- a batch read shared by policy
//! evidence gathering and the metrics, so a run is never judged twice by
//! two different code paths. Nothing here touches a store, a clock, or
//! `factory_core::policy` -- `policy.rs` is the one caller that turns this
//! module's output into a `Check::Attested` status, so this stays free of
//! that dependency rather than the two modules importing each other.

use crate::control_plan::{self, AttestationVerdict, StepAttestation, StepKind};
use crate::run::FailKind;
#[cfg(test)]
use crate::run::RunStatus;
use chrono::{DateTime, Utc};

pub use factory_kernel::AttestedRun;

/// One enforced step's own evidence, from the newest attestation for it by
/// anyone other than the run's own agent -- the same filter
/// `control_plan::judge` applies per step, folded into one verdict there;
/// this keeps the per-step answer so a caller (the `attested` check) can
/// name which step, and which run, is missing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepEvidence {
    Passed,
    Failed,
    /// No attestation from anyone but the run's own agent -- whether
    /// because nothing has attested the step yet, or because the run's own
    /// control plan never required it at all. [`ConformanceEvidence::holds`]
    /// distinguishes the two for a caller that needs to say which.
    Missing,
}

/// Conformance evaluation remains with its evaluator, not in L0.
pub trait ConformanceEvidence {
    fn holds(&self, step: &str) -> bool;
    fn step_evidence(&self, step: &str) -> StepEvidence;
    fn held(&self) -> bool;
    fn is_infrastructure_failure(&self) -> bool;
    fn conforms(&self) -> bool;
}
impl ConformanceEvidence for AttestedRun {
    /// Whether `step` was ever named in this run's own `required_steps` --
    /// enforced or not. A caller building a reason for [`StepEvidence::Missing`]
    /// uses this to say "never held to it" instead of "no evidence yet".
    fn holds(&self, step: &str) -> bool {
        self.required_steps.iter().any(|s| s.step == step)
    }

    /// This run's own verdict for one step: the newest attestation for it
    /// by someone other than this run's own agent, the same "newest wins,
    /// self-attested never counts" rule [`Self::conforms`] applies to the
    /// whole run.
    fn step_evidence(&self, step: &str) -> StepEvidence {
        let required = self.required_steps.iter().find(|s| s.step == step);
        let newest = self
            .attestations
            .iter()
            .filter(|a| a.step == step && a.actor != self.agent
                && required.and_then(|s| s.actor.as_deref()).is_none_or(|actor| actor == a.actor))
            .max_by_key(|a| a.at);
        match newest {
            Some(a) if a.verdict == AttestationVerdict::Pass => StepEvidence::Passed,
            Some(_) => StepEvidence::Failed,
            None => StepEvidence::Missing,
        }
    }

    /// Whether this run holds at least one enforced gate, review or approval.
    /// A run with nothing required never counts
    /// for or against `conformance_rate`: nothing was ever demanded of it.
    fn held(&self) -> bool {
        self.required_steps.iter().any(|s| s.kind.enforced())
    }

    /// Whether this run's own failure, if it has one, never reached an
    /// agent at all (`FailKind::is_infrastructure`: an ack timeout, a run
    /// timeout, a vanished session, or a dispatch that never happened).
    /// `conformance_rate` excludes a run like this from both sides of its
    /// ratio -- it produced no work to conform or not, so counting it
    /// against the rate would turn a conformance metric into a reliability
    /// one. A person's or an agent's own cancellation, and an agent-reported
    /// failure, are not infrastructure and still count against it.
    fn is_infrastructure_failure(&self) -> bool {
        self.fail_kind.is_some_and(FailKind::is_infrastructure)
    }

    /// The `done` gate's own verdict for the whole run, reused rather than
    /// re-derived: `control_plan::judge` with `since` at the dawn of time,
    /// so a step attested in *any* verification round still counts, not
    /// only the round nearest to now -- a re-verification only ever adds
    /// attestations, it never withdraws what an earlier round already left.
    fn conforms(&self) -> bool {
        control_plan::judge(
            &self.required_steps,
            &self.attestations,
            &self.agent,
            DateTime::<Utc>::MIN_UTC,
        )
        .passed
    }
}

/// One registry metric's figure, computed off a slice of [`AttestedRun`] --
/// `value: None` with `reason: Some` for an empty denominator, the same
/// shape `factory_core::operations::Figure` uses, kept as its own small
/// type here rather than reused: `Figure` carries a `samples: u32` this
/// module has no use for, and its constructors are private to `operations.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct ConformanceFigure {
    pub value: Option<f64>,
    /// The newest `ended_at` among the runs the value is actually computed
    /// from -- `None` exactly when `value` is, for the same reason
    /// `operations::registry_metric_as_of` keeps its own answer separate
    /// from the figure: the daemon falls back to "now" only when there
    /// really is no data behind the number.
    pub as_of: Option<DateTime<Utc>>,
    pub reason: Option<String>,
}

/// `conformance_rate.<category>`'s figure: `conforms()` runs over `held()`
/// runs, among `runs` of `category` -- `runs` is expected already narrowed
/// to finished runs of the scope subtree and window a caller asked about
/// (`Engine::attested_runs`); this only filters by category, by `held()`,
/// and by [`ConformanceEvidence::is_infrastructure_failure`], never by scope or
/// time. A held run that was cancelled by a person or an agent, or that the
/// agent itself reported failed, without passing evidence, still counts
/// against the rate -- declared but not shown to be met is not met, the
/// same rule `quality::quality_def` states for its own share. A held run
/// that failed on infrastructure -- an ack timeout, a run timeout, a
/// vanished session, a dispatch that never happened -- is excluded from
/// both the numerator and the denominator: it never reached an agent, so it
/// says nothing about whether the work conforms. `None`, with a reason,
/// when nothing held is left -- an unknown category, a known one nothing
/// has run yet, or one where every held run failed on infrastructure.
pub fn conformance_rate(runs: &[AttestedRun], category: &str) -> ConformanceFigure {
    let held: Vec<&AttestedRun> = runs
        .iter()
        .filter(|r| r.category == category && r.held() && !r.is_infrastructure_failure())
        .collect();
    if held.is_empty() {
        return ConformanceFigure {
            value: None,
            as_of: None,
            reason: Some(
                "no run held to a required step of this category in the window".to_string(),
            ),
        };
    }
    let conforming = held.iter().filter(|r| r.conforms()).count();
    let as_of = held.iter().map(|r| r.ended_at).max();
    ConformanceFigure {
        value: Some(conforming as f64 / held.len() as f64),
        as_of,
        reason: None,
    }
}

/// `gate_fail_rate`'s figure: failed gate attestations over every gate
/// attestation on `runs`, across every category -- re-verification rounds
/// each count on their own, since the andon only ever stops a round at its
/// first failure and a later round's own attestations are new evidence,
/// not a correction of the first. Never filtered by actor: v1 attests a
/// gate as [`control_plan::GATE_ACTOR`] alone, never the run's own agent,
/// so the self-attested exclusion `step_evidence`/`conforms` apply has
/// nothing to exclude here. `None`, with a reason, when nothing in `runs`
/// ever attested a gate.
pub fn gate_fail_rate(runs: &[AttestedRun]) -> ConformanceFigure {
    let mut total = 0u32;
    let mut failed = 0u32;
    let mut as_of: Option<DateTime<Utc>> = None;
    for run in runs {
        let gates: Vec<&StepAttestation> = run
            .attestations
            .iter()
            .filter(|a| a.kind == StepKind::Gate)
            .collect();
        if gates.is_empty() {
            continue;
        }
        total += gates.len() as u32;
        failed += gates
            .iter()
            .filter(|a| a.verdict == AttestationVerdict::Fail)
            .count() as u32;
        as_of = Some(as_of.map_or(run.ended_at, |cur| cur.max(run.ended_at)));
    }
    if total == 0 {
        return ConformanceFigure {
            value: None,
            as_of: None,
            reason: Some("no gate attestation among the selected runs".to_string()),
        };
    }
    ConformanceFigure {
        value: Some(f64::from(failed) / f64::from(total)),
        as_of,
        reason: None,
    }
}

/// Failed independent review decisions over all review decisions, across
/// finished runs already narrowed to the requested scope/window. Each
/// (step, round) counts once; a duplicate's newest decision wins. Approval,
/// gates, self-review and evidence from anyone but the frozen reviewer do
/// not count. A missing review is unknown, not an invented rejection.
pub fn review_reject_rate(runs: &[AttestedRun]) -> ConformanceFigure {
    let mut total = 0usize;
    let mut failed = 0usize;
    let mut as_of = None;
    for run in runs {
        let mut rounds = std::collections::BTreeMap::new();
        for a in run.attestations.iter().filter(|a| {
            a.kind == StepKind::Review
                && a.actor != run.agent
                && run.required_steps.iter().any(|s| {
                    s.step == a.step
                        && s.kind == StepKind::Review
                        && s.actor.as_deref().is_none_or(|actor| actor == a.actor)
                })
        }) {
            let key = (&a.step, a.round);
            let current: &mut &StepAttestation = rounds.entry(key).or_insert(a);
            if (a.at, &a.id) > (current.at, &current.id) {
                *current = a;
            }
        }
        if rounds.is_empty() {
            continue;
        }
        total += rounds.len();
        failed += rounds
            .values()
            .filter(|a| a.verdict == AttestationVerdict::Fail)
            .count();
        as_of = Some(as_of.map_or(run.ended_at, |at: DateTime<Utc>| at.max(run.ended_at)));
    }
    ConformanceFigure {
        value: (total > 0).then(|| failed as f64 / total as f64),
        as_of,
        reason: (total == 0)
            .then(|| "no independent review decision among the selected runs".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plan::RequiredStep;

    fn step(name: &str, kind: StepKind) -> RequiredStep {
        RequiredStep {
            step: name.into(),
            kind,
            command: Some("true".into()),
            timeout_seconds: None,
            required_by: vec![],
            node_id: None,
            by: None,
            actor: None,
        }
    }

    fn attest(
        step_name: &str,
        verdict: AttestationVerdict,
        actor: &str,
        at: DateTime<Utc>,
    ) -> StepAttestation {
        StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: "r1".into(),
            task_id: "t1".into(),
            scope: "demo".into(),
            category: "feature".into(),
            step: step_name.into(),
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

    fn run(
        status: RunStatus,
        category: &str,
        required: Vec<RequiredStep>,
        attestations: Vec<StepAttestation>,
        ended_at: DateTime<Utc>,
    ) -> AttestedRun {
        AttestedRun {
            run_id: "r1".into(),
            task_id: "t1".into(),
            scope: "demo".into(),
            category: category.into(),
            agent: "worker".into(),
            status,
            ended_at,
            fail_kind: None,
            required_steps: required,
            attestations,
        }
    }

    #[test]
    fn step_evidence_reads_passed_failed_and_missing_and_ignores_self_attestation() {
        let t0 = Utc::now();
        let r = run(
            RunStatus::Done,
            "feature",
            vec![
                step("tests", StepKind::Gate),
                step("lint", StepKind::Gate),
                step("sbom", StepKind::Gate),
            ],
            vec![
                attest(
                    "tests",
                    AttestationVerdict::Pass,
                    control_plan::GATE_ACTOR,
                    t0,
                ),
                attest(
                    "lint",
                    AttestationVerdict::Fail,
                    control_plan::GATE_ACTOR,
                    t0,
                ),
                attest("sbom", AttestationVerdict::Pass, "worker", t0),
            ],
            t0,
        );
        assert_eq!(r.step_evidence("tests"), StepEvidence::Passed);
        assert_eq!(r.step_evidence("lint"), StepEvidence::Failed);
        assert_eq!(
            r.step_evidence("sbom"),
            StepEvidence::Missing,
            "self-attested never counts"
        );
        assert_eq!(r.step_evidence("never-required"), StepEvidence::Missing);
        assert!(r.holds("sbom"));
        assert!(!r.holds("never-required"));
    }

    #[test]
    fn step_evidence_and_conformance_require_the_frozen_review_functionary() {
        let t0 = Utc::now();
        let mut review = step("review", StepKind::Review);
        review.actor = Some("checker".into());
        let mut r = run(RunStatus::Done, "feature", vec![review], vec![
            attest("review", AttestationVerdict::Pass, "other", t0),
        ], t0);
        assert_eq!(r.step_evidence("review"), StepEvidence::Missing);
        assert!(!r.conforms());
        let mut evidence = attest("review", AttestationVerdict::Pass, "checker", t0);
        evidence.kind = StepKind::Review;
        r.attestations.push(evidence);
        assert_eq!(r.step_evidence("review"), StepEvidence::Passed);
        assert!(r.conforms());
    }

    #[test]
    fn step_evidence_takes_the_newest_attestation_across_re_verification_rounds() {
        let t0 = Utc::now();
        let r = run(
            RunStatus::Done,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![
                attest(
                    "tests",
                    AttestationVerdict::Fail,
                    control_plan::GATE_ACTOR,
                    t0,
                ),
                attest(
                    "tests",
                    AttestationVerdict::Pass,
                    control_plan::GATE_ACTOR,
                    t0 + chrono::Duration::seconds(1),
                ),
            ],
            t0,
        );
        assert_eq!(r.step_evidence("tests"), StepEvidence::Passed);
    }

    #[test]
    fn conforms_matches_judge_across_re_verification_rounds() {
        let t0 = Utc::now();
        let mut passing = run(
            RunStatus::Done,
            "feature",
            vec![
                step("tests", StepKind::Gate),
                step("review", StepKind::Review),
            ],
            vec![attest(
                "tests",
                AttestationVerdict::Pass,
                control_plan::GATE_ACTOR,
                t0,
            )],
            t0,
        );
        assert!(!passing.conforms(), "a missing independent review blocks conformance");
        let mut review = attest("review", AttestationVerdict::Pass, "checker", t0);
        review.kind = StepKind::Review;
        passing.attestations.push(review);
        assert!(passing.conforms(), "gates and independent review both passed");

        let failing = run(
            RunStatus::Done,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![attest(
                "tests",
                AttestationVerdict::Fail,
                control_plan::GATE_ACTOR,
                t0,
            )],
            t0,
        );
        assert!(!failing.conforms());
    }

    #[test]
    fn held_is_true_only_with_an_enforced_step() {
        let t0 = Utc::now();
        assert!(!run(RunStatus::Done, "feature", vec![], vec![], t0).held());
        assert!(run(
            RunStatus::Done,
            "feature",
            vec![step("review", StepKind::Review)],
            vec![],
            t0
        )
        .held());
        assert!(run(
            RunStatus::Done, "release", vec![step("approval", StepKind::Approval)], vec![], t0,
        ).held());
        assert!(run(
            RunStatus::Done,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![],
            t0
        )
        .held());
    }

    #[test]
    fn conformance_rate_counts_conforming_over_held_and_leaves_unheld_runs_out() {
        let t0 = Utc::now();
        let conforming = run(
            RunStatus::Done,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![attest(
                "tests",
                AttestationVerdict::Pass,
                control_plan::GATE_ACTOR,
                t0,
            )],
            t0,
        );
        // A held run that was cancelled without passing evidence -- the
        // assessment's own rule -- still counts against the rate; it is
        // not simply left out the way an unheld run is.
        let cancelled_without_evidence = run(
            RunStatus::Cancelled,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![],
            t0,
        );
        let unheld = run(RunStatus::Done, "feature", vec![], vec![], t0);
        let other_category = run(
            RunStatus::Done,
            "bugfix",
            vec![step("tests", StepKind::Gate)],
            vec![attest(
                "tests",
                AttestationVerdict::Pass,
                control_plan::GATE_ACTOR,
                t0,
            )],
            t0,
        );
        let runs = vec![conforming, cancelled_without_evidence, unheld, other_category];
        let figure = conformance_rate(&runs, "feature");
        assert_eq!(
            figure.value,
            Some(0.5),
            "one of two held feature runs conforms; the unheld run and the other category are left out"
        );
        assert_eq!(figure.as_of, Some(t0));
    }

    #[test]
    fn conformance_rate_excludes_infrastructure_failures_but_still_counts_an_agent_failure() {
        let t0 = Utc::now();
        let conforming = run(
            RunStatus::Done,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![attest(
                "tests",
                AttestationVerdict::Pass,
                control_plan::GATE_ACTOR,
                t0,
            )],
            t0,
        );
        let mut dispatch_failed = run(
            RunStatus::Failed,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![],
            t0,
        );
        dispatch_failed.fail_kind = Some(FailKind::DispatchFailed);
        let mut ack_timeout = run(
            RunStatus::Failed,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![],
            t0,
        );
        ack_timeout.fail_kind = Some(FailKind::AckTimeout);
        let mut agent_failed = run(
            RunStatus::Failed,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![],
            t0,
        );
        agent_failed.fail_kind = Some(FailKind::AgentFailed);

        // Infrastructure failures alone: nothing held is left.
        let figure = conformance_rate(&[dispatch_failed.clone(), ack_timeout.clone()], "feature");
        assert_eq!(
            figure.value, None,
            "a dispatch failure and an ack timeout never reached an agent"
        );
        assert!(figure.reason.is_some());

        // Mixed with a conforming run: the infrastructure failures are
        // simply absent from the denominator, not counted against it.
        let figure = conformance_rate(
            &[conforming.clone(), dispatch_failed, ack_timeout],
            "feature",
        );
        assert_eq!(
            figure.value,
            Some(1.0),
            "the one held, non-infrastructure run conforms"
        );

        // An agent-reported failure is not infrastructure and still counts
        // against the rate, exactly as a cancellation does.
        let figure = conformance_rate(&[conforming, agent_failed], "feature");
        assert_eq!(
            figure.value,
            Some(0.5),
            "the agent failure counts against the rate, unlike the infrastructure ones"
        );
    }

    #[test]
    fn conformance_rate_is_none_with_a_reason_for_an_empty_denominator() {
        let figure = conformance_rate(&[], "feature");
        assert_eq!(figure.value, None);
        assert!(figure.reason.is_some());
    }

    #[test]
    fn gate_fail_rate_counts_every_round_across_every_category() {
        let t0 = Utc::now();
        let a = run(
            RunStatus::Done,
            "feature",
            vec![step("tests", StepKind::Gate)],
            vec![
                attest(
                    "tests",
                    AttestationVerdict::Fail,
                    control_plan::GATE_ACTOR,
                    t0,
                ),
                attest(
                    "tests",
                    AttestationVerdict::Pass,
                    control_plan::GATE_ACTOR,
                    t0 + chrono::Duration::seconds(1),
                ),
            ],
            t0,
        );
        let b = run(
            RunStatus::Done,
            "bugfix",
            vec![step("tests", StepKind::Gate)],
            vec![attest(
                "tests",
                AttestationVerdict::Pass,
                control_plan::GATE_ACTOR,
                t0,
            )],
            t0 + chrono::Duration::seconds(2),
        );
        let figure = gate_fail_rate(&[a, b]);
        assert_eq!(
            figure.value,
            Some(1.0 / 3.0),
            "one failed round of three gate attestations, re-verification included"
        );
        assert_eq!(figure.as_of, Some(t0 + chrono::Duration::seconds(2)));
    }

    #[test]
    fn gate_fail_rate_is_none_with_a_reason_when_nothing_attested_a_gate() {
        let figure = gate_fail_rate(&[]);
        assert_eq!(figure.value, None);
        assert!(figure.reason.is_some());
    }

    #[test]
    fn review_reject_rate_counts_each_round_once_and_only_the_frozen_independent_reviewer() {
        let now = Utc::now();
        let mut required = step("review", StepKind::Review);
        required.actor = Some("reviewer".into());
        let make = |actor: &str, round, verdict, seconds| {
            let mut a = attest(
                "review",
                verdict,
                actor,
                now + chrono::Duration::seconds(seconds),
            );
            a.kind = StepKind::Review;
            a.round = round;
            a
        };
        let r = run(
            RunStatus::Failed,
            "feature",
            vec![required],
            vec![
                make("reviewer", 0, AttestationVerdict::Fail, 0),
                make("reviewer", 1, AttestationVerdict::Fail, 1),
                make("reviewer", 1, AttestationVerdict::Pass, 2),
                make("worker", 2, AttestationVerdict::Fail, 3),
                make("stranger", 3, AttestationVerdict::Fail, 4),
                attest("tests", AttestationVerdict::Fail, "gate", now),
            ],
            now,
        );
        let figure = review_reject_rate(&[r]);
        assert_eq!(
            figure.value,
            Some(0.5),
            "one failed and one passed round; self/foreign/gate evidence ignored"
        );
        assert_eq!(figure.as_of, Some(now));
        assert!(figure.reason.is_none());
    }

    #[test]
    fn review_reject_rate_is_unknown_without_independent_reviews_not_zero_or_failure() {
        let now = Utc::now();
        let r = run(
            RunStatus::Cancelled,
            "feature",
            vec![step("review", StepKind::Review)],
            vec![],
            now,
        );
        let figure = review_reject_rate(&[r]);
        assert!(figure.value.is_none());
        assert!(figure.as_of.is_none());
        assert!(figure
            .reason
            .unwrap()
            .contains("no independent review decision"));
    }
}
