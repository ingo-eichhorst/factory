//! Live L6 Policy reports/details. Own declarations and receipts stay here;
//! live check judgements and process inventory cross only typed fact ports.
use crate::{policy, policy_intent, policy_report::*, remediation::open_policy_label};
use factory_assurance::{check_evaluation, checks::EvaluationSubject, evidence::BudgetIntent};
use factory_kernel::{
    resolve_scope, scope_ancestors, scope_subtree, Attestation, CheckEvaluationFact,
    CheckObservation, ControlRef, FactoryError, Facts, KnowledgeTags, Provide, Result, ScopeNode,
    TaskInventoryFact, TaskInventoryQuery, L6,
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
        let (catalogues, mut findings, tags) = self.intent.catalogues_with_tags(knowledge).await?;
        let (asked, targets) = scope_subtree(&self.intent.config.scopes, scope)?;
        let attestations = self.intent.receipts.all().await?;
        let now = chrono::Utc::now();
        let mut applied = Vec::new();
        let mut not_applicable = BTreeSet::new();
        for target in targets {
            let (subjects, chain_findings) =
                policy::applicable(&catalogues, &self.intent.config.chain(&target.name));
            findings.extend(chain_findings);
            for subject in &subjects {
                if let Some(na) = &subject.not_applicable {
                    not_applicable.insert((
                        subject.control.clone(),
                        na.scope.clone(),
                        na.rationale.clone(),
                    ));
                }
            }
            if !subjects.is_empty() {
                applied.push((target, subjects));
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
        let mut per_scope_statuses = Vec::new();
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
            per_scope_statuses.push(statuses.clone());
            rows.push(ScopePolicy {
                scope: target.name.clone(),
                statuses,
                rollup,
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
        let rollup = policy::rollup(&policy::worst_across_scopes(&per_scope_statuses));
        findings.sort_by(|a, b| {
            a.subject
                .cmp(&b.subject)
                .then(a.kind.cmp(&b.kind))
                .then(a.detail.cmp(&b.detail))
        });
        findings.dedup();
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
            findings,
            workflow_enforcement: Vec::new(),
            workflow_findings: Vec::new(),
            catalogues: catalogues
                .iter()
                .map(|catalogue| CatalogueSummary {
                    framework: catalogue.framework.clone(),
                    title: catalogue.title.clone(),
                    kind: catalogue.kind,
                    controls: catalogue.controls.len(),
                })
                .collect(),
        })
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
        let (catalogues, _, tags) = self.intent.catalogues_with_tags(knowledge).await?;
        let (applied, _) = policy::applicable(&catalogues, &self.intent.config.chain(&target.name));
        let found = applied.iter().find(|subject| subject.control == control).ok_or_else(|| {
            FactoryError::BadRequest(format!("{control} does not apply at {:?}, or is not a control any loaded catalogue defines", target.name))
        })?;
        let ancestors: BTreeSet<_> = scope_ancestors(&self.intent.config.scopes, target)
            .into_iter()
            .map(|scope| scope.name.as_str())
            .collect();
        let mut history: Vec<Attestation> = self
            .intent
            .receipts
            .all()
            .await?
            .into_iter()
            .filter(|receipt| {
                receipt.control == control
                    && (receipt.scope == target.name || ancestors.contains(receipt.scope.as_str()))
            })
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
            std::fs::write(policy::policies_dir(&fixture.root).join("practices.yaml"), "framework: practices\ntitle: Practices\nkind: best-practice\ncontrols: [{id: p, title: P}]\n").unwrap();
            fixture
        }
        fn catalogue(&self, title: &str, check: &str) {
            std::fs::write(policy::policies_dir(&self.root).join("cra.yaml"), format!("framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - {{id: a, title: {title}, evidence: [{{check: {check}}}]}}\n")).unwrap();
        }
        fn service(&self) -> Service<'_> {
            let configuration = policy_intent::Configuration {
                scopes: [
                    ("parent-id", "company", "."),
                    ("child-id", "app", "projects/app"),
                ]
                .into_iter()
                .map(|(id, name, path)| policy_intent::Scope {
                    id: id.into(),
                    name: name.into(),
                    path: path.into(),
                    policies: Default::default(),
                })
                .collect(),
                root_policies: serde_yaml_ng::from_str("frameworks: [cra, practices]").unwrap(),
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
            let status = if self.stale.load(Ordering::SeqCst) {
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
                                status: status.clone(),
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
            ["child", "parent"]
        );
        assert_eq!(lower.histories.lock().unwrap()[0], first.attestations);
        let withdrawal = factory_kernel::Withdrawal {
            at: chrono::Utc::now(),
            by: "owner".into(),
            reason: Some("superseded".into()),
        };
        fixture
            .receipts
            .append_withdrawal("parent", &"cra/a".parse().unwrap(), "company", &withdrawal)
            .await
            .unwrap();
        let next = service
            .detail("cra/a".parse().unwrap(), "app", &lower, &lower, &inventory)
            .await
            .unwrap();
        assert_eq!(next.attestations[1].withdrawn, Some(withdrawal));
        assert_eq!(*inventory.calls.lock().unwrap(), ["app", "app"]);
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
