//! Live Process evidence, using only L4's task and immutable evidence stores.
use crate::intake;
use crate::{evidence_store::RunEvidenceStore, run::RunStatus, store::TaskStore};
use crate::{
    task::{Task, TaskFilter},
    workflow::WorkflowDefinition,
    workflow_store::WorkflowStore,
};
use async_trait::async_trait;
use factory_kernel::{
    ArtifactProvenance, ConfirmedSecurityReport, FactoryError, Provide, Result, RunFact, ScopeTree,
    TaskFact, WorkflowFact, WorkflowRunFact,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

const RUN_LOOKBACK: u32 = 20;

#[derive(Clone)]
pub struct NamedQuery {
    pub scope: String,
    pub names: BTreeSet<String>,
}
/// Exact selection never expands a subtree. Members are explicit caller selections.
pub use factory_kernel::TaskInventoryQuery;
pub struct RecoveryQuery {
    pub scopes: Option<BTreeSet<String>>,
    pub limit: u32,
}
pub struct ReleaseBuildQuery {
    pub scope: String,
    pub commit: String,
    pub run_id: String,
}

/// Owns Process gathering without a hub, outside facade or callback.
/// The router supplies only store capabilities and current plain scope identities.
pub struct Provider<'a> {
    store: &'a dyn TaskStore,
    workflows: &'a WorkflowStore,
    scopes: ScopeTree,
    root: PathBuf,
}
impl<'a> Provider<'a> {
    pub fn new(
        store: &'a dyn TaskStore,
        workflows: &'a WorkflowStore,
        scopes: ScopeTree,
        root: PathBuf,
    ) -> Self {
        Self {
            store,
            workflows,
            scopes,
            root,
        }
    }
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L4;
}

pub struct ProvenanceProvider<'a> {
    store: &'a dyn TaskStore,
    evidence: &'a RunEvidenceStore,
}

impl<'a> ProvenanceProvider<'a> {
    pub fn new(store: &'a dyn TaskStore, evidence: &'a RunEvidenceStore) -> Self {
        Self { store, evidence }
    }
}

impl factory_kernel::FactProvider for ProvenanceProvider<'_> {
    type Level = factory_kernel::L4;
}

#[async_trait::async_trait]
impl factory_kernel::Provide<ArtifactProvenance> for ProvenanceProvider<'_> {
    type Query = String;
    type Value = Vec<ArtifactProvenance>;
    type Error = FactoryError;
    async fn get(&self, id: &String) -> Result<Self::Value> {
        let run = self
            .store
            .get_run(id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {id}")))?;
        if run.status != RunStatus::Done {
            return Ok(Vec::new());
        }
        let records = self.evidence.provenance(id).await?;
        Ok(records
            .into_iter()
            .filter(|p| {
                p.run_id == run.id
                    && p.task_id == run.task_id
                    && Some(p.statement.predicate.run_details.metadata.finished_on) == run.ended_at
                    && run.artifacts.iter().any(|a| a == &p.artifact)
            })
            .collect())
    }
}

#[async_trait]
impl Provide<factory_kernel::TaskInventoryFact> for Provider<'_> {
    type Query = TaskInventoryQuery;
    type Value = Vec<factory_kernel::TaskInventoryFact>;
    type Error = FactoryError;
    async fn get(&self, query: &Self::Query) -> Result<Self::Value> {
        let snapshot = &self.scopes;
        let (filter, members) = match query {
            TaskInventoryQuery::All => (TaskFilter::default(), None),
            TaskInventoryQuery::Exact(scope) => (
                TaskFilter {
                    scope: Some(snapshot.scope(scope)?.name.clone()),
                    ..Default::default()
                },
                None,
            ),
            TaskInventoryQuery::Members(scopes) => {
                let members: BTreeSet<String> = scopes
                    .iter()
                    .map(|scope| snapshot.scope(scope).map(|s| s.name.clone()))
                    .collect::<Result<_>>()?;
                // No scopes means no evidence, not an accidental unscoped read.
                if members.is_empty() {
                    return Ok(Vec::new());
                }
                (TaskFilter::default(), Some(members))
            }
        };
        Ok(self
            .store
            .list(&filter)
            .await?
            .into_iter()
            .filter(|task| {
                members.as_ref().is_none_or(|scopes| {
                    scopes.contains(&snapshot.canonical_scope_name(&task.scope))
                })
            })
            .map(|task| factory_kernel::TaskInventoryFact {
                open: !task.status.is_terminal(),
                id: task.id,
                title: task.title,
                scope: task.scope,
                labels: task.labels,
            })
            .collect())
    }
}

#[async_trait]
impl Provide<factory_kernel::TaskSnapshotFact> for Provider<'_> {
    type Query = String;
    type Value = factory_kernel::TaskSnapshotFact;
    type Error = FactoryError;
    async fn get(&self, id: &String) -> Result<Self::Value> {
        let task = self.store.get(id).await?
            .ok_or_else(|| FactoryError::TaskNotFound(id.clone()))?;
        Ok(factory_kernel::TaskSnapshotFact(
            serde_json::to_value(task).map_err(|e| FactoryError::Other(e.into()))?
        ))
    }
}

#[async_trait]
impl Provide<factory_kernel::RunSnapshotFact> for Provider<'_> {
    type Query = factory_kernel::RunSnapshotQuery;
    type Value = factory_kernel::RunSnapshotFact;
    type Error = FactoryError;
    async fn get(&self, query: &factory_kernel::RunSnapshotQuery) -> Result<Self::Value> {
        use factory_kernel::RunSnapshotQuery::*;
        let run = match query {
            Latest(task) => self.store.runs(task, 1).await?.into_iter().next(),
            Active(task) => self.store.active_run(task).await?,
            ById(id) => self.store.get_run(id).await?,
        };
        let value = run
            .map(|run| serde_json::to_value(run).map_err(|e| FactoryError::Other(e.into())))
            .transpose()?;
        Ok(factory_kernel::RunSnapshotFact(value))
    }
}

#[async_trait]
impl Provide<factory_kernel::AgentReportedFact> for Provider<'_> {
    type Query = String;
    type Value = factory_kernel::AgentReportedFact;
    type Error = FactoryError;
    async fn get(&self, run_id: &String) -> Result<Self::Value> {
        let reported = self
            .store
            .run_entries(run_id, 500)
            .await
            .unwrap_or_default()
            .iter()
            .any(|e| e.source == "agent" && (e.kind == "done" || e.kind == "failed"));
        Ok(factory_kernel::AgentReportedFact(reported))
    }
}

#[async_trait]
impl Provide<factory_kernel::ScheduledRunDatesFact> for Provider<'_> {
    type Query = ();
    type Value = factory_kernel::ScheduledRunDatesFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        let snapshot = &self.scopes;
        let tasks = self.store.list(&TaskFilter::default()).await?;
        let runs = tasks
            .into_iter()
            .filter(|task| task.schedule.is_some() && !task.schedule_paused && task.fires())
            .filter_map(|task| {
                task.next_run_at
                    .map(|next_run_at| factory_kernel::ScheduledRunDate {
                        task: task.id,
                        title: task.title,
                        scope: snapshot.canonical_scope_name(&task.scope),
                        agent: task.agent,
                        next_run_at,
                    })
            })
            .collect();
        Ok(factory_kernel::ScheduledRunDatesFact { runs })
    }
}

#[async_trait]
impl Provide<factory_kernel::RecoveryJournalFact> for Provider<'_> {
    type Query = RecoveryQuery;
    type Value = factory_kernel::RecoveryJournalFact;
    type Error = FactoryError;
    async fn get(&self, query: &RecoveryQuery) -> Result<Self::Value> {
        let findings = self.import_recovery_journal().await;
        let scopes: Vec<Option<&str>> = query
            .scopes
            .as_ref()
            .map(|scopes| scopes.iter().map(|scope| Some(scope.as_str())).collect())
            .unwrap_or_else(|| vec![None]);
        let mut actions = Vec::new();
        for scope in scopes {
            actions.extend(self.workflows.recovery_actions(scope, query.limit).await?);
        }
        actions.sort_by(|a, b| {
            b.started_at
                .cmp(&a.started_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        actions.truncate(query.limit.clamp(1, 200) as usize);
        Ok(factory_kernel::RecoveryJournalFact { actions, findings })
    }
}

#[async_trait]
impl Provide<factory_kernel::EnvironmentRecoveryFact> for Provider<'_> {
    type Query = RecoveryQuery;
    type Value = Vec<factory_kernel::EnvironmentRecoveryFact>;
    type Error = FactoryError;
    async fn get(&self, query: &RecoveryQuery) -> Result<Self::Value> {
        use factory_kernel::{
            RECOVERY_COMMIT_LABEL, RECOVERY_ENVIRONMENT_LABEL, RECOVERY_REASON_LABEL,
        };
        let limit = query.limit.clamp(1, 200);
        let scopes: Vec<Option<&str>> = match &query.scopes {
            Some(scopes) => scopes.iter().map(|scope| Some(scope.as_str())).collect(),
            None => vec![None],
        };
        let mut workflows = Vec::new();
        for scope in scopes {
            workflows.extend(
                self.workflows
                    .tagged_runs(RECOVERY_ENVIRONMENT_LABEL, scope, limit)
                    .await?,
            );
        }
        workflows.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        workflows.truncate(limit as usize);
        let mut facts = Vec::new();
        for workflow in workflows {
            let Some(node) = workflow
                .definition
                .nodes
                .iter()
                .find(|node| node.task.labels.contains_key(RECOVERY_ENVIRONMENT_LABEL))
            else {
                continue;
            };
            let task_id = workflow
                .nodes
                .iter()
                .find(|run| run.node_id == node.id)
                .and_then(|run| run.task_id.clone());
            let run = match &task_id {
                Some(id) => self
                    .store
                    .runs(id, 1)
                    .await?
                    .into_iter()
                    .next()
                    .map(|run| RunFact {
                        id: run.id,
                        status: run.status,
                        started_at: run.started_at,
                        ended_at: run.ended_at,
                    }),
                None => None,
            };
            let requested_by = match workflow.started_by {
                crate::workflow::WorkflowActor::Owner => "owner".into(),
                crate::workflow::WorkflowActor::Agent { scope, name } => format!("{scope}/{name}"),
            };
            facts.push(factory_kernel::EnvironmentRecoveryFact {
                scope: workflow.scope,
                environment: node.task.labels[RECOVERY_ENVIRONMENT_LABEL].clone(),
                workflow_id: workflow.workflow_id,
                workflow_run_id: workflow.id,
                status: workflow.status,
                requested_at: workflow.created_at,
                requested_by,
                reason: node
                    .task
                    .labels
                    .get(RECOVERY_REASON_LABEL)
                    .cloned()
                    .unwrap_or_default(),
                expected_commit: node.task.labels.get(RECOVERY_COMMIT_LABEL).cloned(),
                task_id,
                run,
            });
        }
        Ok(facts)
    }
}

#[async_trait]
impl Provide<TaskFact> for Provider<'_> {
    type Query = NamedQuery;
    type Value = BTreeMap<String, Vec<TaskFact>>;
    type Error = FactoryError;
    async fn get(&self, q: &NamedQuery) -> Result<Self::Value> {
        if q.names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let snapshot = &self.scopes;
        let scope = snapshot.scope(&q.scope)?.name.clone();
        let tasks = self
            .store
            .list(&TaskFilter {
                scope: Some(scope),
                ..Default::default()
            })
            .await?;
        let mut facts = BTreeMap::new();
        for name in &q.names {
            facts.insert(name.clone(), self.task_facts_for(&tasks, name).await?);
        }
        Ok(facts)
    }
}

#[async_trait]
impl Provide<WorkflowFact> for Provider<'_> {
    type Query = NamedQuery;
    type Value = BTreeMap<String, Vec<WorkflowFact>>;
    type Error = FactoryError;
    async fn get(&self, q: &NamedQuery) -> Result<Self::Value> {
        if q.names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let snapshot = &self.scopes;
        let scope = snapshot.scope(&q.scope)?.name.clone();
        let defs = self.workflows.definitions(Some(&scope)).await?;
        let mut facts = BTreeMap::new();
        for name in &q.names {
            facts.insert(
                name.clone(),
                self.workflow_facts_for(&defs, &scope, name).await?,
            );
        }
        Ok(facts)
    }
}

impl Provider<'_> {
    async fn task_facts_for(&self, scoped: &[Task], name: &str) -> Result<Vec<TaskFact>> {
        if let Some(task) = scoped.iter().find(|t| t.id == name) {
            return Ok(vec![self.task_fact(task).await?]);
        }
        let matches: Vec<&Task> = scoped.iter().filter(|t| t.title == name).collect();
        if let [only] = matches.as_slice() {
            return Ok(vec![self.task_fact(only).await?]);
        }
        Ok(matches
            .into_iter()
            .map(|t| TaskFact {
                id: t.id.clone(),
                title: t.title.clone(),
                runs: Vec::new(),
            })
            .collect())
    }

    async fn task_fact(&self, task: &Task) -> Result<TaskFact> {
        // Newest first (`TaskStore::runs`'s own contract), bounded to
        // `RUN_LOOKBACK` rather than the task's whole history: `evaluate`
        // only ever needs to walk past however many runs are still in
        // progress to find the newest *finished* one, and a task normally
        // has at most one of those at a time.
        let runs = self
            .store
            .runs(&task.id, RUN_LOOKBACK)
            .await?
            .into_iter()
            .map(|r| RunFact {
                id: r.id,
                status: r.status,
                started_at: r.started_at,
                ended_at: r.ended_at,
            })
            .collect();
        Ok(TaskFact {
            id: task.id.clone(),
            title: task.title.clone(),
            runs,
        })
    }

    async fn workflow_facts_for(
        &self,
        defs: &[WorkflowDefinition],
        scope: &str,
        name: &str,
    ) -> Result<Vec<WorkflowFact>> {
        if let Some(def) = defs.iter().find(|d| d.id == name) {
            return Ok(vec![self.workflow_fact(def, scope).await?]);
        }
        let matches: Vec<&WorkflowDefinition> = defs.iter().filter(|d| d.name == name).collect();
        if let [only] = matches.as_slice() {
            return Ok(vec![self.workflow_fact(only, scope).await?]);
        }
        Ok(matches
            .into_iter()
            .map(|d| WorkflowFact {
                id: d.id.clone(),
                name: d.name.clone(),
                runs: Vec::new(),
            })
            .collect())
    }

    async fn workflow_fact(&self, def: &WorkflowDefinition, scope: &str) -> Result<WorkflowFact> {
        let runs = self
            .workflows
            .runs(Some(&def.id), Some(scope), RUN_LOOKBACK)
            .await?
            .into_iter()
            .map(|r| WorkflowRunFact {
                id: r.id,
                status: r.status,
                updated_at: r.updated_at,
            })
            .collect();
        Ok(WorkflowFact {
            id: def.id.clone(),
            name: def.name.clone(),
            runs,
        })
    }

    pub async fn confirmed_security_reports(
        &self,
        scope: Option<&str>,
    ) -> Result<Vec<intake::ConfirmedSecurityReport>> {
        let snapshot = &self.scopes;
        let members: Option<BTreeSet<String>> = match scope {
            None => None,
            Some(name) => {
                let (asked, subtree) = snapshot.subtree_scopes(Some(name))?;
                let mut members: BTreeSet<String> =
                    subtree.into_iter().map(|s| s.name.clone()).collect();
                if let Some(asked) = asked {
                    members.insert(asked.name.clone());
                }
                Some(members)
            }
        };
        let all = self.store.list(&TaskFilter::default()).await?;
        let mut reports: Vec<intake::ConfirmedSecurityReport> = all
            .iter()
            .filter(|t| members.as_ref().is_none_or(|m| m.contains(&t.scope)))
            .filter_map(intake::confirmed_report)
            .collect();
        reports.sort_by(|a, b| {
            a.awareness_at
                .cmp(&b.awareness_at)
                .then(a.item.cmp(&b.item))
        });
        Ok(reports)
    }

    pub async fn import_recovery_journal(&self) -> Vec<String> {
        let snapshot = &self.scopes;
        let root = self.root.clone();
        let loaded =
            tokio::task::spawn_blocking(move || crate::recovery_journal::load(&root)).await;
        let mut findings = Vec::new();
        if let Ok(Ok((receipts, rejected))) = loaded {
            if rejected > 0 {
                findings.push(format!(
                    "{rejected} invalid or unreadable offline receipts were rejected"
                ));
            }
            let mut conflicts = 0;
            let mut archive_failed = 0;
            for receipt in receipts {
                if snapshot.scope(&receipt.scope).is_err()
                    || self.workflows.import_recovery(&receipt).await.is_err()
                {
                    conflicts += 1;
                    continue;
                }
                let root = self.root.clone();
                if !matches!(
                    tokio::task::spawn_blocking(move || crate::recovery_journal::archive(
                        &root, &receipt
                    ))
                    .await,
                    Ok(Ok(()))
                ) {
                    archive_failed += 1;
                }
            }
            if conflicts > 0 {
                findings.push(format!(
                "{conflicts} receipts had an unknown scope or conflicted with immutable history"
            ));
            }
            if archive_failed > 0 {
                findings.push(format!(
                    "{archive_failed} completed receipts were stored but could not be archived"
                ));
            }
        } else {
            findings.push(
                "offline recovery outbox could not be imported; stored history remains available"
                    .into(),
            );
        }
        findings
    }
}

#[async_trait]
impl Provide<factory_kernel::ReleaseBuildFact> for ProvenanceProvider<'_> {
    type Query = ReleaseBuildQuery;
    type Value = Option<factory_kernel::ReleaseBuildFact>;
    type Error = FactoryError;
    async fn get(&self, query: &Self::Query) -> Result<Self::Value> {
        let Some(run) = self.store.get_run(&query.run_id).await? else {
            return Ok(None);
        };
        let Some(task) = self.store.get(&run.task_id).await? else {
            return Ok(None);
        };
        if task.scope != query.scope || run.status != RunStatus::Done {
            return Ok(None);
        }
        let artifacts: Vec<_> = Provide::<ArtifactProvenance>::get(self, &run.id)
            .await?
            .into_iter()
            .filter(|record| {
                record.scope == query.scope
                    && record.artifact.scope == query.scope
                    && record.artifact.source.commit == query.commit
                    && !record.artifact.source.dirty
            })
            .collect();
        if artifacts.is_empty() {
            return Ok(None);
        }
        // Revalidate the run exactly as the former run_attestations bridge did.
        self.store
            .get_run(&run.id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {}", run.id)))?;
        let attestations = self.evidence.step_attestations(&run.id).await?;
        Ok(Some(factory_kernel::ReleaseBuildFact {
            scope: query.scope.clone(),
            commit: query.commit.clone(),
            task_id: task.id,
            run: RunFact {
                id: run.id,
                status: run.status,
                started_at: run.started_at,
                ended_at: run.ended_at,
            },
            artifacts,
            attestations,
        }))
    }
}

#[async_trait]
impl Provide<ConfirmedSecurityReport> for Provider<'_> {
    type Query = Option<String>;
    type Value = Vec<ConfirmedSecurityReport>;
    type Error = FactoryError;
    async fn get(&self, scope: &Self::Query) -> Result<Self::Value> {
        self.confirmed_security_reports(scope.as_deref()).await
    }
}
