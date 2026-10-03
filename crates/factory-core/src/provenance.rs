//! Pure in-toto Statement v1 / SLSA provenance v1 construction.
//! Local unsigned evidence, not a claim to SLSA Build L2/L3 isolation.
use crate::{control_plan::StepAttestation, run::Run};
use chrono::{DateTime, Utc};
use factory_kernel::{
    ArtifactProvenance, ArtifactSnapshot, ProvenanceBuildDefinition, ProvenanceBuilder,
    ProvenanceDependency, ProvenanceMetadata, ProvenanceParameters, ProvenancePredicate,
    ProvenanceRunDetails, ProvenanceRunEvidence, ProvenanceStatement, ProvenanceSubject,
};

pub fn statement(
    run: &Run,
    artifact: &ArtifactSnapshot,
    attestations: &[StepAttestation],
    instance_id: &str,
    finished_at: DateTime<Utc>,
) -> ArtifactProvenance {
    ArtifactProvenance {
        id: artifact.id.clone(),
        run_id: run.id.clone(),
        task_id: run.task_id.clone(),
        scope: artifact.scope.clone(),
        artifact: artifact.clone(),
        statement: ProvenanceStatement {
            statement_type: "https://in-toto.io/Statement/v1".into(),
            subject: vec![ProvenanceSubject {
                name: artifact.name.clone(),
                digest: [("sha256".into(), artifact.sha256.clone())].into(),
            }],
            predicate_type: "https://slsa.dev/provenance/v1".into(),
            predicate: ProvenancePredicate {
                build_definition: ProvenanceBuildDefinition {
                    build_type: "https://github.com/ingo-eichhorst/factory/blob/main/docs/artifact-provenance.md#build-type-v1".into(),
                    external_parameters: ProvenanceParameters {
                        task_id: run.task_id.clone(), scope: artifact.scope.clone(), category: artifact.category.clone(),
                    },
                    resolved_dependencies: vec![ProvenanceDependency {
                        name: "source".into(),
                        digest: [("gitCommit".into(), artifact.source.commit.clone()),
                            ("factoryWorktreeSha256".into(), artifact.source.worktree_digest.clone())].into(),
                    }],
                },
                run_details: ProvenanceRunDetails {
                    builder: ProvenanceBuilder { id: format!("urn:factory:instance:{instance_id}") },
                    metadata: ProvenanceMetadata {
                        invocation_id: run.id.clone(), started_on: run.started_at, finished_on: finished_at,
                    },
                },
                evidence: ProvenanceRunEvidence {
                    agent: run.agent.clone(), adapter: run.adapter.clone(), runtime: run.runtime.clone(),
                    source: artifact.source.clone(), required_steps: run.required_steps.clone(),
                    attestations: attestations.to_vec(),
                },
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_kernel::ArtifactSource;

    #[test]
    fn provenance_schema_and_old_rows_remain_compatible_without_secrets() {
        let mut run: Run = serde_json::from_value(serde_json::json!({
            "id": "r1", "task_id": "t1", "attempt": 1, "status": "running",
            "trigger": "manual", "agent": "builder", "runtime": "quiet",
            "started_at": "2026-10-01T09:00:00Z"
        }))
        .unwrap();
        assert!(run.artifacts.is_empty());
        let old_report: crate::task::TaskReport =
            serde_json::from_str(r#"{"status":"done"}"#).unwrap();
        assert!(old_report.artifacts.is_empty());
        run.token = Some("never-serialize-callback-token".into());
        run.required_steps = vec![serde_json::from_value(serde_json::json!({
            "step": "review", "kind": "review", "actor": "independent", "required_by": ["opaque/framework.requirement"]
        })).unwrap()];
        let artifact = ArtifactSnapshot {
            id: "a1".into(),
            name: "release.tar".into(),
            scope: "demo".into(),
            category: "release".into(),
            sha256: "b".repeat(64),
            size_bytes: 100,
            storage_path: ".factory/artifacts/r1/a1/release.tar".into(),
            source_path: "/work/release.tar".into(),
            source: ArtifactSource {
                commit: "a".repeat(40),
                worktree_digest: "c".repeat(64),
                dirty: true,
            },
        };
        let finished = "2026-10-01T09:05:00Z".parse().unwrap();
        let record = statement(&run, &artifact, &[], "test", finished);
        let json = serde_json::to_value(&record).unwrap();
        let stmt = &json["statement"];
        assert_eq!(stmt["_type"], "https://in-toto.io/Statement/v1");
        assert_eq!(stmt["predicateType"], "https://slsa.dev/provenance/v1");
        assert_eq!(stmt["subject"][0]["digest"]["sha256"], artifact.sha256);
        assert_eq!(
            stmt["predicate"]["runDetails"]["metadata"]["invocationId"],
            "r1"
        );
        assert_eq!(
            record.statement.predicate.evidence.required_steps,
            run.required_steps
        );
        assert_eq!(
            record.statement.predicate.run_details.metadata.finished_on,
            finished
        );
        assert!(!json.to_string().contains("never-serialize-callback-token"));
        assert_eq!(
            serde_json::from_value::<ArtifactProvenance>(json).unwrap(),
            record
        );
    }
}
