//! Owner-side contracts: these cannot import the upper compatibility facade.
use chrono::{DateTime, Utc};
use factory_assurance::checks::*;
use factory_kernel::{Attestation, AttestedRun, BenchVerdict, DependenciesFact, Withdrawal};
use std::collections::{BTreeMap, BTreeSet};

fn now() -> DateTime<Utc> {
    "2026-10-04T12:00:00Z".parse().unwrap()
}
fn subject(id: &str, checks: Vec<Check>) -> EvaluationSubject {
    EvaluationSubject {
        control: ControlRef::new("test", id),
        title: id.into(),
        kind: (),
        maps_to: vec![],
        evidence: checks,
        max_age: Some("1d".parse().unwrap()),
        not_applicable: None,
    }
}
fn result(check: Check, evidence: &Evidence) -> EvaluationResult {
    evaluate(&[subject("a", vec![check])], evidence, now()).remove(0)
}
fn receipt() -> Attestation {
    Attestation {
        id: "att-1".into(),
        control: ControlRef::new("test", "a"),
        scope: "demo".into(),
        evidence: "document".into(),
        note: None,
        attested_by: "owner".into(),
        attested_at: now(),
        expires_at: now() + chrono::Duration::days(1),
        withdrawn: None,
        clock: None,
        corrective: None,
    }
}
fn complete_evidence() -> Evidence {
    let month = factory_assurance::budget::Month::at(now()).unwrap();
    let required = serde_json::from_value(
        serde_json::json!({"step":"tests", "kind":"gate", "command":"true"}),
    )
    .unwrap();
    let attestation = serde_json::from_value(serde_json::json!({
        "id":"step-1", "run_id":"r", "task_id":"t", "scope":"demo", "category":"feature",
        "step":"tests", "kind":"gate", "actor":"reviewer", "verdict":"pass", "dir":"/tmp", "at":now()
    })).unwrap();
    Evidence {
        tags: BTreeSet::from(["control/test/a".into()]),
        attestations: vec![receipt()],
        tasks: BTreeMap::from([(
            "nightly".into(),
            vec![TaskFact {
                id: "t".into(),
                title: "nightly".into(),
                runs: vec![RunFact {
                    id: "r".into(),
                    status: factory_kernel::RunStatus::Done,
                    started_at: now(),
                    ended_at: Some(now()),
                }],
            }],
        )]),
        workflows: BTreeMap::from([(
            "release".into(),
            vec![WorkflowFact {
                id: "w".into(),
                name: "release".into(),
                runs: vec![WorkflowRunFact {
                    id: "wr".into(),
                    status: factory_kernel::WorkflowRunStatus::Done,
                    updated_at: now(),
                }],
            }],
        )]),
        gates: BTreeMap::from([(
            "demo".into(),
            GateFact {
                run_id: "bench".into(),
                ended_at: Some(now()),
                cases: vec![GateCase {
                    id: "gated".into(),
                    gated: true,
                    verdicts: vec![BenchVerdict::Pass],
                }],
            },
        )]),
        agents: Some(vec![]),
        secrets: BTreeMap::from([("scope_env".into(), false)]).into(),
        daemon: Some(DaemonFact {
            foreman_enabled: true,
            http_loopback_only: Some(true),
            power_assertion: true,
        }),
        dependencies: Some(DependenciesFact {
            declared_sbom_at: Some(now()),
            built_sbom_at: Some(now()),
            ..Default::default()
        }),
        attested: Some(vec![AttestedRun {
            run_id: "r".into(),
            task_id: "t".into(),
            scope: "demo".into(),
            category: "feature".into(),
            agent: "worker".into(),
            status: factory_kernel::RunStatus::Done,
            ended_at: now(),
            fail_kind: None,
            required_steps: vec![required],
            attestations: vec![attestation],
        }]),
        budget: Some(factory_assurance::budget::PolicyInput {
            month: month.clone(),
            error: None,
            caps: vec![factory_assurance::budget::PolicyCap {
                scope: "demo".into(),
                monthly_usd: 100.0,
                spend: factory_kernel::CostReport {
                    basis: factory_kernel::SpendBasis::Started,
                    finished: None,
                    group_by: factory_kernel::CostGroupBy::Scope,
                    from: month.from,
                    to: month.as_of,
                    scope: Some("demo".into()),
                    rows: vec![],
                    total: factory_kernel::CostRow::default(),
                    unattributed_runs: 0,
                    daily: vec![],
                },
            }],
        }),
        ..Default::default()
    }
}

#[test]
fn all_twelve_check_kinds_are_evaluated_by_the_owner() {
    let checks = [
        Check::Knowledge { tag: None },
        Check::Attestation,
        Check::Task {
            task: "nightly".into(),
            max_age: None,
        },
        Check::Workflow {
            workflow: "release".into(),
            max_age: None,
        },
        Check::Gate {
            dataset: "demo".into(),
            case: None,
            max_age: None,
        },
        Check::Roles { forbid: vec![] },
        Check::Sandbox,
        Check::Secrets { absent: vec![] },
        Check::Daemon {
            fact: "power_assertion".into(),
        },
        Check::Dependencies {
            sbom_max_age: Some("1d".parse().unwrap()),
            built_sbom: true,
            max_open: BTreeMap::new(),
            exploited_open: Some(0),
        },
        Check::Attested {
            category: "feature".into(),
            step: "tests".into(),
            max_age: "1d".parse().unwrap(),
        },
        Check::BudgetWithin,
    ];
    for check in checks {
        let empty = result(check.clone(), &Evidence::default());
        assert_eq!(
            empty.status.kind(),
            StatusKind::Open,
            "{}: {:?}",
            check.kind_name(),
            empty.status
        );
        let complete = result(check.clone(), &complete_evidence());
        assert_eq!(
            complete.status.kind(),
            if matches!(check, Check::Attestation) {
                StatusKind::Attested
            } else {
                StatusKind::Satisfied
            },
            "{}: {:?}",
            check.kind_name(),
            complete.status
        );
    }
}

#[test]
fn effective_freshness_and_newest_terminal_run_are_preserved() {
    let check = Check::Task {
        task: "nightly".into(),
        max_age: Some("30d".parse().unwrap()),
    };
    let mut evidence = complete_evidence();
    let runs = &mut evidence.tasks.get_mut("nightly").unwrap()[0].runs;
    runs[0].ended_at = Some(now() - chrono::Duration::days(2));
    runs.insert(
        0,
        RunFact {
            id: "running".into(),
            status: factory_kernel::RunStatus::Running,
            started_at: now(),
            ended_at: None,
        },
    );
    assert_eq!(
        result(check.clone(), &evidence).status.kind(),
        StatusKind::Stale,
        "the effective 1d, not the check's authored 30d"
    );
    let mut applied = subject("a", vec![check]);
    applied.max_age = None;
    assert_eq!(
        evaluate(&[applied], &evidence, now())[0].status.kind(),
        StatusKind::Satisfied
    );
    evidence.tasks.get_mut("nightly").unwrap()[0].runs[1].status =
        factory_kernel::RunStatus::Failed;
    assert_eq!(
        result(
            Check::Task {
                task: "nightly".into(),
                max_age: None
            },
            &evidence
        )
        .status
        .kind(),
        StatusKind::Open
    );
}

#[test]
fn withdrawn_clock_and_corrective_receipts_never_attest_the_whole_control() {
    for exclusion in 0..3 {
        let mut att = receipt();
        match exclusion {
            0 => {
                att.withdrawn = Some(Withdrawal {
                    at: now(),
                    by: "owner".into(),
                    reason: None,
                })
            }
            1 => {
                att.clock = Some(factory_kernel::ClockMark {
                    item: factory_kernel::ClockItemRef::Report {
                        item: "report".into(),
                    },
                    deadline: factory_kernel::ClockDeadlineKind::Notification,
                })
            }
            _ => {
                att.corrective = Some(factory_kernel::CorrectiveMeasureMark {
                    item: factory_kernel::ClockItemRef::Report {
                        item: "report".into(),
                    },
                    available_at: now(),
                })
            }
        }
        let out = result(
            Check::Attestation,
            &Evidence {
                attestations: vec![att],
                ..Default::default()
            },
        );
        assert_eq!(out.status.kind(), StatusKind::Open);
        assert!(out.refs.is_empty());
    }
    let mut att = receipt();
    att.expires_at = now();
    let out = result(
        Check::Attestation,
        &Evidence {
            attestations: vec![att],
            ..Default::default()
        },
    );
    assert_eq!(out.status.kind(), StatusKind::Stale);
    assert_eq!(out.refs, vec![EvidenceRef::attestation("att-1")]);
}

#[test]
fn mapping_is_symmetric_one_hop_deterministic_and_keeps_refs_and_metadata() {
    let mut a = subject(
        "a",
        vec![Check::Task {
            task: "nightly".into(),
            max_age: None,
        }],
    );
    let mut b = subject("b", vec![]);
    let c = subject("c", vec![]);
    a.maps_to = vec![b.control.clone()];
    b.maps_to = vec![c.control.clone()];
    let inputs = vec![c.clone(), b.clone(), a.clone()];
    let evaluated = evaluate(&inputs, &complete_evidence(), now());
    assert_eq!(
        evaluated
            .iter()
            .map(|x| x.control.id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    assert_eq!(
        evaluated
            .iter()
            .map(|x| x.status.kind())
            .collect::<Vec<_>>(),
        [
            StatusKind::Satisfied,
            StatusKind::Satisfied,
            StatusKind::Open
        ]
    );
    assert_eq!(
        evaluated[1].refs,
        vec![EvidenceRef::task("t"), EvidenceRef::run("r")]
    );
    assert_eq!(evaluated, evaluate(&[a, b, c], &complete_evidence(), now()));
    #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
    enum Opaque {
        OtherProducerValue,
    }
    let meta = EvaluationSubject {
        kind: Opaque::OtherProducerValue,
        control: ControlRef::new("meta", "a"),
        title: "meta".into(),
        maps_to: vec![],
        evidence: vec![],
        max_age: None,
        not_applicable: None,
    };
    let out = evaluate(&[meta], &Evidence::default(), now());
    assert_eq!(out[0].kind, Opaque::OtherProducerValue);
    let wire = serde_json::to_value(&out[0]).unwrap();
    assert_eq!(wire["control"], "meta/a");
    assert_eq!(wire["kind"], "OtherProducerValue");
    assert_eq!(wire["status"], "open");
    assert!(wire.get("refs").is_none());
}

#[test]
fn exemptions_and_ambiguous_targets_have_the_same_honest_status() {
    let mut input = subject("a", vec![Check::Knowledge { tag: None }]);
    input.not_applicable = Some(Exemption {
        scope: "parent".into(),
        rationale: "not in scope".into(),
    });
    let out = evaluate(&[input], &complete_evidence(), now());
    assert_eq!(
        out[0].status,
        Status::NotApplicable {
            reasons: vec!["marked not applicable at parent: not in scope".into()]
        }
    );
    let mut evidence = complete_evidence();
    let duplicate = evidence.tasks["nightly"][0].clone();
    evidence.tasks.get_mut("nightly").unwrap().push(duplicate);
    assert_eq!(
        result(
            Check::Task {
                task: "nightly".into(),
                max_age: None
            },
            &evidence
        )
        .status
        .kind(),
        StatusKind::Open
    );
    let findings = evidence_findings(&evidence, "demo");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].subject, "demo");
    assert!(findings[0].detail.contains("more than one task's title"));
}

#[test]
fn quality_uses_the_same_subject_for_gathering_and_evaluation() {
    use factory_assurance::quality::*;
    let profile: Profile = serde_yaml_ng::from_str("attributes:\n  - id: reliability.availability\n    importance: H\n    difficulty: M\n    scenarios:\n      - id: nightly\n        measure: { check: task, task: nightly, max_age: 1d }\n").unwrap();
    let catalogue = QualityCatalogue {
        profiles: BTreeMap::from([("demo".into(), profile)]),
        findings: vec![],
    };
    let (tree, findings) = applicable(
        &catalogue,
        "demo",
        &[QualityLayer {
            scope: "demo".into(),
            profiles: vec!["demo".into()],
        }],
    );
    assert!(findings.is_empty(), "{findings:?}");
    let subjects = check_subjects(&tree);
    assert_eq!(subjects.len(), 1);
    let checks = factory_assurance::checks::evaluate(&subjects, &complete_evidence(), now());
    assert_eq!(checks[0].status.kind(), StatusKind::Satisfied);
    let report =
        factory_assurance::quality::evaluate(&tree, &BTreeMap::new(), &complete_evidence(), now());
    assert_eq!(
        report.attributes[0].scenarios[0].status,
        ScenarioStatus::Met
    );
    assert_eq!(report.attributes[0].scenarios[0].refs, checks[0].refs);
    let empty =
        factory_assurance::quality::evaluate(&tree, &BTreeMap::new(), &Evidence::default(), now());
    assert_eq!(
        empty.attributes[0].scenarios[0].status,
        ScenarioStatus::NoData
    );
}
