//! ADR 0005's filesystem-backed dependency inventory.
//!
//! Attachments are append-only daemon state. VEX files are the opposite:
//! authored input, re-read on every request and never changed here.

use chrono::{DateTime, Utc};
use factory_core::dependencies::{
    validate_document, AffectedComponent, Attachment, AttachmentKind, DependenciesFact,
    DependenciesReport, DependencyFinding, DependencyServiceView, DocumentSummary, FindingStatus,
    LifecycleDocuments, LifecycleState, Rating, Severity,
};
use factory_core::error::{FactoryError, Result};
use factory_core::protocol::CredentialRow;
use factory_core::task::TaskEntry;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::engine::Engine;

#[derive(Debug, Clone)]
struct StoredDocument {
    attachment: Attachment,
    json: Value,
}

#[derive(Debug, Clone)]
struct Scan {
    state: LifecycleState,
    sbom: StoredDocument,
    vulnerabilities: Option<StoredDocument>,
}

fn safe_scope_dir(base: &Path, scope: &str) -> Result<PathBuf> {
    let path = Path::new(scope);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(FactoryError::BadRequest(format!(
            "scope name {scope:?} cannot be used as a dependency inventory path"
        )));
    }
    Ok(base.join(path))
}

fn write_attachment(
    root: &Path,
    attachment: &Attachment,
    bytes: &[u8],
) -> std::result::Result<(), String> {
    let run_dir = safe_scope_dir(&root.join(".factory/dependencies"), &attachment.scope)
        .map_err(|e| e.to_string())?
        .join(&attachment.run_id);
    std::fs::create_dir_all(&run_dir)
        .map_err(|e| format!("creating {}: {e}", run_dir.display()))?;
    let document_path = run_dir.join(&attachment.filename);
    let metadata_path = run_dir.join(format!("{}.meta.json", attachment.id));

    let mut document = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&document_path)
        .map_err(|e| {
            format!(
                "creating immutable attachment {}: {e}",
                document_path.display()
            )
        })?;
    if let Err(error) = document.write_all(bytes).and_then(|_| document.sync_all()) {
        let _ = std::fs::remove_file(&document_path);
        return Err(format!("writing {}: {error}", document_path.display()));
    }
    let metadata = serde_json::to_vec_pretty(attachment)
        .map_err(|e| format!("serializing attachment metadata: {e}"))?;
    let mut sidecar = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&metadata_path)
    {
        Ok(file) => file,
        Err(error) => {
            let _ = std::fs::remove_file(&document_path);
            return Err(format!("creating {}: {error}", metadata_path.display()));
        }
    };
    if let Err(error) = sidecar
        .write_all(&metadata)
        .and_then(|_| sidecar.sync_all())
    {
        let _ = std::fs::remove_file(&metadata_path);
        let _ = std::fs::remove_file(&document_path);
        return Err(format!("writing {}: {error}", metadata_path.display()));
    }
    Ok(())
}

fn load_documents(root: &Path, scope: &str) -> Result<Vec<StoredDocument>> {
    let scope_dir = safe_scope_dir(&root.join(".factory/dependencies"), scope)?;
    let Ok(runs) = std::fs::read_dir(&scope_dir) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for run in runs.flatten().filter(|e| e.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(run.path()) else {
            continue;
        };
        for entry in files.flatten() {
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".meta.json"))
            {
                continue;
            }
            let bytes = std::fs::read(&path).map_err(|e| {
                FactoryError::Other(anyhow::anyhow!("reading {}: {e}", path.display()))
            })?;
            let attachment: Attachment = serde_json::from_slice(&bytes).map_err(|e| {
                FactoryError::Other(anyhow::anyhow!("parsing {}: {e}", path.display()))
            })?;
            if attachment.scope != scope {
                continue;
            }
            let document_path = run.path().join(&attachment.filename);
            let document = std::fs::read(&document_path).map_err(|e| {
                FactoryError::Other(anyhow::anyhow!("reading {}: {e}", document_path.display()))
            })?;
            let json = serde_json::from_slice(&document).map_err(|e| {
                FactoryError::Other(anyhow::anyhow!("parsing {}: {e}", document_path.display()))
            })?;
            out.push(StoredDocument { attachment, json });
        }
    }
    out.sort_by_key(|d| d.attachment.attached_at);
    Ok(out)
}

fn tool_and_time(json: &Value) -> (Option<String>, Option<String>, Option<DateTime<Utc>>) {
    let metadata = json.get("metadata").and_then(Value::as_object);
    let tool = metadata.and_then(|m| m.get("tools")).and_then(|tools| {
        tools
            .get("components")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .or_else(|| tools.as_array().and_then(|a| a.first()))
    });
    let name = tool
        .and_then(|t| t.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let version = tool
        .and_then(|t| t.get("version"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let time = metadata
        .and_then(|m| m.get("timestamp"))
        .and_then(Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc));
    (name, version, time)
}

fn summary(document: &StoredDocument) -> DocumentSummary {
    let (tool, tool_version, scan_time) = tool_and_time(&document.json);
    DocumentSummary {
        attachment: document.attachment.clone(),
        tool,
        tool_version,
        scan_time,
    }
}

fn scans(documents: &[StoredDocument]) -> Vec<Scan> {
    let mut by_run: BTreeMap<&str, Vec<&StoredDocument>> = BTreeMap::new();
    for document in documents {
        by_run
            .entry(&document.attachment.run_id)
            .or_default()
            .push(document);
    }
    let mut out = Vec::new();
    for documents in by_run.values() {
        let vulnerability = documents
            .iter()
            .filter(|d| d.attachment.kind == AttachmentKind::Vulnerabilities)
            .max_by_key(|d| d.attachment.attached_at)
            .map(|d| (*d).clone());
        for sbom in documents
            .iter()
            .filter(|d| d.attachment.kind == AttachmentKind::Sbom)
        {
            for state in &sbom.attachment.states {
                out.push(Scan {
                    state: *state,
                    sbom: (*sbom).clone(),
                    vulnerabilities: vulnerability.clone(),
                });
            }
        }
    }
    out.sort_by_key(|scan| scan.sbom.attachment.attached_at);
    out
}

fn component_label(component: &Map<String, Value>, fallback: &str) -> String {
    let name = component
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(fallback);
    match component.get("version").and_then(Value::as_str) {
        Some(version) => format!("{name}@{version}"),
        None => name.to_string(),
    }
}

fn components_and_paths(
    sbom: &Value,
) -> (
    BTreeMap<String, AffectedComponent>,
    BTreeMap<String, Vec<String>>,
) {
    let mut components = BTreeMap::new();
    let mut labels = BTreeMap::new();
    let mut roots = Vec::new();
    if let Some(component) = sbom
        .pointer("/metadata/component")
        .and_then(Value::as_object)
    {
        if let Some(reference) = component.get("bom-ref").and_then(Value::as_str) {
            roots.push(reference.to_string());
            labels.insert(reference.to_string(), component_label(component, reference));
            components.insert(
                reference.to_string(),
                AffectedComponent {
                    bom_ref: reference.to_string(),
                    name: component
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(reference)
                        .to_string(),
                    version: component
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    path: Vec::new(),
                },
            );
        }
    }
    for component in sbom
        .get("components")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(object) = component.as_object() else {
            continue;
        };
        let Some(reference) = object.get("bom-ref").and_then(Value::as_str) else {
            continue;
        };
        labels.insert(reference.to_string(), component_label(object, reference));
        components.insert(
            reference.to_string(),
            AffectedComponent {
                bom_ref: reference.to_string(),
                name: object
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(reference)
                    .to_string(),
                version: object
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                path: Vec::new(),
            },
        );
    }
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut depended_on = BTreeSet::new();
    for dependency in sbom
        .get("dependencies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(reference) = dependency.get("ref").and_then(Value::as_str) else {
            continue;
        };
        let children = dependency
            .get("dependsOn")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        depended_on.extend(children.iter().cloned());
        graph.insert(reference.to_string(), children);
    }
    if roots.is_empty() {
        roots.extend(graph.keys().filter(|r| !depended_on.contains(*r)).cloned());
    }
    let mut paths = BTreeMap::new();
    let mut queue: VecDeque<(String, Vec<String>)> = roots
        .into_iter()
        .map(|root| {
            let label = labels.get(&root).cloned().unwrap_or_else(|| root.clone());
            (root, vec![label])
        })
        .collect();
    while let Some((reference, path)) = queue.pop_front() {
        if paths.contains_key(&reference) {
            continue;
        }
        paths.insert(reference.clone(), path.clone());
        for child in graph.get(&reference).into_iter().flatten() {
            let mut child_path = path.clone();
            child_path.push(labels.get(child).cloned().unwrap_or_else(|| child.clone()));
            queue.push_back((child.clone(), child_path));
        }
    }
    (components, paths)
}

fn property<'a>(vulnerability: &'a Value, name: &str) -> Option<&'a str> {
    vulnerability
        .get("properties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|p| p.get("value"))
        .and_then(Value::as_str)
}

fn property_bool(vulnerability: &Value, name: &str) -> bool {
    property(vulnerability, name)
        .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "1"))
}

fn ratings(vulnerability: &Value) -> Vec<Rating> {
    vulnerability
        .get("ratings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|rating| Rating {
            source: rating
                .pointer("/source/name")
                .and_then(Value::as_str)
                .map(str::to_string),
            severity: Severity::from_cyclonedx(rating.get("severity").and_then(Value::as_str)),
            score: rating.get("score").and_then(Value::as_f64),
            method: rating
                .get("method")
                .and_then(Value::as_str)
                .map(str::to_string),
            vector: rating
                .get("vector")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
        .collect()
}

fn severity_of(ratings: &[Rating]) -> Severity {
    fn rank(severity: Severity) -> u8 {
        match severity {
            Severity::Critical => 4,
            Severity::High => 3,
            Severity::Medium => 2,
            Severity::Low => 1,
            Severity::Unknown => 0,
        }
    }
    ratings
        .iter()
        .map(|r| r.severity)
        .max_by_key(|s| rank(*s))
        .unwrap_or(Severity::Unknown)
}

fn scan_findings(scan: &Scan, status_override: Option<FindingStatus>) -> Vec<DependencyFinding> {
    let Some(document) = &scan.vulnerabilities else {
        return Vec::new();
    };
    let (components, paths) = components_and_paths(&scan.sbom.json);
    let mut out = Vec::new();
    for vulnerability in document
        .json
        .get("vulnerabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(id) = vulnerability.get("id").and_then(Value::as_str) else {
            continue;
        };
        let analysis = vulnerability.get("analysis");
        let vex_state = analysis
            .and_then(|a| a.get("state"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let rating_rows = ratings(vulnerability);
        let severity = severity_of(&rating_rows);
        for affect in vulnerability
            .get("affects")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(reference) = affect.get("ref").and_then(Value::as_str) else {
                continue;
            };
            let mut affected =
                components
                    .get(reference)
                    .cloned()
                    .unwrap_or_else(|| AffectedComponent {
                        bom_ref: reference.to_string(),
                        name: reference.to_string(),
                        version: None,
                        path: Vec::new(),
                    });
            affected.path = paths
                .get(reference)
                .cloned()
                .unwrap_or_else(|| vec![component_label_from_affected(&affected)]);
            let fixed_version = affect
                .get("versions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|version| version.get("status").and_then(Value::as_str) == Some("unaffected"))
                .and_then(|version| version.get("version").and_then(Value::as_str))
                .map(str::to_string);
            out.push(DependencyFinding {
                id: id.to_string(),
                state: scan.state,
                status: status_override.unwrap_or_else(|| {
                    if vex_state.is_some() {
                        FindingStatus::Assessed
                    } else {
                        FindingStatus::Open
                    }
                }),
                severity,
                ratings: rating_rows.clone(),
                affected,
                scan: summary(document),
                fixed_version,
                vex_state: vex_state.clone(),
                vex_justification: analysis
                    .and_then(|a| a.get("justification"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                vex_response: analysis
                    .and_then(|a| a.get("response"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                kev: property_bool(vulnerability, "factory:kev"),
                euvd: property_bool(vulnerability, "factory:euvd"),
                epss: property(vulnerability, "factory:epss").and_then(|v| v.parse().ok()),
            });
        }
    }
    out
}

fn component_label_from_affected(component: &AffectedComponent) -> String {
    match &component.version {
        Some(version) => format!("{}@{version}", component.name),
        None => component.name.clone(),
    }
}

fn finding_key(finding: &DependencyFinding) -> (String, String) {
    (finding.id.clone(), finding.affected.bom_ref.clone())
}

fn build_report(
    scope: &factory_core::config::Scope,
    stored: &[StoredDocument],
    credentials: &[CredentialRow],
    now: DateTime<Utc>,
) -> DependenciesReport {
    let scans = scans(stored);
    let mut newest: BTreeMap<LifecycleState, &Scan> = BTreeMap::new();
    for scan in &scans {
        newest.insert(scan.state, scan);
    }
    let documents = newest
        .iter()
        .map(|(state, scan)| LifecycleDocuments {
            state: *state,
            sbom: summary(&scan.sbom),
            vulnerabilities: scan.vulnerabilities.as_ref().map(summary),
        })
        .collect::<Vec<_>>();

    let mut findings = Vec::new();
    for (state, latest) in newest {
        let stale = scope
            .dependencies
            .max_age
            .is_some_and(|age| now - latest.sbom.attachment.attached_at > age.as_time_delta());
        let latest_rows = scan_findings(latest, stale.then_some(FindingStatus::Stale));
        let latest_keys: BTreeSet<_> = latest_rows.iter().map(finding_key).collect();
        findings.extend(latest_rows);

        let mut historical = BTreeMap::new();
        for scan in scans.iter().filter(|scan| {
            scan.state == state && scan.sbom.attachment.id != latest.sbom.attachment.id
        }) {
            for finding in scan_findings(scan, None) {
                historical.insert(finding_key(&finding), finding);
            }
        }
        // A newer SBOM without a vulnerability attachment proves only that
        // inventory ran, not that an older finding disappeared. Keep the
        // newest assessment for each finding until another vulnerability
        // document actually reports its absence.
        if latest.vulnerabilities.is_none() {
            for mut finding in historical.into_values() {
                if scope.dependencies.max_age.is_some_and(|age| {
                    now - finding.scan.attachment.attached_at > age.as_time_delta()
                }) {
                    finding.status = FindingStatus::Stale;
                }
                findings.push(finding);
            }
            continue;
        }
        for (key, mut finding) in historical {
            if latest_keys.contains(&key) {
                continue;
            }
            finding.status = if stale {
                FindingStatus::Stale
            } else {
                FindingStatus::Resolved
            };
            finding.scan = latest
                .vulnerabilities
                .as_ref()
                .map(summary)
                .unwrap_or_else(|| summary(&latest.sbom));
            findings.push(finding);
        }
    }
    findings.sort_by(|a, b| {
        a.state
            .cmp(&b.state)
            .then(a.status_as_rank().cmp(&b.status_as_rank()))
            .then(a.severity.cmp(&b.severity))
            .then(a.id.cmp(&b.id))
            .then(a.affected.name.cmp(&b.affected.name))
    });

    let services = scope
        .dependencies
        .services
        .iter()
        .cloned()
        .map(|service| {
            let credential_present = service.credential.as_ref().and_then(|location| {
                credentials
                    .iter()
                    .find(|row| row.path == *location || row.integration == *location)
                    .map(|row| row.present)
            });
            DependencyServiceView {
                service,
                credential_present,
            }
        })
        .collect();

    DependenciesReport {
        scope: scope.name.clone(),
        scan_workflow: scope.dependencies.scan_workflow.clone(),
        max_age: scope.dependencies.max_age,
        documents,
        findings,
        services,
    }
}

trait FindingRank {
    fn status_as_rank(&self) -> u8;
}

impl FindingRank for DependencyFinding {
    fn status_as_rank(&self) -> u8 {
        match self.status {
            FindingStatus::Open => 0,
            FindingStatus::Assessed => 1,
            FindingStatus::Stale => 2,
            FindingStatus::Resolved => 3,
        }
    }
}

pub(crate) fn fact(report: &DependenciesReport) -> DependenciesFact {
    let declared_sbom_at = report
        .documents
        .iter()
        .find(|d| d.state == LifecycleState::Declared)
        .map(|d| d.sbom.attachment.attached_at);
    let mut fact = DependenciesFact {
        declared_sbom_at,
        ..Default::default()
    };
    for finding in report
        .findings
        .iter()
        .filter(|f| f.status == FindingStatus::Open)
    {
        *fact.open.entry(finding.severity).or_default() += 1;
        if finding.kev || finding.euvd {
            fact.exploited_open += 1;
        }
    }
    fact
}

impl Engine {
    pub(crate) async fn attach_dependency(
        &self,
        task_id: &str,
        kind: AttachmentKind,
        bytes: Vec<u8>,
        token: Option<&str>,
    ) -> Result<Attachment> {
        let run = self.store.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; attachments are no longer accepted"
            ))
        })?;
        self.check_run_token(&run, token, task_id)?;
        let task = self
            .store
            .get(task_id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such task: {task_id}")))?;
        let validated = validate_document(&bytes, kind).map_err(FactoryError::BadRequest)?;
        let id = uuid::Uuid::new_v4().to_string();
        let attached_at = Utc::now();
        let filename = format!(
            "{}-{}-{id}.cdx.json",
            attached_at.format("%Y%m%dT%H%M%S%.6fZ"),
            kind.as_str()
        );
        let attachment = Attachment {
            id,
            kind,
            scope: task.scope.clone(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            attempt: run.attempt,
            attached_at,
            filename,
            spec_version: validated.spec_version,
            states: validated.states,
        };
        let root = self.factory_snapshot().root;
        let to_write = attachment.clone();
        tokio::task::spawn_blocking(move || write_attachment(&root, &to_write, &bytes))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("attachment writer: {e}")))?
            .map_err(FactoryError::BadRequest)?;
        self.entry(
            task_id,
            TaskEntry::new(
                "agent",
                "attachment",
                format!("attached {} as {}", attachment.filename, attachment.kind),
            )
            .in_run(&run.id),
        )
        .await;
        Ok(attachment)
    }

    pub(crate) async fn dependencies_report(&self, scope: &str) -> Result<DependenciesReport> {
        let snapshot = self.factory_snapshot();
        let scope = snapshot.scope(scope)?.clone();
        let root = snapshot.root;
        let name = scope.name.clone();
        let documents = tokio::task::spawn_blocking(move || load_documents(&root, &name))
            .await
            .map_err(|e| {
                FactoryError::Other(anyhow::anyhow!("dependency inventory walk: {e}"))
            })??;
        let credentials = self.credential_inventory().await;
        Ok(build_report(&scope, &documents, &credentials, Utc::now()))
    }

    pub(crate) async fn dependencies_vex(&self, scope: &str) -> Result<String> {
        let snapshot = self.factory_snapshot();
        let canonical = snapshot.scope(scope)?.name.clone();
        let root = snapshot.root;
        tokio::task::spawn_blocking(move || merge_vex(&root, &canonical))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("VEX walk: {e}")))?
    }
}

fn merge_vex(root: &Path, scope: &str) -> Result<String> {
    let dir = safe_scope_dir(&root.join(".factory/vex"), scope)?;
    let mut paths = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".cdx.json"))
        })
        .collect::<Vec<_>>();
    paths.sort();
    let mut vulnerabilities = Vec::new();
    for path in paths {
        let bytes = std::fs::read(&path)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("reading {}: {e}", path.display())))?;
        let validated = validate_document(&bytes, AttachmentKind::Vulnerabilities)
            .map_err(|e| FactoryError::BadRequest(format!("{}: {e}", path.display())))?;
        vulnerabilities.extend(
            validated
                .json
                .get("vulnerabilities")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .cloned(),
        );
    }
    serde_json::to_string_pretty(&serde_json::json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "serialNumber": format!("urn:uuid:{}", uuid::Uuid::new_v4()),
        "version": 1,
        "metadata": { "timestamp": Utc::now() },
        "vulnerabilities": vulnerabilities,
    }))
    .map_err(|e| FactoryError::Other(e.into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::config::{DependenciesConfig, Scope};

    fn stored(kind: AttachmentKind, json: &[u8], at: &str, id: &str, run: &str) -> StoredDocument {
        let validated = validate_document(json, kind).unwrap();
        StoredDocument {
            attachment: Attachment {
                id: id.into(),
                kind,
                scope: "demo".into(),
                run_id: run.into(),
                task_id: "task".into(),
                attempt: 1,
                attached_at: DateTime::parse_from_rfc3339(at)
                    .unwrap()
                    .with_timezone(&Utc),
                filename: format!("{id}.cdx.json"),
                spec_version: validated.spec_version,
                states: validated.states,
            },
            json: validated.json,
        }
    }

    fn scope(max_age: Option<factory_core::policy::Duration>) -> Scope {
        serde_yaml_ng::from_str::<Scope>("name: demo")
            .map(|mut scope| {
                scope.dependencies = DependenciesConfig {
                    max_age,
                    ..Default::default()
                };
                scope
            })
            .unwrap()
    }

    #[test]
    fn newest_scan_is_open_and_an_absent_older_finding_is_resolved() {
        let sbom =
            include_bytes!("../../factory-core/tests/fixtures/dependencies/declared-sbom.cdx.json");
        let vulns = include_bytes!(
            "../../factory-core/tests/fixtures/dependencies/vulnerabilities.cdx.json"
        );
        let empty = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[]}"#;
        let docs = vec![
            stored(
                AttachmentKind::Sbom,
                sbom,
                "2026-09-24T10:00:00Z",
                "s1",
                "r1",
            ),
            stored(
                AttachmentKind::Vulnerabilities,
                vulns,
                "2026-09-24T10:01:00Z",
                "v1",
                "r1",
            ),
            stored(
                AttachmentKind::Sbom,
                sbom,
                "2026-09-25T10:00:00Z",
                "s2",
                "r2",
            ),
            stored(
                AttachmentKind::Vulnerabilities,
                empty,
                "2026-09-25T10:01:00Z",
                "v2",
                "r2",
            ),
        ];
        let report = build_report(
            &scope(None),
            &docs,
            &[],
            DateTime::parse_from_rfc3339("2026-09-25T11:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].status, FindingStatus::Resolved);
    }

    #[test]
    fn stale_scan_never_claims_a_finding_was_resolved() {
        let age = "1h".parse().unwrap();
        let sbom =
            include_bytes!("../../factory-core/tests/fixtures/dependencies/declared-sbom.cdx.json");
        let vulns = include_bytes!(
            "../../factory-core/tests/fixtures/dependencies/vulnerabilities.cdx.json"
        );
        let docs = vec![
            stored(
                AttachmentKind::Sbom,
                sbom,
                "2026-09-25T10:00:00Z",
                "s1",
                "r1",
            ),
            stored(
                AttachmentKind::Vulnerabilities,
                vulns,
                "2026-09-25T10:01:00Z",
                "v1",
                "r1",
            ),
        ];
        let report = build_report(
            &scope(Some(age)),
            &docs,
            &[],
            DateTime::parse_from_rfc3339("2026-09-25T12:00:01Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(report.findings[0].status, FindingStatus::Stale);
    }

    #[test]
    fn a_new_sbom_without_an_assessment_never_proves_resolution() {
        let sbom = include_bytes!(
            "../../factory-core/tests/fixtures/dependencies/declared-sbom.cdx.json"
        );
        let vulns = include_bytes!(
            "../../factory-core/tests/fixtures/dependencies/vulnerabilities.cdx.json"
        );
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-09-24T10:00:00Z", "s1", "r1"),
            stored(AttachmentKind::Vulnerabilities, vulns, "2026-09-24T10:01:00Z", "v1", "r1"),
            stored(AttachmentKind::Sbom, sbom, "2026-09-25T10:00:00Z", "s2", "r2"),
        ];
        let report = build_report(
            &scope(None),
            &docs,
            &[],
            DateTime::parse_from_rfc3339("2026-09-25T11:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(report.findings[0].status, FindingStatus::Open);
        assert_eq!(report.findings[0].scan.attachment.run_id, "r1");
    }

    #[test]
    fn stored_documents_are_create_only() {
        let root = std::env::temp_dir().join(format!("factory-dependencies-{}", uuid::Uuid::new_v4()));
        let document = stored(
            AttachmentKind::Sbom,
            include_bytes!("../../factory-core/tests/fixtures/dependencies/declared-sbom.cdx.json"),
            "2026-09-25T10:00:00Z", "same", "run",
        );
        let first = b"original";
        write_attachment(&root, &document.attachment, first).unwrap();
        assert!(write_attachment(&root, &document.attachment, b"replacement").is_err());
        let path = root.join(".factory/dependencies/demo/run").join(&document.attachment.filename);
        assert_eq!(std::fs::read(path).unwrap(), first);
        std::fs::remove_dir_all(root).unwrap();
    }
}
