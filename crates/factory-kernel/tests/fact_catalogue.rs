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
    assert_producer::<ProductionFact, L4>("ProductionFact", "L4");
    assert_producer::<ProcessMetricFact, L4>("ProcessMetricFact", "L4");
    assert_producer::<BenchResolutionFact, L5>("BenchResolutionFact", "L5");
    assert_producer::<InfrastructureExpiryFact, L1>("InfrastructureExpiryFact", "L1");
    assert_producer::<RenewalDeclarationsFact, L1>("RenewalDeclarationsFact", "L1");
    assert_producer::<CredentialExpiryFact, L2>("CredentialExpiryFact", "L2");
    assert_producer::<ScheduledRunDatesFact, L4>("ScheduledRunDatesFact", "L4");
    assert_producer::<DaemonConfigFact, L1>("DaemonConfigFact", "L1");
    assert_producer::<ScopeCapacityFact, L1>("ScopeCapacityFact", "L1");
    assert_producer::<EnvironmentMetricFact, L1>("EnvironmentMetricFact", "L1");
    assert_producer::<BackupFact, L1>("BackupFact", "L1");
    assert_producer::<SecretsPresence, L2>("SecretsPresence", "L2");
    assert_producer::<DependenciesFact, L2>("DependenciesFact", "L2");
    assert_producer::<SandboxServiceEvidenceFact, L2>("SandboxServiceEvidenceFact", "L2");
    assert_producer::<ExploitedFinding, L2>("ExploitedFinding", "L2");
    assert_producer::<AgentFact, L3>("AgentFact", "L3");
    assert_producer::<TaskFact, L4>("TaskFact", "L4");
    assert_producer::<TaskInventoryFact, L4>("TaskInventoryFact", "L4");
    assert_producer::<WorkflowFact, L4>("WorkflowFact", "L4");
    assert_producer::<EnvironmentRecoveryFact, L4>("EnvironmentRecoveryFact", "L4");
    assert_producer::<RecoveryJournalFact, L4>("RecoveryJournalFact", "L4");
    assert_producer::<DeploymentMirrorFact, L4>("DeploymentMirrorFact", "L4");
    assert_producer::<DeploymentPublicationFact, L1>("DeploymentPublicationFact", "L1");
    assert_producer::<ReleaseBuildFact, L4>("ReleaseBuildFact", "L4");
    assert_producer::<ReleaseSbomFact, L2>("ReleaseSbomFact", "L2");
    assert_producer::<ConfirmedSecurityReport, L4>("ConfirmedSecurityReport", "L4");
    assert_producer::<AttestedRun, L4>("AttestedRun", "L4");
    assert_producer::<ArtifactProvenance, L4>("ArtifactProvenance", "L4");
    assert_producer::<CostReport, L4>("CostReport", "L4");
    assert_producer::<GateFact, L5>("GateFact", "L5");
    assert_producer::<KnowledgeTags, L5>("KnowledgeTags", "L5");
}

#[test]
fn task_inventory_is_plain_metadata_and_preserves_label_defaults() {
    let row: TaskInventoryFact = serde_json::from_value(serde_json::json!({
        "id": "task", "title": "standing intent", "scope": "demo", "open": true
    })).unwrap();
    assert!(row.labels.is_empty());
    let json = serde_json::to_value(&row).unwrap();
    assert!(json.get("labels").is_none());
    assert_eq!(serde_json::from_value::<TaskInventoryFact>(json).unwrap(), row);
    let labelled: TaskInventoryFact = serde_json::from_value(serde_json::json!({
        "id": "task", "title": "standing intent", "scope": "demo", "open": false,
        "labels": {"goal": "ship/kr1", "custom": "preserved"}
    })).unwrap();
    assert_eq!(serde_json::from_value::<TaskInventoryFact>(serde_json::to_value(&labelled).unwrap()).unwrap(), labelled);
}

#[test]
fn sandbox_service_evidence_roundtrips_in_l0_with_unknown_empty_defaults() {
    let empty: SandboxServiceEvidenceFact = serde_json::from_value(serde_json::json!({"scope": "demo"})).unwrap();
    assert!(empty.captures.is_empty() && empty.findings.is_empty());
    let fact: SandboxServiceEvidenceFact = serde_json::from_value(serde_json::json!({
        "scope": "demo", "captures": [{"id": "receipt", "run_id": "run", "task_id": "task", "agent": "curator",
            "sandbox": "factory-run", "captured_at": "2026-10-04T12:00:00Z", "source": "OpenShell supervisor/proxy OCSF", "partial": true,
            "accesses": [{"at": "2026-10-04T11:59:00Z", "transport": "network", "target": "api.github.com:443", "disposition": "denied"}]}]
    })).unwrap();
    assert_eq!(fact.captures[0].accesses[0].disposition, AccessDisposition::Denied);
    assert_eq!(serde_json::from_value::<SandboxServiceEvidenceFact>(serde_json::to_value(&fact).unwrap()).unwrap(), fact);
}

#[test]
fn renewal_fact_metadata_and_nested_dependencies_roundtrip_without_core() {
    let declaration: RenewalDecl = serde_json::from_value(serde_json::json!({
        "name": "token", "kind": "credential", "expires": "2027-10-04", "lead": "30d", "affects": ["demo/curator"]
    })).unwrap();
    let declared = RenewalDeclarationsFact { declarations: vec![ScopedRenewalDeclaration { scope: Some("demo".into()), declaration }], findings: vec![] };
    let json = serde_json::to_value(&declared).unwrap();
    assert_eq!(json["declarations"][0]["declaration"]["owner"], "owner");
    assert_eq!(serde_json::from_value::<RenewalDeclarationsFact>(json).unwrap(), declared);
    assert!(serde_json::from_value::<RenewalDecl>(serde_json::json!({"name": "bad", "kind": "credential", "secret": "forbidden"})).is_err());
    let observation = ExpiryObservation {
        id: "openshell:gateway:claude".into(), name: "Claude token".into(), kind: DateKind::Credential, scope: None,
        expires_at: Some("2027-10-04T00:00:00Z".parse().unwrap()), no_expiry: false, basis: DateBasis::Observed, source: DateSource::Openshell,
        detail: "expiry metadata".into(), observed_at: None, attempted_at: "2026-10-04T00:00:00Z".parse().unwrap(), issue: None,
        affects: vec![DateDependency { scope: Some("demo".into()), agent: Some("curator".into()), environment: None, provider: Some("claude".into()), label: "demo/curator".into() }],
        lead_seconds: 30 * 86400, renew: "rotate at source".into(), owner: "owner".into(),
    };
    let fact = CredentialExpiryFact { observations: vec![observation] };
    let json = serde_json::to_value(&fact).unwrap();
    assert_eq!(json["observations"][0]["affects"][0]["agent"], "curator");
    assert_eq!(serde_json::from_value::<CredentialExpiryFact>(json).unwrap(), fact);
}

#[test]
fn catalogue_is_complete_unique_and_has_readers() {
    let mut expected = vec![
        "ProductionFact", "ProcessMetricFact", "BenchResolutionFact",
        "InfrastructureExpiryFact", "CredentialExpiryFact", "ScheduledRunDatesFact",
        "RenewalDeclarationsFact",
        "DaemonConfigFact",
        "ScopeCapacityFact",
        "EnvironmentMetricFact",
        "BackupFact",
        "SecretsPresence",
        "DependenciesFact",
        "SandboxServiceEvidenceFact",
        "ExploitedFinding",
        "AgentFact",
        "TaskFact",
        "TaskInventoryFact",
        "WorkflowFact",
        "EnvironmentRecoveryFact",
        "RecoveryJournalFact",
        "DeploymentMirrorFact",
        "DeploymentPublicationFact",
        "ReleaseBuildFact",
        "ReleaseSbomFact",
        "ConfirmedSecurityReport",
        "AttestedRun",
        "ArtifactProvenance",
        "CostReport",
        "GateFact",
        "KnowledgeTags",
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
            assert!(["L1", "L2", "L3", "L4", "L5", "L6", "People"].contains(&level));
            if level != "People" {
                assert!(entry.producer < level, "{}: {} is not strictly below {level}", entry.fact, entry.producer);
            }
        }
    }
}

#[test]
fn production_and_measurements_preserve_plain_wire_defaults_and_unknowns() {
    let stamp = "2026-10-04T10:00:00Z";
    let production: ProductionFact = serde_json::from_value(serde_json::json!({
        "bin": "day", "from": stamp, "to": stamp, "buckets": [],
        "daily": [{"from": stamp, "to": stamp, "finished": 2,
            "scrapped": 1, "reworked": 1, "partial": true}]
    })).unwrap();
    assert!(production.earliest_run.is_none());
    assert_eq!(production.daily[0].first_pass, 0);
    let wire = serde_json::to_value(production).unwrap();
    assert!(wire.get("earliest_run").is_none());
    assert_eq!(wire["daily"][0]["first_pass"], 0);
    let measurement: ProcessMetricFact = serde_json::from_value(serde_json::json!({
        "name": "fail_rate", "value": null, "as_of": stamp, "reason": "no finished runs"
    })).unwrap();
    assert_eq!(measurement.value, None);
    let wire = serde_json::to_value(&measurement).unwrap();
    assert_eq!(serde_json::from_value::<ProcessMetricFact>(wire).unwrap(), measurement);
    let bench = BenchResolutionFact {
        dataset: "fixture".into(), run_id: "actual-run".into(),
        started_at: stamp.parse().unwrap(), ended_at: None, passed: 1, failed: 0,
    };
    let wire = serde_json::to_value(&bench).unwrap();
    assert_eq!(serde_json::from_value::<BenchResolutionFact>(wire).unwrap(), bench);
}

#[test]
fn old_cost_json_defaults_new_spend_history_and_scope_uncertainty() {
    let row = serde_json::to_value(CostRow::new("total", None)).unwrap();
    let report: CostReport = serde_json::from_value(serde_json::json!({
        "group_by": "scope", "from": "2026-10-01T00:00:00Z", "to": "2026-10-02T00:00:00Z",
        "rows": [], "total": row
    })).unwrap();
    assert_eq!(report.unattributed_runs, 0);
    assert!(report.daily.is_empty());
    let json = serde_json::to_value(report).unwrap();
    assert!(json.get("daily").is_none() && json.get("unattributed_runs").is_none());
}

#[test]
fn deployment_publication_and_append_only_receipts_roundtrip_in_l0() {
    let fact = DeploymentPublicationFact { deployment: "d".into(), scope: "demo".into(),
        environment: "prod".into(), commit: "a".repeat(40), dirty: false,
        state: "success".into(), verified: Some(true), repository: Some("owner/repo".into()),
        transient: false, production: true };
    let json = serde_json::to_value(&fact).unwrap();
    assert_eq!(serde_json::from_value::<DeploymentPublicationFact>(json).unwrap(), fact);
    let receipt = DeploymentMirrorFact { id: "receipt".into(), plan: DeploymentMirrorPlan {
        deployment: fact.deployment, scope: fact.scope, repository: fact.repository.unwrap(),
        environment: fact.environment, commit: fact.commit, state: fact.state, verified: fact.verified,
        transient: fact.transient, production: fact.production, approval: "b".repeat(64) },
        phase: DeploymentMirrorPhase::Published, at: "2026-10-04T00:00:00Z".parse().unwrap(),
        approved_by: "owner".into(), remote_id: Some(71), status_id: Some(81), error: None };
    let json = serde_json::to_value(&receipt).unwrap();
    assert_eq!(json["phase"], "published");
    assert_eq!(serde_json::from_value::<DeploymentMirrorFact>(json).unwrap(), receipt);
    assert_eq!(serde_json::to_value(Grant::DeployPublish).unwrap(), "deploy.publish");
    assert_eq!(serde_json::to_value(Grant::SecretsEdit).unwrap(), "secrets.edit");
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

#[test]
fn recovery_fact_preserves_actual_run_outcome_without_a_deployment_schema() {
    let json = serde_json::json!({
        "scope": "demo", "environment": "production", "workflow_id": "w", "workflow_run_id": "wr",
        "status": "running", "requested_at": "2026-10-04T00:00:00Z", "requested_by": "owner", "reason": "repair",
        "task_id": "t", "run": { "id": "r", "status": "failed", "started_at": "2026-10-04T00:01:00Z" }
    });
    let fact: EnvironmentRecoveryFact = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(fact.run.as_ref().unwrap().status, RunStatus::Failed);
    assert_eq!(serde_json::to_value(fact).unwrap(), json);
}

#[test]
fn offline_action_schema_keeps_an_absent_finish_unknown_and_rejects_fabricated_run_fields() {
    let json = serde_json::json!({ "actions": [{
        "id": "37646f22-22a3-4f20-904a-ad352e37dcbd", "scope": "demo", "environment": "prod",
        "source": "ensure.sh", "actor": "operator", "reason": "daemon stopped", "command": "restart installed",
        "started_at": "2026-10-04T00:00:00Z"
    }], "findings": [] });
    let fact: RecoveryJournalFact = serde_json::from_value(json.clone()).unwrap();
    assert!(fact.actions[0].finish.is_none());
    assert_eq!(serde_json::to_value(fact).unwrap(), json);
    let mut fabricated = json["actions"][0].clone();
    fabricated["run_status"] = serde_json::json!("done");
    assert!(serde_json::from_value::<ScriptRecoveryAction>(fabricated).is_err());
}

#[test]
fn release_evidence_facts_and_nested_attachment_schema_serialize_from_l0_alone() {
    let build = serde_json::json!({ "scope": "demo", "commit": "sha", "task_id": "t",
        "run": { "id": "r", "status": "done", "started_at": "2026-10-04T00:00:00Z" }, "artifacts": [], "attestations": [] });
    let fact: ReleaseBuildFact = serde_json::from_value(build.clone()).unwrap();
    assert_eq!(serde_json::to_value(fact).unwrap(), build);
    let sbom = serde_json::json!({ "scope": "demo", "commit": "sha", "version": "v1", "attachment": {
        "id": "a", "scope": "demo", "kind": "sbom", "run_id": "r", "task_id": "t", "attempt": 1,
        "attached_at": "2026-10-04T00:00:00Z", "filename": "build.cdx.json", "spec_version": "1.6", "states": ["built"]
    } });
    let fact: ReleaseSbomFact = serde_json::from_value(sbom.clone()).unwrap();
    assert_eq!(serde_json::to_value(fact).unwrap(), sbom);
}
