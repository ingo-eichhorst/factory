//! ADR 0005's filesystem-backed dependency inventory.
//!
//! Attachments are append-only daemon state. VEX files are the opposite:
//! authored input, re-read on every request and never changed here.

use chrono::{DateTime, Utc};
use factory_core::dependencies::{
    product_identity, validate_document, AffectedComponent, Attachment, AttachmentKind,
    DependenciesFact, DependenciesReport, DependencyFinding, DependencyServiceView,
    DocumentSummary, ExploitedFinding, FindingStatus, LifecycleDocuments, LifecycleState, Rating,
    Severity,
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
        identity: product_identity(&document.json),
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
            let reachability = component_property(vulnerability, "factory:reachability", reference);
            let analysis_detail = [
                analysis
                    .and_then(|a| a.get("detail"))
                    .and_then(Value::as_str)
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string),
                component_property(vulnerability, "factory:reachability-detail", reference),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("\n");
            let analysis_scan = (reachability.is_some() || !analysis_detail.is_empty())
                .then(|| summary(document));
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
                reachability,
                analysis_detail: (!analysis_detail.is_empty()).then_some(analysis_detail),
                analysis_scan,
                euvd: property_bool(vulnerability, "factory:euvd"),
                epss: property(vulnerability, "factory:epss").and_then(|v| v.parse().ok()),
            });
        }
    }
    out
}

/// A scalar is a vulnerability-wide claim. A JSON object binds claims to
/// exact affects refs; missing/malformed entries are unknown, not inherited.
fn component_property(vulnerability: &Value, name: &str, reference: &str) -> Option<String> {
    let claims: BTreeSet<String> = vulnerability
        .get("properties")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| p.get("name").and_then(Value::as_str) == Some(name))
        .filter_map(|p| p.get("value").and_then(Value::as_str))
        .filter_map(|raw| {
            let raw = raw.trim();
            if raw.is_empty() {
                return None;
            }
            if raw.starts_with('{') {
                let claims: BTreeMap<String, String> = serde_json::from_str(raw).ok()?;
                claims
                    .get(reference)
                    .filter(|value| !value.trim().is_empty())
                    .cloned()
            } else {
                Some(raw.to_string())
            }
        })
        .collect();
    match claims.len() {
        0 => None,
        1 => claims.into_iter().next(),
        _ => Some(format!(
            "conflicting claims: {}",
            claims.into_iter().collect::<Vec<_>>().join(" | ")
        )),
    }
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
        service_evidence: None,
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
    let built_sbom_at = report
        .documents
        .iter()
        .find(|d| d.state == LifecycleState::Built)
        .map(|d| d.sbom.attachment.attached_at);
    let mut fact = DependenciesFact {
        declared_sbom_at,
        built_sbom_at,
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

/// One vulnerability's per-mention bookkeeping while `exploited` walks the
/// scope's scans oldest first -- folded into an [`ExploitedFinding`] once
/// every relevant scan has been seen.
struct ExploitedTrack {
    components: BTreeMap<String, AffectedComponent>,
    states: BTreeSet<LifecycleState>,
    first_seen_at: DateTime<Utc>,
    first_document: Attachment,
    latest_vex: Option<String>,
    last_mentioned_document_id: String,
}

/// `analysis.state` values that never establish -- or continue to count as
/// -- a sighting: CRA Art. 14's reporting clock (`#157`, phase 1) is built
/// on KEV/EUVD-listed vulnerabilities a scan still treats as live.
fn never_a_sighting(state: Option<&str>) -> bool {
    matches!(state, Some("not_affected") | Some("false_positive") | Some("resolved") | Some("resolved_with_pedigree"))
}

/// The two `analysis.state` values that exclude an already-sighted item --
/// a strict subset of [`never_a_sighting`]: `resolved`/`resolved_with_pedigree`
/// stop a *new* sighting but never retroactively excuse one already made
/// (CRA counts from awareness).
fn excludes(state: Option<&str>) -> bool {
    matches!(state, Some("not_affected") | Some("false_positive"))
}

fn affected_component(
    reference: &str,
    components: &BTreeMap<String, AffectedComponent>,
    paths: &BTreeMap<String, Vec<String>>,
) -> AffectedComponent {
    let mut affected = components.get(reference).cloned().unwrap_or_else(|| AffectedComponent {
        bom_ref: reference.to_string(),
        name: reference.to_string(),
        version: None,
        path: Vec::new(),
    });
    affected.path = paths
        .get(reference)
        .cloned()
        .unwrap_or_else(|| vec![component_label_from_affected(&affected)]);
    affected
}

/// `Engine::exploited_findings`'s pure core, kept free of `Engine` so the
/// unit tests below can build `StoredDocument`s directly with `stored()`,
/// exactly as `build_report`'s own tests do.
///
/// One item per (scope, vulnerability id) sighted against a `built` or
/// `running` SBOM -- see `ExploitedFinding`'s own doc comment for exactly
/// what counts as a sighting, an exclusion, and `reported_now`. Scans are
/// walked in the *vulnerability document's* own `attached_at` order (never
/// the SBOM's): that is the CRA awareness time, and it is also what decides
/// which built/running document is "newest" for exclusion and
/// `reported_now`.
fn exploited(scope: &str, stored: &[StoredDocument]) -> Vec<ExploitedFinding> {
    let all_scans = scans(stored);
    let mut relevant: Vec<&Scan> = all_scans
        .iter()
        .filter(|scan| {
            matches!(scan.state, LifecycleState::Built | LifecycleState::Running) && scan.vulnerabilities.is_some()
        })
        .collect();
    relevant.sort_by(|a, b| {
        let a = &a.vulnerabilities.as_ref().expect("filtered above").attachment;
        let b = &b.vulnerabilities.as_ref().expect("filtered above").attachment;
        a.attached_at.cmp(&b.attached_at).then_with(|| a.id.cmp(&b.id))
    });
    let Some(newest) = relevant.last() else {
        return Vec::new();
    };
    let newest_document_id = newest.vulnerabilities.as_ref().expect("filtered above").attachment.id.clone();

    let mut tracks: BTreeMap<String, ExploitedTrack> = BTreeMap::new();
    for scan in relevant {
        let document = scan.vulnerabilities.as_ref().expect("filtered above");
        let (components, paths) = components_and_paths(&scan.sbom.json);
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
            let state = vulnerability.get("analysis").and_then(|a| a.get("state")).and_then(Value::as_str);
            let refs: Vec<String> = vulnerability
                .get("affects")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|a| a.get("ref").and_then(Value::as_str))
                .map(str::to_string)
                .collect();

            if !tracks.contains_key(id) {
                let kev = property_bool(vulnerability, "factory:kev");
                let euvd = property_bool(vulnerability, "factory:euvd");
                if !(kev || euvd) || never_a_sighting(state) {
                    continue;
                }
                tracks.insert(
                    id.to_string(),
                    ExploitedTrack {
                        components: BTreeMap::new(),
                        states: BTreeSet::new(),
                        first_seen_at: document.attachment.attached_at,
                        first_document: document.attachment.clone(),
                        latest_vex: None,
                        last_mentioned_document_id: String::new(),
                    },
                );
            }
            let track = tracks.get_mut(id).expect("just inserted or already present");
            track.states.insert(scan.state);
            track.latest_vex = excludes(state).then(|| state.expect("excludes() only true for Some").to_string());
            track.last_mentioned_document_id = document.attachment.id.clone();
            for reference in &refs {
                track
                    .components
                    .insert(reference.clone(), affected_component(reference, &components, &paths));
            }
        }
    }

    let mut out: Vec<ExploitedFinding> = tracks
        .into_iter()
        .map(|(vulnerability, track)| ExploitedFinding {
            scope: scope.to_string(),
            vulnerability,
            components: track.components.into_values().collect(),
            states: track.states.into_iter().collect(),
            first_seen_at: track.first_seen_at,
            first_document: track.first_document,
            latest_vex: track.latest_vex,
            reported_now: track.last_mentioned_document_id == newest_document_id,
        })
        .collect();
    out.sort_by(|a, b| a.vulnerability.cmp(&b.vulnerability));
    out
}

impl Engine {
    pub(crate) async fn release_sboms(&self, scope: &str, commit: &str, version: Option<&str>) -> Result<Vec<factory_kernel::ReleaseSbomFact>> {
        let snapshot = self.factory_snapshot();
        let canonical = snapshot.scope(scope)?.name.clone();
        let root = snapshot.root;
        let name = canonical.clone();
        let documents = tokio::task::spawn_blocking(move || load_documents(&root, &name)).await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("release SBOM read: {e}")))??;
        let mut facts = Vec::new();
        for document in documents {
            if document.attachment.kind != AttachmentKind::Sbom || !document.attachment.states.contains(&LifecycleState::Built) { continue; }
            let Some(identity) = product_identity(&document.json) else { continue; };
            if identity.git_sha != commit || version.is_some_and(|version| version != identity.version) { continue; }
            facts.push(factory_kernel::ReleaseSbomFact { scope: canonical.clone(), commit: identity.git_sha,
                version: identity.version, attachment: document.attachment });
        }
        Ok(facts)
    }

    pub(crate) async fn dependency_document(&self, scope: &str, id: &str) -> Result<(Attachment, Value)> {
        let snapshot = self.factory_snapshot();
        let canonical = snapshot.scope(scope)?.name.clone();
        let root = snapshot.root;
        let documents = tokio::task::spawn_blocking(move || load_documents(&root, &canonical)).await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("dependency document read: {e}")))??;
        let document = documents.into_iter().find(|document| document.attachment.id == id)
            .ok_or_else(|| FactoryError::BadRequest("no such attachment in the selected scope".into()))?;
        Ok((document.attachment, document.json))
    }

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

    /// The CRA Art. 14 reporting clock's L2 read (`#157`, phase 1): every
    /// exploited finding in `scope` alone -- never its subtree, unlike
    /// `Request::Policy`'s own rollup -- so a caller that needs a subtree's
    /// worth (`policies::clock`) asks once per scope and so that
    /// `policy_attest`'s own equality check (an item's own scope must equal
    /// the canonical `--scope`) has something exact to check against.
    pub(crate) async fn exploited_findings(&self, scope: &str) -> Result<Vec<ExploitedFinding>> {
        let snapshot = self.factory_snapshot();
        let canonical = snapshot.scope(scope)?.name.clone();
        let root = snapshot.root;
        let name = canonical.clone();
        let documents = tokio::task::spawn_blocking(move || load_documents(&root, &name))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("dependency inventory walk: {e}")))??;
        Ok(exploited(&canonical, &documents))
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

    #[test]
    fn reachability_is_display_only_and_keeps_its_immutable_source_when_resolved() {
        let sbom =
            include_bytes!("../../factory-core/tests/fixtures/dependencies/build-sbom.cdx.json");
        let mut vulns: Value = serde_json::from_slice(include_bytes!(
            "../../factory-core/tests/fixtures/dependencies/vulnerabilities.cdx.json"
        ))
        .unwrap();
        vulns["vulnerabilities"][0]["analysis"] =
            serde_json::json!({"detail":"call path <entry> → library; not a VEX decision"});
        vulns["vulnerabilities"][0]["properties"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"name":"factory:reachability", "value":"unreachable"}));
        let bytes = serde_json::to_vec(&vulns).unwrap();
        let mut docs = vec![
            stored(
                AttachmentKind::Sbom,
                sbom,
                "2026-10-01T10:00:00Z",
                "s1",
                "r1",
            ),
            stored(
                AttachmentKind::Vulnerabilities,
                &bytes,
                "2026-10-01T10:01:00Z",
                "v1",
                "r1",
            ),
        ];
        let now = "2026-10-01T12:00:00Z".parse().unwrap();
        let report = build_report(&scope(None), &docs, &[], now);
        let finding = &report.findings[0];
        assert_eq!(finding.status, FindingStatus::Open);
        assert_eq!(finding.reachability.as_deref(), Some("unreachable"));
        assert!(finding
            .analysis_detail
            .as_deref()
            .unwrap()
            .contains("call path"));
        assert_eq!(finding.vex_state, None);
        assert_eq!(finding.analysis_scan.as_ref().unwrap().attachment.id, "v1");
        assert_eq!(
            exploited("demo", &docs).len(),
            1,
            "reachability cannot exclude exploited findings from the clock"
        );
        assert_eq!(fact(&report).exploited_open, 1);
        let stale = build_report(&scope(Some("1h".parse().unwrap())), &docs, &[], now);
        assert_eq!(stale.findings[0].status, FindingStatus::Stale);
        assert_eq!(stale.findings[0].reachability, finding.reachability);
        assert_eq!(stale.findings[0].analysis_scan, finding.analysis_scan);
        docs.push(stored(
            AttachmentKind::Sbom,
            sbom,
            "2026-10-01T11:00:00Z",
            "s2",
            "r2",
        ));
        docs.push(stored(
            AttachmentKind::Vulnerabilities,
            br#"{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[]}"#,
            "2026-10-01T11:01:00Z",
            "v2",
            "r2",
        ));
        let resolved = build_report(&scope(None), &docs, &[], now);
        assert_eq!(resolved.findings[0].status, FindingStatus::Resolved);
        assert_eq!(resolved.findings[0].scan.attachment.id, "v2");
        assert_eq!(
            resolved.findings[0]
                .analysis_scan
                .as_ref()
                .unwrap()
                .attachment
                .id,
            "v1"
        );
        assert_eq!(resolved.findings[0].reachability, finding.reachability);
    }

    #[test]
    fn reachability_properties_bind_exact_components_and_prose_is_not_classified() {
        let sbom =
            include_bytes!("../../factory-core/tests/fixtures/dependencies/declared-sbom.cdx.json");
        let mut vuln: Value = serde_json::from_slice(include_bytes!(
            "../../factory-core/tests/fixtures/dependencies/vulnerabilities.cdx.json"
        ))
        .unwrap();
        vuln["vulnerabilities"][0]["affects"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"ref":"pkg:cargo/another-lib@1"}));
        vuln["vulnerabilities"][0]["analysis"] =
            serde_json::json!({"state":"exploitable", "detail":"unreachable prose is not a claim"});
        vuln["vulnerabilities"][0]["properties"].as_array_mut().unwrap().push(serde_json::json!({"name":"factory:reachability", "value":r#"{"pkg:cargo/demo-lib@1.2.3":"reachable"}"#}));
        let bytes = serde_json::to_vec(&vuln).unwrap();
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-10-01T10:00:00Z", "s", "r"),
            stored(
                AttachmentKind::Vulnerabilities,
                &bytes,
                "2026-10-01T10:01:00Z",
                "v",
                "r",
            ),
        ];
        let report = build_report(
            &scope(None),
            &docs,
            &[],
            "2026-10-01T12:00:00Z".parse().unwrap(),
        );
        let first = report
            .findings
            .iter()
            .find(|f| f.affected.name == "demo-lib")
            .unwrap();
        let second = report
            .findings
            .iter()
            .find(|f| f.affected.bom_ref == "pkg:cargo/another-lib@1")
            .unwrap();
        assert_eq!(first.reachability.as_deref(), Some("reachable"));
        assert_eq!(second.reachability, None);
        assert_eq!(
            second.analysis_detail.as_deref(),
            Some("unreachable prose is not a claim")
        );
        assert_eq!(first.vex_state.as_deref(), Some("exploitable"));
        assert_eq!(first.status, FindingStatus::Assessed);
        assert_eq!(
            component_property(
                &vuln["vulnerabilities"][0],
                "factory:reachability",
                "wrong-ref"
            ),
            None
        );
        let malformed =
            serde_json::json!({"properties":[{"name":"factory:reachability","value":"{broken"}]});
        assert_eq!(
            component_property(&malformed, "factory:reachability", "ref"),
            None
        );
        let conflicting = serde_json::json!({"properties":[{"name":"factory:reachability","value":"reachable"},{"name":"factory:reachability","value":"unreachable"}]});
        assert_eq!(
            component_property(&conflicting, "factory:reachability", "ref").as_deref(),
            Some("conflicting claims: reachable | unreachable")
        );
    }

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

    fn scope(max_age: Option<factory_kernel::Duration>) -> Scope {
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
    fn policy_fact_projects_the_newest_build_sbom() {
        let built = include_bytes!(
            "../../factory-core/tests/fixtures/dependencies/build-sbom.cdx.json"
        );
        let docs = vec![stored(
            AttachmentKind::Sbom,
            built,
            "2026-09-25T10:00:00Z",
            "built",
            "r1",
        )];
        let report = build_report(
            &scope(None),
            &docs,
            &[],
            DateTime::parse_from_rfc3339("2026-09-25T11:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );

        assert_eq!(
            fact(&report).built_sbom_at,
            Some(
                DateTime::parse_from_rfc3339("2026-09-25T10:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
        assert_eq!(fact(&build_report(&scope(None), &[], &[], Utc::now())).built_sbom_at, None);
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

    // ---------------------------------------------------- exploited findings

    /// A minimal vulnerabilities document naming one vulnerability, with
    /// `kev`/`euvd` and an optional `analysis.state` -- everything
    /// `exploited`'s tests below vary.
    fn vuln_doc(id_suffix: &str, kev: bool, euvd: bool, state: Option<&str>) -> String {
        let analysis = state.map(|s| format!(r#","analysis":{{"state":"{s}"}}"#)).unwrap_or_default();
        format!(
            r#"{{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[{{"id":"CVE-2026-{id_suffix}","affects":[{{"ref":"pkg:cargo/serde@1.0.0"}}],"properties":[{{"name":"factory:kev","value":"{kev}"}},{{"name":"factory:euvd","value":"{euvd}"}}]{analysis}}}]}}"#
        )
    }

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn one_qualifying_sighting_gives_one_item_at_the_vulnerability_documents_own_time() {
        let sbom = include_bytes!("../../factory-core/tests/fixtures/dependencies/build-sbom.cdx.json");
        let vulns = include_bytes!("../../factory-core/tests/fixtures/dependencies/vulnerabilities.cdx.json");
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-09-24T10:00:00Z", "s1", "r1"),
            stored(AttachmentKind::Vulnerabilities, vulns, "2026-09-24T10:01:00Z", "v1", "r1"),
        ];
        let items = exploited("demo", &docs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].scope, "demo");
        assert_eq!(items[0].vulnerability, "CVE-2026-1234");
        // The vulnerability document's own `attached_at` -- a minute after
        // the SBOM's, in this fixture pair -- never the SBOM's.
        assert_eq!(items[0].first_seen_at, t("2026-09-24T10:01:00Z"));
        assert_eq!(items[0].first_document.id, "v1");
        assert_eq!(items[0].states, vec![LifecycleState::Built]);
        assert!(items[0].reported_now);
        assert_eq!(items[0].latest_vex, None);
    }

    #[test]
    fn a_declared_only_sbom_gives_no_exploited_findings() {
        let sbom = include_bytes!("../../factory-core/tests/fixtures/dependencies/declared-sbom.cdx.json");
        let vulns = include_bytes!("../../factory-core/tests/fixtures/dependencies/vulnerabilities.cdx.json");
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-09-24T10:00:00Z", "s1", "r1"),
            stored(AttachmentKind::Vulnerabilities, vulns, "2026-09-24T10:01:00Z", "v1", "r1"),
        ];
        assert!(exploited("demo", &docs).is_empty(), "declared never counts");
    }

    #[test]
    fn analysis_state_exploitable_still_counts_as_a_sighting() {
        let sbom = include_bytes!("../../factory-core/tests/fixtures/dependencies/build-sbom.cdx.json");
        let vulns = vuln_doc("0001", true, false, Some("exploitable"));
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-09-24T10:00:00Z", "s1", "r1"),
            stored(AttachmentKind::Vulnerabilities, vulns.as_bytes(), "2026-09-24T10:01:00Z", "v1", "r1"),
        ];
        let items = exploited("demo", &docs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].latest_vex, None, "exploitable is not an exclusion");
        assert!(items[0].reported_now);
    }

    #[test]
    fn a_later_not_affected_mention_excludes_the_item_but_never_its_awareness() {
        let sbom = include_bytes!("../../factory-core/tests/fixtures/dependencies/build-sbom.cdx.json");
        let first = vuln_doc("0002", true, false, None);
        let second = vuln_doc("0002", true, false, Some("not_affected"));
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-09-24T10:00:00Z", "s1", "r1"),
            stored(AttachmentKind::Vulnerabilities, first.as_bytes(), "2026-09-24T10:01:00Z", "v1", "r1"),
            stored(AttachmentKind::Sbom, sbom, "2026-09-25T10:00:00Z", "s2", "r2"),
            stored(AttachmentKind::Vulnerabilities, second.as_bytes(), "2026-09-25T10:01:00Z", "v2", "r2"),
        ];
        let items = exploited("demo", &docs);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].latest_vex.as_deref(), Some("not_affected"));
        assert_eq!(items[0].first_seen_at, t("2026-09-24T10:01:00Z"), "awareness never moves");
    }

    #[test]
    fn a_later_scan_without_the_vulnerability_keeps_the_item_with_reported_now_false() {
        let sbom = include_bytes!("../../factory-core/tests/fixtures/dependencies/build-sbom.cdx.json");
        let first = vuln_doc("0003", true, false, None);
        let empty = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[]}"#;
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-09-24T10:00:00Z", "s1", "r1"),
            stored(AttachmentKind::Vulnerabilities, first.as_bytes(), "2026-09-24T10:01:00Z", "v1", "r1"),
            stored(AttachmentKind::Sbom, sbom, "2026-09-25T10:00:00Z", "s2", "r2"),
            stored(AttachmentKind::Vulnerabilities, empty, "2026-09-25T10:01:00Z", "v2", "r2"),
        ];
        let items = exploited("demo", &docs);
        assert_eq!(items.len(), 1, "CRA counts from awareness, not the newest scan");
        assert!(!items[0].reported_now);
        assert_eq!(items[0].latest_vex, None, "dropped, not excluded");
        assert_eq!(items[0].first_seen_at, t("2026-09-24T10:01:00Z"));
    }

    #[test]
    fn kev_and_euvd_both_false_gives_no_exploited_findings() {
        let sbom = include_bytes!("../../factory-core/tests/fixtures/dependencies/build-sbom.cdx.json");
        let vulns = vuln_doc("0004", false, false, None);
        let docs = vec![
            stored(AttachmentKind::Sbom, sbom, "2026-09-24T10:00:00Z", "s1", "r1"),
            stored(AttachmentKind::Vulnerabilities, vulns.as_bytes(), "2026-09-24T10:01:00Z", "v1", "r1"),
        ];
        assert!(exploited("demo", &docs).is_empty());
    }
}
