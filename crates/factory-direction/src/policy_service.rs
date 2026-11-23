//! Live L6 Policy reports/details. Own declarations and receipts stay here;
//! live check judgements and process inventory cross only typed fact ports.
use crate::{policy, policy_intent, policy_report::*, remediation::open_policy_label};
use factory_assurance::{check_evaluation, checks::EvaluationSubject, evidence::BudgetIntent};
use factory_kernel::{
    resolve_scope, scope_subtree, Attestation, CheckEvaluationFact, CheckObservation, ControlRef,
    FactoryError, Facts, KnowledgeTags, Provide, Result, ScopeNode, TaskInventoryFact,
    TaskInventoryQuery, L6,
};
use std::collections::{BTreeMap, BTreeSet};

mod receipts;

pub struct Service<'a> {
    intent: policy_intent::Service<'a>,
}
impl<'a> Service<'a> {
    pub fn new(intent: policy_intent::Service<'a>) -> Self {
        Self { intent }
    }

    /// L6 selects authored workflows and requirements; L5 owns every live
    /// preview judgement. Store-wide failure is fatal, per-subject refusal
    /// is a finding. Refresh each selected blueprint after the initial list,
    /// preserving the old list/lint read boundary and original row labels.
    pub async fn workflow_enforcement<B, P>(
        &self, scope: Option<&str>, blueprints: &B, preview: &P,
    ) -> Result<(Vec<WorkflowEnforcement>, Vec<WorkflowEnforcementFinding>)>
    where
        B: Provide<factory_kernel::WorkflowBlueprintFact,
            Query = factory_kernel::WorkflowBlueprintQuery,
            Value = Vec<factory_kernel::WorkflowBlueprintFact>, Error = FactoryError>,
        P: Provide<factory_kernel::WorkflowTargetsFact,
            Query = factory_kernel::WorkflowBlueprintFact,
            Value = factory_kernel::WorkflowTargetsFact, Error = FactoryError>
            + Provide<factory_kernel::WorkflowPreviewFact,
                Query = factory_assurance::workflow_preview::Read,
                Value = factory_kernel::WorkflowPreviewFact, Error = FactoryError>,
    {
        use factory_kernel::{WorkflowBlueprintFact, WorkflowBlueprintQuery, WorkflowTargetsFact, WorkflowPreviewFact};
        let (_, targets) = scope_subtree(&self.intent.config.scopes, scope)?;
        let names: BTreeSet<_> = targets.iter().map(|target| target.name.as_str()).collect();
        let facts = Facts::<L6>::new();
        let mut enforcement = Vec::new();
        let mut findings = Vec::new();
        for initial in facts.get::<WorkflowBlueprintFact, _>(blueprints, &WorkflowBlueprintQuery::All).await? {
            let authored = &initial.blueprint;
            if !names.contains(authored.scope.as_str()) { continue }
            let result = async {
                let blueprint = facts.get::<WorkflowBlueprintFact, _>(blueprints,
                    &WorkflowBlueprintQuery::Workflow(authored.id.clone())).await?
                    .into_iter().next().ok_or_else(|| FactoryError::Other(anyhow::anyhow!("workflow blueprint provider returned no subject")))?;
                let targets = facts.get::<WorkflowTargetsFact, _>(preview, &blueprint).await?;
                let mut requirements = Vec::new();
                for category in targets.categories {
                    requirements.push(self.intent.plan_input(&targets.scope, &category).await);
                }
                let read = factory_assurance::workflow_preview::Read { blueprint, requirements };
                facts.get::<WorkflowPreviewFact, _>(preview, &read).await
            }.await;
            match result {
                Ok(observed) => {
                    // Names/scope came from the initial list in the old fold,
                    // even when the authored definition changed during lint.
                    enforcement.extend(observed.enforcement.into_iter().map(|mut row| {
                        row.workflow = authored.id.clone(); row.name = authored.name.clone();
                        row.scope = authored.scope.clone(); row
                    }));
                    findings.extend(observed.findings.into_iter().map(|mut row| {
                        row.workflow = authored.id.clone(); row.name = authored.name.clone();
                        row.scope = authored.scope.clone(); row
                    }));
                }
                Err(error) => findings.push(WorkflowEnforcementFinding {
                    workflow: authored.id.clone(), name: authored.name.clone(),
                    scope: authored.scope.clone(), detail: error.to_string(),
                }),
            }
        }
        Ok((enforcement, findings))
    }

    pub async fn report_with_workflows<K, C, I, B, P>(
        &self, scope: Option<&str>, knowledge: &K, checks: &C, inventory: &I,
        blueprints: &B, preview: &P,
    ) -> Result<PolicyReport>
    where
        K: Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>,
        C: Provide<CheckEvaluationFact, Query = check_evaluation::Read, Value = CheckEvaluationFact, Error = FactoryError>,
        I: Provide<TaskInventoryFact, Query = TaskInventoryQuery, Value = Vec<TaskInventoryFact>, Error = FactoryError>,
        B: Provide<factory_kernel::WorkflowBlueprintFact, Query = factory_kernel::WorkflowBlueprintQuery,
            Value = Vec<factory_kernel::WorkflowBlueprintFact>, Error = FactoryError>,
        P: Provide<factory_kernel::WorkflowTargetsFact, Query = factory_kernel::WorkflowBlueprintFact,
            Value = factory_kernel::WorkflowTargetsFact, Error = FactoryError>
            + Provide<factory_kernel::WorkflowPreviewFact, Query = factory_assurance::workflow_preview::Read,
                Value = factory_kernel::WorkflowPreviewFact, Error = FactoryError>,
    {
        let mut report = self.report(scope, knowledge, checks, inventory).await?;
        let (enforcement, findings) = self.workflow_enforcement(scope, blueprints, preview).await?;
        report.workflow_enforcement = enforcement;
        report.workflow_findings = findings;
        Ok(report)
    }

    async fn budgets(
        &self,
        applied: &[(&policy_intent::Scope, Vec<policy::Applied>)],
    ) -> Result<Vec<Option<BudgetIntent>>> {
        let config = self
            .intent
            .budget_for(
                &applied
                    .iter()
                    .map(|(_, subjects)| subjects.as_slice())
                    .collect::<Vec<_>>(),
            )
            .await?;
        Ok(applied
            .iter()
            .map(|(scope, _)| {
                config
                    .as_ref()
                    .map(|config| self.intent.config.budget_intent(scope, config))
            })
            .collect())
    }

    pub async fn report<K, C, I>(
        &self,
        scope: Option<&str>,
        knowledge: &K,
        checks: &C,
        inventory: &I,
    ) -> Result<PolicyReport>
    where
        K: Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>,
        C: Provide<
            CheckEvaluationFact,
            Query = check_evaluation::Read,
            Value = CheckEvaluationFact,
            Error = FactoryError,
        >,
        I: Provide<
            TaskInventoryFact,
            Query = TaskInventoryQuery,
            Value = Vec<TaskInventoryFact>,
            Error = FactoryError,
        >,
    {
        let (policies, tags) = self.intent.policies_with_tags(knowledge).await?;
        let (asked, targets) = scope_subtree(&self.intent.config.scopes, scope)?;
        let attestations = self.intent.receipts.all().await?;
        let now = chrono::Utc::now();
        let in_subtree: BTreeSet<&str> = targets.iter().map(|t| t.name.as_str()).collect();
        // Load findings about a file in the subtree, every template finding,
        // and every legacy block in the subtree.
        let mut findings: Vec<policy::Finding> = policies
            .findings
            .iter()
            .filter(|finding| {
                asked.is_none()
                    || in_subtree.contains(finding.subject.as_str())
                    || finding.subject.starts_with(".factory/policies/templates/")
                    || targets
                        .iter()
                        .any(|t| finding_is_in(&finding.subject, &t.path))
            })
            .cloned()
            .collect();
        let mut applied = Vec::new();
        let mut not_applicable = BTreeSet::new();
        let mut not_selected: BTreeMap<String, Vec<policy::NotSelected>> = BTreeMap::new();
        // Per framework: the scopes in the subtree with a policy for it.
        let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut summaries: BTreeMap<String, CatalogueSummary> = BTreeMap::new();
        for target in &targets {
            for owned in policies.of(&target.name) {
                owners
                    .entry(owned.framework.clone())
                    .or_default()
                    .push(target.name.clone());
                let summary =
                    summaries
                        .entry(owned.framework.clone())
                        .or_insert_with(|| CatalogueSummary {
                            framework: owned.framework.clone(),
                            title: owned.title.clone(),
                            kind: owned.kind,
                            controls: 0,
                        });
                summary.controls += owned.controls.len();
                not_selected
                    .entry(target.name.clone())
                    .or_default()
                    .extend(owned.not_selected.iter().cloned());
            }
            let subjects = policies.applied(&target.name);
            for subject in &subjects {
                if let Some(na) = &subject.not_applicable {
                    not_applicable.insert((
                        subject.control.clone(),
                        na.scope.clone(),
                        na.rationale.clone(),
                    ));
                }
            }
            if policies.of(&target.name).next().is_some() {
                applied.push((*target, subjects));
            }
        }
        let budgets = self.budgets(&applied).await;
        let read = check_evaluation::Read {
            scopes: inputs(&applied),
            tags,
            attestations,
            budgets,
            now: Some(now),
        };
        let fact = Facts::<L6>::new()
            .get::<CheckEvaluationFact, _>(checks, &read)
            .await?;
        if fact.scopes.len() != applied.len() {
            return Err(FactoryError::Other(anyhow::anyhow!(
                "check results do not match their scopes"
            )));
        }
        let mut rows = Vec::new();
        let mut every_pair = Vec::new();
        for (result, (target, subjects)) in fact.scopes.into_iter().zip(&applied) {
            if result.scope != target.name {
                return Err(FactoryError::Other(anyhow::anyhow!(
                    "check result scope does not match its declaration"
                )));
            }
            findings.extend(result.findings.into_iter().map(|finding| policy::Finding {
                kind: policy::FindingKind::AmbiguousCheckTarget,
                subject: finding.subject,
                detail: finding.detail,
            }));
            let statuses = classified(result.statuses, subjects)?;
            let rollup = policy::rollup(&statuses);
            every_pair.extend(statuses.iter().cloned());
            rows.push(ScopePolicy {
                scope: target.name.clone(),
                statuses,
                rollup,
                not_selected: not_selected.remove(&target.name).unwrap_or_default(),
                open_tasks: BTreeMap::new(),
            });
        }
        if !rows.is_empty() {
            let tasks = Facts::<L6>::new()
                .get::<TaskInventoryFact, _>(inventory, &TaskInventoryQuery::All)
                .await?;
            let mut open = BTreeMap::new();
            for task in tasks {
                if let Some(label) = open_policy_label(&task) {
                    open.entry((task.scope.clone(), label.to_owned()))
                        .or_insert(task.id);
                }
            }
            for row in &mut rows {
                for status in &row.statuses {
                    let label = status.control.to_string();
                    if let Some(id) = open.get(&(row.scope.clone(), label.clone())) {
                        row.open_tasks.insert(label, id.clone());
                    }
                }
            }
        }
        // #390: every (scope, control) pair counts -- a control open in one
        // child and satisfied in another is one open and one satisfied, and
        // the subtree is compliant only when every owning scope is.
        let rollup = policy::rollup(&every_pair);
        findings.sort_by(|a, b| {
            a.subject
                .cmp(&b.subject)
                .then(a.kind.cmp(&b.kind))
                .then(a.detail.cmp(&b.detail))
        });
        findings.dedup();
        let applicability = owners
            .into_iter()
            .map(|(framework, covers)| FrameworkApplicability {
                no_policy: targets
                    .iter()
                    .map(|t| t.name.clone())
                    .filter(|name| !covers.contains(name))
                    .collect(),
                framework,
                covers,
            })
            .collect();
        let templates = templates(&policies);
        Ok(PolicyReport {
            scope: asked.map(|scope| scope.name.clone()),
            rows,
            rollup,
            not_applicable: not_applicable
                .into_iter()
                .map(|(control, scope, rationale)| NotApplicableEntry {
                    control,
                    scope,
                    rationale,
                })
                .collect(),
            applicability,
            findings,
            workflow_enforcement: Vec::new(),
            workflow_findings: Vec::new(),
            catalogues: summaries.into_values().collect(),
            templates,
        })
    }

    /// Every template on disk and the scopes picking from each (#390).
    pub async fn templates(&self) -> Result<Vec<TemplateSummary>> {
        Ok(templates(&self.intent.policies().await?))
    }

    /// One template and, per scope -- `scope` alone, or every scope whose
    /// policy picks from it -- whether each control is selected, `n/a` or
    /// not selected (#390).
    pub async fn template(&self, name: &str, scope: Option<&str>) -> Result<TemplateView> {
        let asked = scope
            .map(|scope| resolve_scope(&self.intent.config.scopes, scope))
            .transpose()?
            .map(|scope| scope.name.clone());
        let policies = self.intent.policies().await?;
        let template = policies.template(name).cloned().ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "no template {name:?} in .factory/policies/templates/ (it has: {})",
                policies
                    .templates
                    .iter()
                    .map(|t| t.template.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let scopes: Vec<String> = match asked {
            Some(scope) => vec![scope],
            None => self
                .intent
                .config
                .scopes
                .iter()
                .filter(|scope| {
                    policies
                        .of(&scope.name)
                        .any(|p| p.template.as_deref() == Some(name))
                })
                .map(|scope| scope.name.clone())
                .collect(),
        };
        let scopes = scopes
            .into_iter()
            .map(|scope| {
                let own = policies
                    .of(&scope)
                    .find(|p| p.template.as_deref() == Some(name));
                let controls = template
                    .controls
                    .iter()
                    .map(|control| {
                        let picked = own
                            .and_then(|p| p.controls.iter().find(|c| c.control.id == control.id));
                        let (selection, rationale) = match picked {
                            Some(applied) => match &applied.not_applicable {
                                Some(na) => (Selection::NotApplicable, Some(na.rationale.clone())),
                                None => (Selection::Selected, None),
                            },
                            None => (Selection::NotSelected, None),
                        };
                        TemplateControlSelection {
                            id: control.id.clone(),
                            title: control.title.clone(),
                            selection,
                            rationale,
                        }
                    })
                    .collect();
                TemplateScopeSelection {
                    framework: own.map(|p| p.framework.clone()),
                    scope,
                    controls,
                }
            })
            .collect();
        Ok(TemplateView { template, scopes })
    }

    pub async fn detail<K, C, I>(
        &self,
        control: ControlRef,
        scope: &str,
        knowledge: &K,
        checks: &C,
        inventory: &I,
    ) -> Result<PolicyControlDetail>
    where
        K: Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>,
        C: Provide<
            CheckEvaluationFact,
            Query = check_evaluation::Read,
            Value = CheckEvaluationFact,
            Error = FactoryError,
        >,
        I: Provide<
            TaskInventoryFact,
            Query = TaskInventoryQuery,
            Value = Vec<TaskInventoryFact>,
            Error = FactoryError,
        >,
    {
        let target = resolve_scope(&self.intent.config.scopes, scope)?;
        let (policies, tags) = self.intent.policies_with_tags(knowledge).await?;
        // Resolved in the scope that owns it (#390), never an ancestor's.
        let applied = policies.applied(&target.name);
        let found = applied
            .iter()
            .find(|subject| subject.control == control)
            .ok_or_else(|| self.intent.config.not_owned(&policies, &control, target))?;
        let mut history: Vec<Attestation> = self
            .intent
            .receipts
            .all()
            .await?
            .into_iter()
            .filter(|receipt| receipt.control == control && receipt.scope == target.name)
            .collect();
        history.sort_by_key(|receipt| std::cmp::Reverse(receipt.attested_at));
        // Full subjects retain maps_to neighbour checks. Only the requested
        // control's raw history is supplied, exactly as the legacy detail.
        let selected = vec![(target, applied.clone())];
        let budgets = self.budgets(&selected).await;
        let read = check_evaluation::Read {
            scopes: inputs(&selected),
            tags,
            attestations: history.clone(),
            budgets,
            now: None,
        };
        let fact = Facts::<L6>::new()
            .get::<CheckEvaluationFact, _>(checks, &read)
            .await?;
        let evaluated = fact
            .scopes
            .into_iter()
            .filter(|scope| scope.scope == target.name)
            .flat_map(|scope| scope.statuses)
            .find(|status| status.control == control)
            .ok_or_else(|| {
                FactoryError::Other(anyhow::anyhow!("{control} evaluated to no status"))
            })?;
        let label = control.to_string();
        let open_task = Facts::<L6>::new()
            .get::<TaskInventoryFact, _>(inventory, &TaskInventoryQuery::Exact(target.name.clone()))
            .await?
            .into_iter()
            .find(|task| open_policy_label(task) == Some(label.as_str()))
            .map(|task| task.id);
        Ok(PolicyControlDetail {
            control: found.control.clone(),
            title: found.title.clone(),
            kind: found.kind,
            checks: found.evidence.clone(),
            maps_to: found.maps_to.clone(),
            max_age: found.max_age,
            not_applicable: found.not_applicable.clone(),
            remediation: found.remediation.clone(),
            refs: evaluated.refs,
            status: evaluated.status,
            open_task,
            attestations: history,
        })
    }
}

/// Every template, with the scopes whose policies pick from it.
fn templates(policies: &policy::Policies) -> Vec<TemplateSummary> {
    policies
        .templates
        .iter()
        .map(|template| TemplateSummary {
            template: template.template.clone(),
            title: template.title.clone(),
            kind: template.kind,
            controls: template.controls.len(),
            used_by: policies
                .owned
                .iter()
                .filter(|p| p.template.as_deref() == Some(template.template.as_str()))
                .map(|p| p.scope.clone())
                .collect(),
        })
        .collect()
}

/// A finding subject that is a file under `scope_path`'s own
/// `.factory/policies/` -- by whole path components, never a name prefix.
fn finding_is_in(subject: &str, scope_path: &std::path::Path) -> bool {
    let label: Vec<String> = scope_path
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let mut parts: Vec<&str> = subject.split('/').collect();
    if parts.len() < label.len() + 3 {
        return false;
    }
    let file = parts.split_off(label.len());
    parts.iter().zip(&label).all(|(a, b)| *a == b)
        && file.len() == 3
        && file[0] == ".factory"
        && file[1] == "policies"
}

pub(crate) fn inputs(
    applied: &[(&policy_intent::Scope, Vec<policy::Applied>)],
) -> Vec<check_evaluation::ScopeInput> {
    applied
        .iter()
        .map(|(scope, subjects)| check_evaluation::ScopeInput {
            scope: ScopeNode {
                name: scope.name.clone(),
                path: scope.path.clone(),
            },
            subjects: subjects
                .iter()
                .map(|applied| {
                    let original = policy::evaluation_subject(applied);
                    EvaluationSubject {
                        control: original.control,
                        title: original.title,
                        kind: (),
                        maps_to: original.maps_to,
                        evidence: original.evidence,
                        max_age: original.max_age,
                        not_applicable: original.not_applicable,
                    }
                })
                .collect(),
        })
        .collect()
}

pub(crate) fn classified(
    statuses: Vec<CheckObservation>,
    applied: &[policy::Applied],
) -> Result<Vec<policy::ControlStatus>> {
    if statuses.len() != applied.len() {
        return Err(FactoryError::Other(anyhow::anyhow!(
            "check results do not match their applicable declarations"
        )));
    }
    statuses
        .into_iter()
        .map(|status| {
            let declaration = applied
                .iter()
                .find(|subject| subject.control == status.control)
                .ok_or_else(|| {
                    FactoryError::Other(anyhow::anyhow!(
                        "check result has no applicable declaration"
                    ))
                })?;
            Ok(policy::ControlStatus {
                control: status.control,
                title: status.title,
                kind: declaration.kind,
                refs: status.refs,
                status: status.status,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_kernel::{FactProvider, ScopeCheckEvaluation, Status, L4, L5};
    use std::{
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
    };
    struct Fixture {
        root: PathBuf,
        receipts: crate::policy_store::PolicyStore,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("factory-policy-service-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(policy::policies_dir(&root)).unwrap();
            std::fs::create_dir_all(crate::budget::budgets_dir(&root)).unwrap();
            let receipts =
                crate::policy_store::PolicyStore::open(&root.join("receipts.sqlite")).unwrap();
            let fixture = Self { root, receipts };
            fixture.catalogue("A", "knowledge");
            for scope in [".", "projects/app"] {
                fixture.policy(scope, "practices", "framework: practices\ntitle: Practices\nkind: best-practice\ncontrols: [{id: p, title: P}]\n");
            }
            fixture
        }
        /// The same CRA policy, owned by both `company` and `app` (#390).
        fn catalogue(&self, title: &str, check: &str) {
            for scope in [".", "projects/app"] {
                self.policy(scope, "cra", &format!("framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - {{id: a, title: {title}, evidence: [{{check: {check}}}]}}\n"));
            }
        }
        fn policy(&self, scope_path: &str, framework: &str, raw: &str) {
            let dir = policy::scope_policies_dir(&self.root, std::path::Path::new(scope_path));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{framework}.yaml")), raw).unwrap();
        }
        fn service(&self) -> Service<'_> {
            self.service_with(&[
                ("parent-id", "company", "."),
                ("child-id", "app", "projects/app"),
            ])
        }
        fn service_with(&self, scopes: &[(&str, &str, &str)]) -> Service<'_> {
            let configuration = policy_intent::Configuration {
                scopes: scopes
                    .iter()
                    .map(|(id, name, path)| policy_intent::Scope {
                        id: (*id).into(),
                        name: (*name).into(),
                        path: (*path).into(),
                        policies: Default::default(),
                    })
                    .collect(),
                root_policies: Default::default(),
                root_name: Some("company".into()),
                instance_name: "instance".into(),
            };
            Service::new(policy_intent::Service::new(
                self.root.clone(),
                configuration,
                &self.receipts,
            ))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[derive(Default)]
    struct Lower {
        tag_calls: AtomicUsize,
        check_calls: AtomicUsize,
        fail_tags: AtomicBool,
        fail_checks: AtomicBool,
        stale: AtomicBool,
        /// Scopes whose every control reads open.
        open_in: Mutex<BTreeSet<String>>,
        histories: Mutex<Vec<Vec<Attestation>>>,
        budgets: Mutex<Vec<Vec<Option<BudgetIntent>>>>,
    }
    impl FactProvider for Lower {
        type Level = L5;
    }
    #[async_trait::async_trait]
    impl Provide<KnowledgeTags> for Lower {
        type Query = ();
        type Value = KnowledgeTags;
        type Error = FactoryError;
        async fn get(&self, _: &()) -> Result<KnowledgeTags> {
            self.tag_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_tags.load(Ordering::SeqCst) {
                return Err(FactoryError::adapter("knowledge", "unavailable"));
            }
            Ok(KnowledgeTags {
                tags: BTreeSet::new(),
            })
        }
    }
    #[async_trait::async_trait]
    impl Provide<CheckEvaluationFact> for Lower {
        type Query = check_evaluation::Read;
        type Value = CheckEvaluationFact;
        type Error = FactoryError;
        async fn get(&self, read: &check_evaluation::Read) -> Result<CheckEvaluationFact> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_checks.load(Ordering::SeqCst) {
                return Err(FactoryError::adapter("checks", "unavailable"));
            }
            self.histories
                .lock()
                .unwrap()
                .push(read.attestations.clone());
            self.budgets.lock().unwrap().push(
                read.budgets
                    .as_ref()
                    .map_err(|error| FactoryError::BadRequest(error.to_string()))?
                    .clone(),
            );
            // This is a predetermined lower-port result, not another evaluator.
            let open_in = self.open_in.lock().unwrap().clone();
            let status_in = |scope: &str| if open_in.contains(scope) {
                Status::Open {
                    reasons: vec!["lower result".into()],
                }
            } else if self.stale.load(Ordering::SeqCst) {
                Status::Stale {
                    reasons: vec!["lower result".into()],
                }
            } else {
                Status::Satisfied {
                    reasons: vec!["lower result".into()],
                }
            };
            Ok(CheckEvaluationFact {
                at: read.now.unwrap_or_else(chrono::Utc::now),
                scopes: read
                    .scopes
                    .iter()
                    .map(|input| ScopeCheckEvaluation {
                        scope: input.scope.name.clone(),
                        findings: Vec::new(),
                        statuses: input
                            .subjects
                            .iter()
                            .map(|subject| CheckObservation {
                                control: subject.control.clone(),
                                title: subject.title.clone(),
                                refs: Vec::new(),
                                status: status_in(&input.scope.name),
                            })
                            .collect(),
                    })
                    .collect(),
            })
        }
    }
    #[derive(Default)]
    struct Inventory {
        rows: Mutex<Vec<TaskInventoryFact>>,
        calls: Mutex<Vec<String>>,
    }
    impl FactProvider for Inventory {
        type Level = L4;
    }
    #[async_trait::async_trait]
    impl Provide<TaskInventoryFact> for Inventory {
        type Query = TaskInventoryQuery;
        type Value = Vec<TaskInventoryFact>;
        type Error = FactoryError;
        async fn get(&self, query: &TaskInventoryQuery) -> Result<Self::Value> {
            self.calls.lock().unwrap().push(match query {
                TaskInventoryQuery::All => "all".into(),
                TaskInventoryQuery::Exact(scope) => scope.clone(),
                TaskInventoryQuery::Members(_) => "members".into(),
            });
            Ok(self
                .rows
                .lock()
                .unwrap()
                .iter()
                .filter(|row| match query {
                    TaskInventoryQuery::All => true,
                    TaskInventoryQuery::Exact(scope) => row.scope == *scope,
                    TaskInventoryQuery::Members(scopes) => scopes.contains(&row.scope),
                })
                .cloned()
                .collect())
        }
    }
    fn receipt(id: &str, scope: &str, hours_ago: i64) -> Attestation {
        Attestation {
            id: id.into(),
            control: "cra/a".parse().unwrap(),
            scope: scope.into(),
            evidence: "evidence".into(),
            note: None,
            attested_by: "owner".into(),
            attested_at: chrono::Utc::now() - chrono::Duration::hours(hours_ago),
            expires_at: chrono::Utc::now() + chrono::Duration::days(3),
            withdrawn: None,
            clock: None,
            corrective: None,
        }
    }

    #[tokio::test]
    async fn report_owns_classification_rollups_catalogue_reload_and_newest_inventory_links() {
        let fixture = Fixture::new();
        let service = fixture.service();
        let lower = Lower::default();
        let inventory = Inventory::default();
        for id in ["newest", "older"] {
            inventory.rows.lock().unwrap().push(TaskInventoryFact {
                id: id.into(),
                title: id.into(),
                scope: "app".into(),
                open: true,
                labels: BTreeMap::from([("policy".into(), "cra/a".into())]),
            });
        }
        let first = service
            .report(Some("projects/app"), &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(first.scope.as_deref(), Some("app"));
        assert_eq!(first.rows.len(), 1);
        assert_eq!(first.rows[0].open_tasks["cra/a"], "newest");
        assert_eq!(first.rows[0].statuses[0].kind, policy::Kind::Regulation);
        assert_eq!(first.rows[0].statuses[1].kind, policy::Kind::BestPractice);
        assert!(
            first
                .rollup
                .iter()
                .find(|rollup| rollup.framework == "cra")
                .unwrap()
                .compliant
        );
        fixture.catalogue("Updated", "knowledge");
        lower.stale.store(true, Ordering::SeqCst);
        let next = service
            .report(None, &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(next.rows.len(), 2);
        assert_eq!(next.rows[0].statuses[0].title, "Updated");
        assert_eq!(
            next.rows[0].statuses[0].status.kind(),
            factory_kernel::StatusKind::Stale
        );
        assert!(
            !next
                .rollup
                .iter()
                .find(|rollup| rollup.framework == "cra")
                .unwrap()
                .compliant
        );
        assert_eq!(*inventory.calls.lock().unwrap(), ["all", "all"]);
    }

    #[tokio::test]
    async fn detail_keeps_full_subject_set_and_live_ancestor_receipt_history() {
        let fixture = Fixture::new();
        let service = fixture.service();
        let lower = Lower::default();
        let inventory = Inventory::default();
        for receipt in [
            receipt("parent", "company", 2),
            receipt("child", "app", 1),
            receipt("sibling", "elsewhere", 0),
        ] {
            fixture.receipts.append_attestation(&receipt).await.unwrap();
        }
        let first = service
            .detail(
                "cra/a".parse().unwrap(),
                "projects/app",
                &lower,
                &lower,
                &inventory,
            )
            .await
            .unwrap();
        assert_eq!(
            first
                .attestations
                .iter()
                .map(|receipt| receipt.id.as_str())
                .collect::<Vec<_>>(),
            ["child"],
            "the owning scope's own history only: an ancestor's never counts (#390)"
        );
        assert_eq!(lower.histories.lock().unwrap()[0], first.attestations);
        let withdrawal = factory_kernel::Withdrawal {
            at: chrono::Utc::now(),
            by: "owner".into(),
            reason: Some("superseded".into()),
        };
        fixture
            .receipts
            .append_withdrawal("child", &"cra/a".parse().unwrap(), "app", &withdrawal)
            .await
            .unwrap();
        let next = service
            .detail("cra/a".parse().unwrap(), "app", &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(next.attestations[0].withdrawn, Some(withdrawal));
        assert_eq!(*inventory.calls.lock().unwrap(), ["app", "app"]);
    }

    /// `projects` groups two projects and owns nothing; `projects/app-other`
    /// shares a name prefix with `projects/app` but is not below it.
    fn tree(fixture: &Fixture) -> Service<'_> {
        for path in [".", "projects/app"] {
            std::fs::remove_dir_all(policy::scope_policies_dir(
                &fixture.root,
                std::path::Path::new(path),
            ))
            .unwrap();
        }
        std::fs::create_dir_all(policy::policies_dir(&fixture.root)).unwrap();
        let cra = "framework: cra\ntitle: CRA\nkind: regulation\ncontrols: [{id: a, title: A, evidence: [{check: attestation}]}]\n";
        for path in ["projects/app", "projects/web", "projects/app-other"] {
            fixture.policy(path, "cra", cra);
        }
        fixture.service_with(&[
            ("root-id", "company", "."),
            ("projects-id", "projects", "projects"),
            ("app-id", "app", "projects/app"),
            ("web-id", "web", "projects/web"),
            ("lookalike-id", "lookalike", "projects/app-other"),
            ("bare-id", "bare", "projects/bare"),
        ])
    }

    #[tokio::test]
    async fn the_subtree_aggregate_counts_every_scope_and_one_open_child_makes_the_parent_non_compliant(
    ) {
        let fixture = Fixture::new();
        let service = tree(&fixture);
        let lower = Lower::default();
        let inventory = Inventory::default();
        lower.open_in.lock().unwrap().insert("web".into());

        // An organizational scope with no policy of its own shows its
        // children's aggregate.
        let projects = service
            .report(Some("projects"), &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(
            projects
                .rows
                .iter()
                .map(|r| r.scope.as_str())
                .collect::<Vec<_>>(),
            ["app", "web", "lookalike"]
        );
        let cra = &projects.rollup[0];
        assert_eq!(cra.framework, "cra");
        assert_eq!(
            (cra.counts.satisfied, cra.counts.open),
            (2, 1),
            "every (scope, control) pair counts, not one status per control"
        );
        assert!(
            !cra.compliant,
            "one child open: the parent is not compliant"
        );
        assert_eq!(
            projects.applicability,
            [FrameworkApplicability {
                framework: "cra".into(),
                covers: vec!["app".into(), "web".into(), "lookalike".into()],
                no_policy: vec!["projects".into(), "bare".into()],
            }],
            "a scope without a policy is listed, never counted"
        );
        assert_eq!(
            projects.applicability[0].describe(),
            "policy at app, web, lookalike; no policy at projects, bare"
        );

        // `projects/app` aggregates only itself, never its lookalike.
        let app = service
            .report(Some("projects/app"), &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(app.rows.len(), 1);
        assert!(app.rollup[0].compliant);
        assert!(app.applicability[0].no_policy.is_empty());

        // The whole instance is the root's view.
        let whole = service
            .report(None, &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(whole.rows.len(), 3);
        assert!(whole.applicability[0]
            .no_policy
            .contains(&"company".to_string()));

        // `detail` resolves in the owning scope only.
        let refused = service
            .detail(
                "cra/a".parse().unwrap(),
                "projects",
                &lower,
                &lower,
                &inventory,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("\"projects\" has no cra policy of its own"),
            "{refused}"
        );
        assert!(
            refused.contains("belongs to app, web, lookalike"),
            "{refused}"
        );
        assert!(service
            .detail("cra/a".parse().unwrap(), "web", &lower, &lower, &inventory)
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn unselected_template_controls_and_legacy_blocks_are_listed_never_counted() {
        let fixture = Fixture::new();
        let service = tree(&fixture);
        let dir = policy::templates_dir(&fixture.root);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cra.yaml"), "template: cra\ntitle: Cyber Resilience Act\nkind: regulation\ncontrols: [{id: a, title: A}, {id: b, title: B}]\n").unwrap();
        fixture.policy(
            "projects/web",
            "cra",
            "framework: cra\ntemplate: cra\ncontrols: [{id: a, evidence: [{check: attestation}]}]\n",
        );
        let lower = Lower::default();
        let inventory = Inventory::default();
        let report = service
            .report(Some("web"), &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(report.rows[0].statuses.len(), 1);
        assert_eq!(
            report.rows[0].not_selected,
            [policy::NotSelected {
                control: "cra/b".parse().unwrap(),
                title: "B".into()
            }]
        );
        assert_eq!(report.rollup[0].counts.satisfied, 1);
        assert_eq!(
            report.rollup[0].counts.open, 0,
            "an unselected control is never counted"
        );
        assert_eq!(report.templates[0].used_by, ["web"]);

        let view = service.template("cra", Some("projects/web")).await.unwrap();
        assert_eq!(view.scopes.len(), 1);
        assert_eq!(view.scopes[0].framework.as_deref(), Some("cra"));
        assert_eq!(
            view.scopes[0]
                .controls
                .iter()
                .map(|c| c.selection)
                .collect::<Vec<_>>(),
            [Selection::Selected, Selection::NotSelected]
        );
        let all = service.template("cra", None).await.unwrap();
        assert_eq!(
            all.scopes
                .iter()
                .map(|s| s.scope.as_str())
                .collect::<Vec<_>>(),
            ["web"]
        );
        let none = service.template("cra", Some("bare")).await.unwrap();
        assert!(none.scopes[0].framework.is_none());
        assert!(service
            .template("nope", None)
            .await
            .unwrap_err()
            .to_string()
            .contains("no template"));

        let mut legacy = policy_intent::Service::new(
            fixture.root.clone(),
            service.intent.config.clone(),
            &fixture.receipts,
        );
        legacy.config.root_policies =
            serde_yaml_ng::from_str("frameworks: [{name: cra, except: [web]}]").unwrap();
        let report = Service::new(legacy)
            .report(None, &lower, &lower, &inventory)
            .await
            .unwrap();
        let found: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == policy::FindingKind::LegacyPolicies)
            .collect();
        assert_eq!(found.len(), 1, "{:?}", report.findings);
        assert_eq!(found[0].subject, "company");
    }

    #[tokio::test]
    async fn raw_budget_inputs_stay_lazy_live_invalid_and_independently_inherited() {
        let fixture = Fixture::new();
        let service = fixture.service();
        let lower = Lower::default();
        let inventory = Inventory::default();
        let limits = crate::budget::catalogue_path(&fixture.root);
        std::fs::write(&limits, "invalid: authored intent\n").unwrap();
        service
            .report(Some("app"), &lower, &lower, &inventory)
            .await
            .unwrap();
        assert!(lower.budgets.lock().unwrap()[0][0].is_none());
        fixture.catalogue("A", "budget_within");
        service
            .report(Some("app"), &lower, &lower, &inventory)
            .await
            .unwrap();
        assert!(lower.budgets.lock().unwrap()[1][0]
            .as_ref()
            .unwrap()
            .error
            .is_some());
        std::fs::write(
            &limits,
            "version: 1\nscopes: {parent-id: {monthly_usd: 50}, child-id: {monthly_usd: 20}}\n",
        )
        .unwrap();
        service
            .report(Some("app"), &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(
            lower.budgets.lock().unwrap()[2][0].as_ref().unwrap().caps,
            [("company".into(), 50.0), ("app".into(), 20.0)]
        );
    }

    #[tokio::test]
    async fn scope_error_order_and_actual_receipt_failure_retry_are_preserved() {
        let fixture = Fixture::new();
        let service = fixture.service();
        let lower = Lower::default();
        let inventory = Inventory::default();
        lower.fail_tags.store(true, Ordering::SeqCst);
        assert_eq!(
            service
                .detail(
                    "cra/a".parse().unwrap(),
                    "missing",
                    &lower,
                    &lower,
                    &inventory
                )
                .await
                .unwrap_err()
                .code(),
            "no_such_scope"
        );
        assert_eq!(lower.tag_calls.load(Ordering::SeqCst), 0);
        assert!(service
            .report(Some("missing"), &lower, &lower, &inventory)
            .await
            .unwrap_err()
            .to_string()
            .contains("knowledge"));
        lower.fail_tags.store(false, Ordering::SeqCst);
        assert_eq!(
            service
                .report(Some("missing"), &lower, &lower, &inventory)
                .await
                .unwrap_err()
                .code(),
            "no_such_scope"
        );
        let connection = rusqlite::Connection::open(fixture.root.join("receipts.sqlite")).unwrap();
        connection
            .execute(
                "ALTER TABLE policy_attestations RENAME TO qa_hidden_receipts",
                [],
            )
            .unwrap();
        assert!(service
            .report(Some("app"), &lower, &lower, &inventory)
            .await
            .unwrap_err()
            .to_string()
            .contains("policy_attestations"));
        assert_eq!(lower.check_calls.load(Ordering::SeqCst), 0);
        connection
            .execute(
                "ALTER TABLE qa_hidden_receipts RENAME TO policy_attestations",
                [],
            )
            .unwrap();
        lower.fail_checks.store(true, Ordering::SeqCst);
        assert!(service
            .report(Some("app"), &lower, &lower, &inventory)
            .await
            .is_err());
        lower.fail_checks.store(false, Ordering::SeqCst);
        assert!(service
            .report(Some("app"), &lower, &lower, &inventory)
            .await
            .is_ok());
    }
}
