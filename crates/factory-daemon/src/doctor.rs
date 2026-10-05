//! L1 Doctor's deliberately narrow v1 projection: compare the newest Factory
//! build evidence with the installed binaries' evidence. Scanners stay
//! outside the daemon; this module only reads immutable dependency reports.

use crate::engine::Engine;
use factory_core::dependencies::{DependenciesReport, DoctorReport, DoctorStatus, LifecycleState};
use factory_core::{FactoryError, Result};

const FACTORY_SCOPE: &str = "factory";

pub(crate) fn project(
    report: DependenciesReport,
    now: chrono::DateTime<chrono::Utc>,
) -> DoctorReport {
    let built = report
        .documents
        .iter()
        .find(|document| document.state == LifecycleState::Built)
        .cloned();
    let running = report
        .documents
        .iter()
        .find(|document| document.state == LifecycleState::Running)
        .cloned();
    let status = match (
        built
            .as_ref()
            .and_then(|document| document.sbom.identity.as_ref()),
        running
            .as_ref()
            .and_then(|document| document.sbom.identity.as_ref()),
    ) {
        (Some(built), Some(running)) if built == running => DoctorStatus::Current,
        (Some(_), Some(_)) => DoctorStatus::Behind,
        _ => DoctorStatus::Missing,
    };
    let findings = report
        .findings
        .into_iter()
        .filter(|finding| finding.state == LifecycleState::Running)
        .collect();
    DoctorReport {
        now,
        status,
        built,
        running,
        findings,
        openshell: Vec::new(),
        openshell_image_failures: Vec::new(),
    }
}

impl Engine {
    pub(crate) async fn doctor_report(&self) -> Result<DoctorReport> {
        let dependencies = match self.dependencies_report(FACTORY_SCOPE).await {
            Ok(report) => report,
            // An instance need not host Factory's source scope. That is
            // missing evidence, not a broken Doctor endpoint.
            Err(FactoryError::NoSuchScope(_)) => DependenciesReport {
                scope: FACTORY_SCOPE.into(),
                scan_workflow: None,
                max_age: None,
                documents: Vec::new(),
                findings: Vec::new(),
                services: Vec::new(),
                service_evidence: None,
            },
            Err(error) => return Err(error),
        };
        let mut report = project(dependencies, chrono::Utc::now());
        // The OpenShell gateways sandboxed agents use, as the provisioner
        // last found them -- and whether it had to start one (`#234`).
        report.openshell = self.provision.gateway_rows();
        report.openshell_image_failures = self.provision.image_failures();
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use factory_core::dependencies::{
        product_identity, AffectedComponent, Attachment, AttachmentKind, DependencyFinding,
        DocumentSummary, FindingStatus, LifecycleDocuments, ProductIdentity, Severity,
    };

    fn document(state: LifecycleState, fixture: &[u8], id: &str) -> LifecycleDocuments {
        let json: serde_json::Value = serde_json::from_slice(fixture).unwrap();
        let at = DateTime::parse_from_rfc3339("2026-09-25T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        LifecycleDocuments {
            state,
            sbom: DocumentSummary {
                attachment: Attachment {
                    id: id.into(),
                    kind: AttachmentKind::Sbom,
                    scope: "factory".into(),
                    run_id: format!("run-{id}"),
                    task_id: "task".into(),
                    attempt: 1,
                    attached_at: at,
                    filename: format!("{id}.cdx.json"),
                    spec_version: "1.6".into(),
                    states: vec![state],
                },
                tool: Some("syft".into()),
                tool_version: Some("1.30.0".into()),
                scan_time: Some(at),
                identity: product_identity(&json),
            },
            vulnerabilities: None,
        }
    }

    fn report(documents: Vec<LifecycleDocuments>) -> DependenciesReport {
        DependenciesReport {
            scope: "factory".into(),
            scan_workflow: Some("dependency-scan".into()),
            max_age: None,
            documents,
            findings: Vec::new(),
            services: Vec::new(),
            service_evidence: None,
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-25T14:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    const BUILT: &[u8] =
        include_bytes!("../../factory-environment/tests/fixtures/dependencies/build-sbom.cdx.json");
    const RUNNING: &[u8] =
        include_bytes!("../../factory-environment/tests/fixtures/dependencies/operations-sbom.cdx.json");

    #[test]
    fn matching_build_and_running_identity_is_current() {
        let answer = project(
            report(vec![
                document(LifecycleState::Built, BUILT, "built"),
                document(LifecycleState::Running, RUNNING, "running"),
            ]),
            now(),
        );
        assert_eq!(answer.status, DoctorStatus::Current);
        assert_eq!(
            answer.built.unwrap().sbom.identity,
            answer.running.unwrap().sbom.identity
        );
    }

    #[test]
    fn a_different_version_or_commit_is_behind() {
        let built = document(LifecycleState::Built, BUILT, "built");
        let mut running = document(LifecycleState::Running, RUNNING, "running");
        running.sbom.identity = Some(ProductIdentity {
            version: "0.1.0".into(),
            git_sha: "different".into(),
        });
        assert_eq!(
            project(report(vec![built.clone(), running]), now()).status,
            DoctorStatus::Behind
        );

        let mut running = document(LifecycleState::Running, RUNNING, "running-2");
        running.sbom.identity.as_mut().unwrap().version = "0.2.0".into();
        assert_eq!(
            project(report(vec![built, running]), now()).status,
            DoctorStatus::Behind
        );
    }

    #[test]
    fn either_missing_lifecycle_or_identity_is_missing() {
        let built = document(LifecycleState::Built, BUILT, "built");
        let running = document(LifecycleState::Running, RUNNING, "running");
        assert_eq!(
            project(report(vec![built.clone()]), now()).status,
            DoctorStatus::Missing
        );
        assert_eq!(
            project(report(vec![running]), now()).status,
            DoctorStatus::Missing
        );

        let mut unidentified = built;
        unidentified.sbom.identity = None;
        assert_eq!(
            project(report(vec![unidentified]), now()).status,
            DoctorStatus::Missing
        );
    }

    #[test]
    fn only_running_findings_reach_doctor() {
        let running = document(LifecycleState::Running, RUNNING, "running");
        let scan = running.sbom.clone();
        let finding = |state| DependencyFinding {
            reachability: None,
            analysis_detail: None,
            analysis_scan: None,
            id: format!("CVE-{state}"),
            state,
            status: FindingStatus::Open,
            severity: Severity::High,
            ratings: Vec::new(),
            affected: AffectedComponent {
                bom_ref: "pkg:cargo/serde@1.0.0".into(),
                name: "serde".into(),
                version: Some("1.0.0".into()),
                path: Vec::new(),
            },
            scan: scan.clone(),
            fixed_version: None,
            vex_state: None,
            vex_justification: None,
            vex_response: Vec::new(),
            kev: false,
            euvd: false,
            epss: None,
        };
        let mut dependencies = report(vec![running]);
        dependencies.findings = vec![
            finding(LifecycleState::Declared),
            finding(LifecycleState::Running),
        ];
        let answer = project(dependencies, now());
        assert_eq!(answer.findings.len(), 1);
        assert_eq!(answer.findings[0].state, LifecycleState::Running);
    }
}
