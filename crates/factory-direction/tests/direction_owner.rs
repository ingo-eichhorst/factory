//! Exercise the physical L6 owner directly, without Core or a skipped level.
use chrono::{TimeZone, Utc};
use factory_direction::{goals, goals_store::GoalsStore, policy, reporting_clock};
use factory_kernel::{ConfirmedSecurityReport, IntakeSource, SourceKind};
use std::collections::BTreeMap;

fn now() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
}

fn tempdir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "factory-direction-owner-{name}-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn authored_policy_is_reread_and_lower_projection_keeps_requirements_and_waivers() {
    let root = tempdir("policy");
    let dir = policy::policies_dir(&root);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("cra.yaml");
    let text = "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - id: tests\n    title: Tests\n    evidence: [{check: knowledge, tag: evidence/tests}]\n    requires:\n      - {applies_to: [release], step: tests, gate: cargo test}\n";
    std::fs::write(&path, text).unwrap();
    let (catalogues, findings) = policy::load_all(&dir);
    assert!(findings.is_empty(), "{findings:?}");
    let control = policy::ControlRef::new("cra", "tests");
    let chain = vec![policy::PolicyLayer {
        scope: "demo".into(),
        frameworks: vec!["cra".into()],
        tighten: BTreeMap::new(),
        not_applicable: vec![policy::NotApplicable {
            control: control.clone(),
            rationale: "No release product".into(),
        }],
    }];
    let (applied, findings) = policy::applicable(&catalogues, &chain);
    assert!(findings.is_empty(), "{findings:?}");
    let source = &policy::plan_sources(&applied)[0];
    assert_eq!(source.source, "cra/tests");
    assert_eq!(source.requires, applied[0].requires);
    assert_eq!(source.requires[0].gate.as_deref(), Some("cargo test"));
    assert_eq!(source.not_applicable.as_ref().unwrap().scope, "demo");
    assert_eq!(
        source.not_applicable.as_ref().unwrap().rationale,
        "No release product"
    );
    let subject = policy::evaluation_subject(&applied[0]);
    assert_eq!(subject.control, control);
    assert_eq!(subject.kind, policy::Kind::Regulation);
    assert_eq!(subject.evidence, applied[0].evidence);
    let statuses = policy::evaluate(&applied, &policy::Evidence::default(), now());
    assert_eq!(statuses[0].status.kind(), policy::StatusKind::NotApplicable);
    std::fs::write(&path, text.replace("title: Tests", "title: Revised")).unwrap();
    let (reloaded, _) = policy::load_all(&dir);
    assert_eq!(reloaded[0].controls[0].title, "Revised");
    assert_eq!(catalogues[0].controls[0].title, "Tests");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn counted_policy_and_best_practice_still_have_distinct_rollup_semantics() {
    let statuses = [
        policy::ControlStatus {
            control: policy::ControlRef::new("cra", "counted"),
            title: "Counted".into(),
            kind: policy::Kind::Regulation,
            refs: vec![],
            status: policy::Status::Satisfied { reasons: vec![] },
        },
        policy::ControlStatus {
            control: policy::ControlRef::new("cra", "optional"),
            title: "Optional".into(),
            kind: policy::Kind::BestPractice,
            refs: vec![],
            status: policy::Status::Open { reasons: vec![] },
        },
    ];
    let rollup = policy::rollup(&statuses);
    assert!(rollup[0].compliant);
    assert_eq!(rollup[0].counts.satisfied, 1);
    assert_eq!(rollup[0].best_practice.open, 1);
}

#[tokio::test]
async fn checkins_survive_reopen_and_score_the_authored_goal_without_task_state() {
    let root = tempdir("checkins");
    let path = root.join("goals.db");
    let cycle: goals::Cycle = serde_yaml_ng::from_str(
        "cycle: {id: autumn, from: 2026-10-01, to: 2026-12-31}\nobjectives:\n  - id: ship\n    title: Ship\n    key_results:\n      - id: evidence\n        title: Evidence\n        manual: true\n        kind: committed\n        baseline: 0\n        target: 10\n",
    ).unwrap();
    let store = GoalsStore::open(&path).unwrap();
    let checkin = goals::CheckIn {
        id: "c1".into(),
        kr: goals::KrRef::new("ship", "evidence"),
        value: 5.0,
        confidence: 7,
        note: None,
        by: "owner".into(),
        at: now(),
    };
    store.append(&checkin).await.unwrap();
    drop(store);
    let reopened = GoalsStore::open(&path).unwrap();
    let history = reopened.all().await.unwrap();
    assert_eq!(history, vec![checkin]);
    let scored = goals::evaluate(&cycle, &BTreeMap::new(), &history, now());
    assert_eq!(scored.objectives[0].key_results[0].score, Some(0.5));
    let unscored = goals::evaluate(&cycle, &BTreeMap::new(), &[], now());
    assert_eq!(unscored.objectives[0].key_results[0].score, None);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reporting_clock_consumes_kernel_facts_and_never_invents_final_report_evidence() {
    let report = ConfirmedSecurityReport {
        item: "incident".into(),
        scope: "demo".into(),
        source: IntakeSource {
            kind: SourceKind::Cli,
            external_id: None,
            reference: None,
            provider: None,
            relayed_by: None,
            repository: None,
            number: None,
        },
        parent: None,
        awareness_at: now(),
        confirmed_at: now(),
        confirmed_by: "owner".into(),
    };
    let clock = reporting_clock::compute(&[], &[report], &[], now());
    assert_eq!(clock.items.len(), 1);
    assert!(clock.items[0].corrective_measure.is_none());
    assert_eq!(clock.items[0].deadlines.len(), 2);
    assert_eq!(
        clock.items[0].deadlines[0].state,
        reporting_clock::ClockDeadlineState::Due
    );
}
