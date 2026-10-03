//! The entire fact catalogue must be usable without any producing-level crate.
use factory_kernel::*;

fn assert_producer<F: Fact<Producer = P>, P: Level>(name: &str, producer: &str) {
    let entry = FACT_CATALOGUE
        .iter()
        .find(|entry| entry.fact == name)
        .unwrap();
    assert_eq!(entry.producer, producer);
    assert!(entry.lives_in_kernel);
    assert!(!entry.readers.is_empty());
}

#[test]
fn every_fact_is_in_l0_and_its_catalogue_producer_matches_its_type() {
    assert_producer::<DaemonConfigFact, L1>("DaemonConfigFact", "L1");
    assert_producer::<BackupFact, L1>("BackupFact", "L1");
    assert_producer::<SecretsPresence, L2>("SecretsPresence", "L2");
    assert_producer::<DependenciesFact, L2>("DependenciesFact", "L2");
    assert_producer::<ExploitedFinding, L2>("ExploitedFinding", "L2");
    assert_producer::<AgentFact, L3>("AgentFact", "L3");
    assert_producer::<TaskFact, L4>("TaskFact", "L4");
    assert_producer::<WorkflowFact, L4>("WorkflowFact", "L4");
    assert_producer::<ConfirmedSecurityReport, L4>("ConfirmedSecurityReport", "L4");
    assert_producer::<AttestedRun, L4>("AttestedRun", "L4");
    assert_producer::<GateFact, L5>("GateFact", "L5");
}

#[test]
fn catalogue_is_complete_unique_and_has_readers() {
    let mut expected = vec![
        "DaemonConfigFact",
        "BackupFact",
        "SecretsPresence",
        "DependenciesFact",
        "ExploitedFinding",
        "AgentFact",
        "TaskFact",
        "WorkflowFact",
        "ConfirmedSecurityReport",
        "AttestedRun",
        "GateFact",
    ];
    let mut actual: Vec<_> = FACT_CATALOGUE.iter().map(|entry| entry.fact).collect();
    actual.sort_unstable();
    expected.sort_unstable();
    assert_eq!(actual, expected);
    for entry in FACT_CATALOGUE {
        assert!(entry.lives_in_kernel);
        assert!(!entry.note.is_empty());
        for reader in entry.readers {
            let level = reader.split_whitespace().next().unwrap();
            assert!(["L1", "L2", "L3", "L4", "L5", "L6"].contains(&level));
        }
    }
}

#[test]
fn nested_fact_vocabulary_preserves_serialized_values_and_defaults() {
    use serde_json::{from_value, json, to_value};
    let run: TaskFact = from_value(json!({"id":"t", "title":"test", "runs":[{
        "id":"r", "status":"verifying", "started_at":"2026-09-29T00:00:00Z"
    }]}))
    .unwrap();
    assert_eq!(run.runs[0].status, RunStatus::Verifying);
    assert_eq!(to_value(&run).unwrap()["runs"][0]["status"], "verifying");
    assert!(to_value(&run).unwrap()["runs"][0].get("ended_at").is_none());
    let workflow: WorkflowFact = from_value(json!({"id":"w", "name":"scan"})).unwrap();
    assert_eq!(
        to_value(workflow).unwrap(),
        json!({"id":"w", "name":"scan"})
    );
    let gate: GateFact = from_value(json!({"run_id":"scan", "cases":[{
        "id":"one", "gated":true, "verdicts":["pass","unverified"]
    }]}))
    .unwrap();
    assert_eq!(
        gate.cases[0].verdicts,
        vec![BenchVerdict::Pass, BenchVerdict::Unverified]
    );
    assert_eq!(to_value(Grant::RunApprove).unwrap(), "run.approve");
    assert_eq!(to_value(LifecycleState::Running).unwrap(), "running");
    assert_eq!(to_value(Severity::High).unwrap(), "high");
    let report: ConfirmedSecurityReport = from_value(json!({"item":"i", "scope":"demo",
        "awareness_at":"2026-09-29T00:00:00Z", "source":{"kind":"github", "external_id":"id"},
        "confirmed_by":"owner", "confirmed_at":"2026-09-29T01:00:00Z"}))
    .unwrap();
    assert_eq!(report.source.kind, SourceKind::Github);
    assert_eq!(report.source.external_id.as_deref(), Some("id"));
    assert_eq!(report.parent, None);
    let step: StepAttestation = from_value(json!({"id":"a", "run_id":"r", "task_id":"t",
        "scope":"demo", "category":"feature", "step":"tests", "kind":"gate", "actor":"factory-daemon",
        "verdict":"pass", "dir":"/tmp/work", "at":"2026-09-29T01:00:00Z"})).unwrap();
    assert_eq!(step.round, 0);
    assert_eq!(step.verdict, AttestationVerdict::Pass);
    assert!(to_value(step).unwrap().get("round").is_none());
}

#[test]
fn a_new_fact_impl_cannot_be_left_out_of_the_catalogue() {
    let source = include_str!("../src/facts.rs");
    let mut declared: Vec<_> = source
        .lines()
        .filter_map(|line| {
            let rest = line.strip_prefix("impl Fact for ")?;
            Some(rest.split_whitespace().next().unwrap())
        })
        .collect();
    let mut catalogued: Vec<_> = FACT_CATALOGUE.iter().map(|entry| entry.fact).collect();
    declared.sort_unstable();
    catalogued.sort_unstable();
    assert_eq!(declared, catalogued);
}
