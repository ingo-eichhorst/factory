//! L5's live check-evidence service. Lower state is read through typed facts;
//! authored subjects, receipts and budget limits are downward command inputs.
//! Same-level benchmark and knowledge reads stay within assurance. There is no
//! router callback, precomputed evidence snapshot or status cache here.
use crate::{
    budget,
    checks::{Check, CheckSource, Evidence},
    facts, metrics, quality,
};
use chrono::{DateTime, Utc};
use factory_kernel::{
    AgentFact, Attestation, AttestedRun, BackupFact, CostGroupBy, CostReport, CostRow,
    DaemonConfigFact, DependenciesFact, FactoryError, Facts, GateFact, KnowledgeTags, Provide,
    Result, ScopeNode, ScopeTree, SecretsPresence, TaskFact, WorkflowFact, L5,
};
use factory_process::{
    facts::NamedQuery, measurements::AttestedQuery, usage::SpendQuery, window::Window,
};
use std::collections::{BTreeMap, BTreeSet};

pub fn gate_dataset_names(applied: &[impl CheckSource]) -> BTreeSet<String> {
    applied
        .iter()
        .flat_map(|a| a.checks())
        .filter_map(|check| match check {
            Check::Gate { dataset, .. } => Some(dataset.clone()),
            _ => None,
        })
        .collect()
}

pub fn needs_agent_facts(applied: &[impl CheckSource]) -> bool {
    applied
        .iter()
        .flat_map(|a| a.checks())
        .any(|check| matches!(check, Check::Roles { .. } | Check::Sandbox))
}

pub fn needs_secrets_facts(applied: &[impl CheckSource]) -> bool {
    applied
        .iter()
        .flat_map(|a| a.checks())
        .any(|check| matches!(check, Check::Secrets { .. }))
}

pub fn needs_daemon_facts(applied: &[impl CheckSource]) -> bool {
    applied
        .iter()
        .flat_map(|a| a.checks())
        .any(|check| matches!(check, Check::Daemon { .. }))
}

pub fn needs_backup_facts(applied: &[impl CheckSource]) -> bool {
    applied.iter().flat_map(|a| a.checks()).any(|check| {
        matches!(
            check,
            Check::Daemon { fact } if matches!(fact.as_str(), "backup_recent" | "backup_offsite" | "backup_verified")
        )
    })
}

pub fn needs_dependencies_facts(applied: &[impl CheckSource]) -> bool {
    applied
        .iter()
        .flat_map(|a| a.checks())
        .any(|check| matches!(check, Check::Dependencies { .. }))
}

pub fn needs_attested_facts(applied: &[impl CheckSource]) -> bool {
    applied
        .iter()
        .flat_map(|a| a.checks())
        .any(|check| matches!(check, Check::Attested { .. }))
}

pub fn needs_budget_facts(applied: &[impl CheckSource]) -> bool {
    applied
        .iter()
        .flat_map(|a| a.checks())
        .any(|check| matches!(check, Check::BudgetWithin))
}

pub fn attested_categories(
    applied: &[impl CheckSource],
) -> (BTreeSet<String>, Option<factory_kernel::Duration>) {
    let mut categories = BTreeSet::new();
    let mut widest: Option<factory_kernel::Duration> = None;
    for a in applied {
        if !a
            .checks()
            .iter()
            .any(|c| matches!(c, Check::Attested { .. }))
        {
            continue;
        }
        for check in a.checks() {
            if let Check::Attested { category, .. } = check {
                categories.insert(category.clone());
            }
        }
        if let Some(w) = a.max_age() {
            widest = Some(widest.map_or(w, |cur| cur.max(w)));
        }
    }
    (categories, widest)
}

/// Each capability can return only its producer's own fact. Wiring may supply
/// concrete owners without making assurance import non-adjacent level crates.
pub trait Ports {
    type Tasks: Provide<
        TaskFact,
        Query = NamedQuery,
        Value = BTreeMap<String, Vec<TaskFact>>,
        Error = FactoryError,
    >;
    type Workflows: Provide<
        WorkflowFact,
        Query = NamedQuery,
        Value = BTreeMap<String, Vec<WorkflowFact>>,
        Error = FactoryError,
    >;
    type Agents: Provide<AgentFact, Query = String, Value = Vec<AgentFact>, Error = FactoryError>;
    type Daemon: Provide<
        DaemonConfigFact,
        Query = (),
        Value = DaemonConfigFact,
        Error = FactoryError,
    >;
    type Secrets: Provide<
        SecretsPresence,
        Query = BTreeSet<String>,
        Value = BTreeMap<String, SecretsPresence>,
        Error = FactoryError,
    >;
    type Dependencies: Provide<
        DependenciesFact,
        Query = String,
        Value = DependenciesFact,
        Error = FactoryError,
    >;
    type Backup: Provide<
        BackupFact,
        Query = DateTime<Utc>,
        Value = BackupFact,
        Error = FactoryError,
    >;
    type Attested: Provide<
        AttestedRun,
        Query = AttestedQuery,
        Value = Vec<AttestedRun>,
        Error = FactoryError,
    >;
    type Spend: Provide<CostReport, Query = SpendQuery, Value = CostReport, Error = FactoryError>;
    fn tasks(&self) -> &Self::Tasks;
    fn workflows(&self) -> &Self::Workflows;
    fn agents(&self) -> &Self::Agents;
    fn daemon(&self) -> &Self::Daemon;
    fn secrets(&self) -> &Self::Secrets;
    fn dependencies(&self) -> &Self::Dependencies;
    fn backup(&self) -> &Self::Backup;
    fn attested(&self) -> &Self::Attested;
    fn spend(&self) -> &Self::Spend;
}

/// Applicable authored caps in ancestor-to-child order, with no spend or
/// verdict. L6 resolves its own catalogue ids; L5 gathers the live evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct BudgetIntent {
    pub caps: Vec<(String, f64)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Shared {
    pub gates: BTreeMap<String, GateFact>,
    pub daemon: Option<DaemonConfigFact>,
    pub credentials: BTreeMap<String, SecretsPresence>,
    pub backup: Option<BackupFact>,
}

pub struct QualityScope<'a> {
    pub scope: ScopeNode,
    pub tree: &'a quality::QualityTree,
    pub budget: Option<BudgetIntent>,
}

pub struct Service<'a, P> {
    ports: P,
    own: facts::Provider<'a>,
    scopes: ScopeTree,
}

impl<'a, P: Ports> Service<'a, P> {
    pub fn new(ports: P, own: facts::Provider<'a>, scopes: ScopeTree) -> Self {
        Self { ports, own, scopes }
    }

    /// Referenced names only; even an empty query goes to the producing
    /// owner's fast path, preserving unknown versus honestly empty history.
    pub async fn named(
        &self,
        scope: &str,
        applied: &[impl CheckSource],
    ) -> Result<(
        BTreeMap<String, Vec<TaskFact>>,
        BTreeMap<String, Vec<WorkflowFact>>,
    )> {
        let mut tasks = BTreeSet::new();
        let mut workflows = BTreeSet::new();
        for check in applied.iter().flat_map(CheckSource::checks) {
            match check {
                Check::Task { task, .. } => {
                    tasks.insert(task.clone());
                }
                Check::Workflow { workflow, .. } => {
                    workflows.insert(workflow.clone());
                }
                _ => {}
            }
        }
        let facts = Facts::<L5>::new();
        let tasks = facts
            .get::<TaskFact, _>(
                self.ports.tasks(),
                &NamedQuery {
                    scope: scope.into(),
                    names: tasks,
                },
            )
            .await?;
        let workflows = facts
            .get::<WorkflowFact, _>(
                self.ports.workflows(),
                &NamedQuery {
                    scope: scope.into(),
                    names: workflows,
                },
            )
            .await?;
        Ok((tasks, workflows))
    }

    pub async fn gates(&self, names: &BTreeSet<String>) -> Result<BTreeMap<String, GateFact>> {
        Provide::<GateFact>::get(&self.own, names).await
    }

    /// Dataset, daemon and backup reads occur once over the union of the
    /// requested scopes. Secrets are requested only for scopes needing them.
    pub async fn shared<S: CheckSource>(&self, per_scope: &[(&str, &[S])]) -> Result<Shared> {
        let mut names = BTreeSet::new();
        for (_, applied) in per_scope {
            names.extend(gate_dataset_names(applied));
        }
        let gates = self.gates(&names).await?;
        let facts = Facts::<L5>::new();
        let daemon = if per_scope.iter().any(|(_, a)| needs_daemon_facts(a)) {
            Some(
                facts
                    .get::<DaemonConfigFact, _>(self.ports.daemon(), &())
                    .await?,
            )
        } else {
            None
        };
        let credentials = if per_scope.iter().any(|(_, a)| needs_secrets_facts(a)) {
            let scopes = per_scope
                .iter()
                .filter(|(_, a)| needs_secrets_facts(a))
                .map(|(s, _)| s.to_string())
                .collect();
            facts
                .get::<SecretsPresence, _>(self.ports.secrets(), &scopes)
                .await?
        } else {
            BTreeMap::new()
        };
        let backup = if per_scope.iter().any(|(_, a)| needs_backup_facts(a)) {
            Some(
                facts
                    .get::<BackupFact, _>(self.ports.backup(), &Utc::now())
                    .await?,
            )
        } else {
            None
        };
        Ok(Shared {
            gates,
            daemon,
            credentials,
            backup,
        })
    }

    /// Exact-scope finished runs, over twice the widest effective check age:
    /// every stale lookback has enough history. None means never gathered.
    pub async fn attested(
        &self,
        scope: &str,
        applied: &[impl CheckSource],
    ) -> Result<Option<Vec<AttestedRun>>> {
        if !needs_attested_facts(applied) {
            return Ok(None);
        }
        let (categories, widest) = attested_categories(applied);
        let w = widest
            .unwrap_or(factory_kernel::Duration::from_hours(0))
            .as_time_delta();
        let now = Utc::now();
        let query = AttestedQuery {
            scopes: Some(std::iter::once(scope.to_string()).collect()),
            categories: Some(categories),
            window: Window {
                from: now - (w + w),
                to: now,
            },
        };
        Ok(Some(
            Facts::<L5>::new()
                .get::<AttestedRun, _>(self.ports.attested(), &query)
                .await?,
        ))
    }

    pub async fn budget(
        &self,
        intent: &BudgetIntent,
        now: DateTime<Utc>,
    ) -> Result<budget::PolicyInput> {
        let month = budget::Month::at(now).map_err(FactoryError::BadRequest)?;
        let mut input = budget::PolicyInput {
            month,
            caps: Vec::new(),
            error: intent.error.clone(),
        };
        if input.error.is_some() {
            return Ok(input);
        }
        for (scope, limit) in &intent.caps {
            let spend = self
                .month_spend(Some(scope.clone()), CostGroupBy::Scope, &input.month)
                .await?;
            input.caps.push(budget::PolicyCap {
                scope: scope.clone(),
                monthly_usd: *limit,
                spend,
            });
        }
        Ok(input)
    }

    /// At the month's first instant, known empty spend does not call a
    /// provider with an invalid zero-width window or invent missing evidence.
    pub async fn month_spend(
        &self,
        scope: Option<String>,
        group_by: CostGroupBy,
        month: &budget::Month,
    ) -> Result<CostReport> {
        if month.from == month.as_of {
            return Ok(CostReport {
                basis: factory_kernel::SpendBasis::Started,
                finished: None,
                group_by,
                from: month.from,
                to: month.as_of,
                scope,
                rows: Vec::new(),
                total: CostRow::new("total", None),
                unattributed_runs: 0,
                daily: Vec::new(),
            });
        }
        Facts::<L5>::new()
            .get::<CostReport, _>(
                self.ports.spend(),
                &SpendQuery {
                    scope,
                    from: Some(month.from),
                    to: Some(month.as_of),
                    group_by,
                    ..Default::default()
                },
            )
            .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn for_scope(
        &self,
        scope: &ScopeNode,
        applied: &[impl CheckSource],
        tags: &BTreeSet<String>,
        attestations: &[Attestation],
        shared: &Shared,
        budget: Option<&BudgetIntent>,
        now: DateTime<Utc>,
    ) -> Result<Evidence> {
        let ancestors: BTreeSet<&str> = self
            .scopes
            .ancestors_of(scope)
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let (tasks, workflows) = self.named(&scope.name, applied).await?;
        let facts = Facts::<L5>::new();
        let agents = if needs_agent_facts(applied) {
            Some(
                facts
                    .get::<AgentFact, _>(self.ports.agents(), &scope.name)
                    .await?,
            )
        } else {
            None
        };
        let secrets = if needs_secrets_facts(applied) {
            shared
                .credentials
                .get(&scope.name)
                .cloned()
                .unwrap_or_default()
        } else {
            SecretsPresence::default()
        };
        let dependencies = if needs_dependencies_facts(applied) {
            Some(
                facts
                    .get::<DependenciesFact, _>(self.ports.dependencies(), &scope.name)
                    .await?,
            )
        } else {
            None
        };
        let attested = self.attested(&scope.name, applied).await?;
        let budget = if needs_budget_facts(applied) {
            match budget {
                Some(intent) => Some(self.budget(intent, now).await?),
                None => None,
            }
        } else {
            None
        };
        Ok(Evidence {
            tags: tags.clone(),
            attestations: attestations
                .iter()
                .filter(|att| att.scope == scope.name || ancestors.contains(att.scope.as_str()))
                .cloned()
                .collect(),
            tasks,
            workflows,
            gates: shared.gates.clone(),
            agents,
            secrets,
            daemon: shared.daemon,
            dependencies,
            backup: shared.backup.clone(),
            attested,
            budget,
        })
    }

    /// Actual live Quality judgement belongs to L5, not a callback into the
    /// router or L6 policy evaluation. Metric values are already computed so
    /// quality.* measures cannot recursively call themselves.
    pub async fn judge_quality(
        &self,
        scopes: &[QualityScope<'_>],
        values: &BTreeMap<metrics::MetricId, metrics::MetricValue>,
        now: DateTime<Utc>,
    ) -> Result<(Vec<quality::ScopeReport>, Vec<quality::Finding>)> {
        let applied: Vec<_> = scopes
            .iter()
            .map(|s| quality::check_subjects(s.tree))
            .collect();
        let needs_tags = applied
            .iter()
            .flatten()
            .flat_map(CheckSource::checks)
            .any(|c| matches!(c, Check::Knowledge { .. }));
        let tags = if needs_tags {
            Provide::<KnowledgeTags>::get(&self.own, &()).await?.tags
        } else {
            BTreeSet::new()
        };
        let targets: Vec<_> = scopes
            .iter()
            .zip(&applied)
            .map(|(s, a)| (s.scope.name.as_str(), a.as_slice()))
            .collect();
        let shared = self.shared(&targets).await?;
        let mut reports = Vec::new();
        let mut findings = Vec::new();
        for (scope, applied) in scopes.iter().zip(&applied) {
            let evidence = self
                .for_scope(
                    &scope.scope,
                    applied,
                    &tags,
                    &[],
                    &shared,
                    scope.budget.as_ref(),
                    now,
                )
                .await?;
            findings.extend(
                crate::checks::evidence_findings(&evidence, &scope.scope.name)
                    .into_iter()
                    .map(|f| quality::Finding {
                        kind: quality::FindingKind::AmbiguousCheckTarget,
                        subject: f.subject,
                        detail: f.detail,
                    }),
            );
            reports.push(quality::evaluate(scope.tree, values, &evidence, now));
        }
        Ok((reports, findings))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bench_store::BenchStore, checks::EvaluationSubject};
    use async_trait::async_trait;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    };

    #[derive(Default)]
    struct Recorder {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        generation: AtomicUsize,
        failing: Mutex<Option<&'static str>>,
    }
    impl Recorder {
        fn record(&self, kind: &str, arguments: Vec<String>) -> Result<()> {
            self.calls.lock().unwrap().push((kind.into(), arguments));
            if self
                .failing
                .lock()
                .unwrap()
                .is_some_and(|name| name == kind)
            {
                return Err(FactoryError::BadRequest(format!("{kind} unavailable")));
            }
            Ok(())
        }
        fn calls(&self, kind: &str) -> Vec<Vec<String>> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(name, _)| name == kind)
                .map(|(_, args)| args.clone())
                .collect()
        }
    }
    macro_rules! owner {
        ($name:ident, $level:ty) => {
            struct $name(Arc<Recorder>);
            impl factory_kernel::FactProvider for $name {
                type Level = $level;
            }
        };
    }
    owner!(Infrastructure, factory_kernel::L1);
    owner!(Environment, factory_kernel::L2);
    owner!(Agents, factory_kernel::L3);
    owner!(Process, factory_kernel::L4);

    #[async_trait]
    impl Provide<TaskFact> for Process {
        type Query = NamedQuery;
        type Value = BTreeMap<String, Vec<TaskFact>>;
        type Error = FactoryError;
        async fn get(&self, q: &NamedQuery) -> Result<Self::Value> {
            self.0.record(
                "tasks",
                std::iter::once(q.scope.clone())
                    .chain(q.names.iter().cloned())
                    .collect(),
            )?;
            Ok(q.names
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        vec![TaskFact {
                            id: format!("{}-{}", q.scope, self.0.generation.load(Ordering::SeqCst)),
                            title: name.clone(),
                            runs: Vec::new(),
                        }],
                    )
                })
                .collect())
        }
    }
    #[async_trait]
    impl Provide<WorkflowFact> for Process {
        type Query = NamedQuery;
        type Value = BTreeMap<String, Vec<WorkflowFact>>;
        type Error = FactoryError;
        async fn get(&self, q: &NamedQuery) -> Result<Self::Value> {
            self.0.record(
                "workflows",
                std::iter::once(q.scope.clone())
                    .chain(q.names.iter().cloned())
                    .collect(),
            )?;
            Ok(BTreeMap::new())
        }
    }
    #[async_trait]
    impl Provide<AttestedRun> for Process {
        type Query = AttestedQuery;
        type Value = Vec<AttestedRun>;
        type Error = FactoryError;
        async fn get(&self, q: &AttestedQuery) -> Result<Self::Value> {
            self.0.record(
                "attested",
                q.scopes
                    .iter()
                    .flatten()
                    .cloned()
                    .chain(q.categories.iter().flatten().cloned())
                    .chain([q.window.from.to_rfc3339(), q.window.to.to_rfc3339()])
                    .collect(),
            )?;
            Ok(Vec::new())
        }
    }
    #[async_trait]
    impl Provide<CostReport> for Process {
        type Query = SpendQuery;
        type Value = CostReport;
        type Error = FactoryError;
        async fn get(&self, q: &SpendQuery) -> Result<Self::Value> {
            self.0.record(
                "spend",
                vec![
                    q.scope.clone().unwrap_or_default(),
                    q.from.unwrap().to_rfc3339(),
                    q.to.unwrap().to_rfc3339(),
                ],
            )?;
            assert_eq!(q.basis, factory_kernel::SpendBasis::Started);
            assert_eq!(q.group_by, CostGroupBy::Scope);
            Ok(CostReport {
                basis: q.basis,
                finished: None,
                group_by: q.group_by,
                from: q.from.unwrap(),
                to: q.to.unwrap(),
                scope: q.scope.clone(),
                rows: Vec::new(),
                total: CostRow {
                    cost_usd: self.0.generation.load(Ordering::SeqCst) as f64,
                    ..Default::default()
                },
                unattributed_runs: 0,
                daily: Vec::new(),
            })
        }
    }
    #[async_trait]
    impl Provide<DaemonConfigFact> for Infrastructure {
        type Query = ();
        type Value = DaemonConfigFact;
        type Error = FactoryError;
        async fn get(&self, _: &()) -> Result<Self::Value> {
            self.0.record("daemon", Vec::new())?;
            Ok(DaemonConfigFact {
                foreman_enabled: self.0.generation.load(Ordering::SeqCst) > 0,
                ..Default::default()
            })
        }
    }
    #[async_trait]
    impl Provide<BackupFact> for Infrastructure {
        type Query = DateTime<Utc>;
        type Value = BackupFact;
        type Error = FactoryError;
        async fn get(&self, at: &DateTime<Utc>) -> Result<Self::Value> {
            self.0.record("backup", vec![at.to_rfc3339()])?;
            Ok(BackupFact {
                at: *at,
                configured: false,
                newest: None,
                recent: None,
                offsite: None,
                verified: None,
                last_verified: None,
            })
        }
    }
    #[async_trait]
    impl Provide<SecretsPresence> for Environment {
        type Query = BTreeSet<String>;
        type Value = BTreeMap<String, SecretsPresence>;
        type Error = FactoryError;
        async fn get(&self, q: &Self::Query) -> Result<Self::Value> {
            self.0.record("secrets", q.iter().cloned().collect())?;
            Ok(q.iter()
                .map(|scope| (scope.clone(), SecretsPresence::default()))
                .collect())
        }
    }
    #[async_trait]
    impl Provide<DependenciesFact> for Environment {
        type Query = String;
        type Value = DependenciesFact;
        type Error = FactoryError;
        async fn get(&self, q: &String) -> Result<Self::Value> {
            self.0.record("dependencies", vec![q.clone()])?;
            Ok(DependenciesFact::default())
        }
    }
    #[async_trait]
    impl Provide<AgentFact> for Agents {
        type Query = String;
        type Value = Vec<AgentFact>;
        type Error = FactoryError;
        async fn get(&self, q: &String) -> Result<Self::Value> {
            self.0.record("agents", vec![q.clone()])?;
            Ok(Vec::new())
        }
    }
    struct Inputs {
        infrastructure: Infrastructure,
        environment: Environment,
        agents: Agents,
        process: Process,
    }
    macro_rules! capability {
        ($name:ident, $method:ident, $field:ident, $owner:ty) => {
            type $name = $owner;
            fn $method(&self) -> &Self::$name {
                &self.$field
            }
        };
    }
    impl Ports for Inputs {
        capability!(Tasks, tasks, process, Process);
        capability!(Workflows, workflows, process, Process);
        capability!(Agents, agents, agents, Agents);
        capability!(Daemon, daemon, infrastructure, Infrastructure);
        capability!(Secrets, secrets, environment, Environment);
        capability!(Dependencies, dependencies, environment, Environment);
        capability!(Backup, backup, infrastructure, Infrastructure);
        capability!(Attested, attested, process, Process);
        capability!(Spend, spend, process, Process);
    }
    fn service<'a>(recorder: &Arc<Recorder>, bench: &'a BenchStore) -> Service<'a, Inputs> {
        Service::new(
            Inputs {
                infrastructure: Infrastructure(recorder.clone()),
                environment: Environment(recorder.clone()),
                agents: Agents(recorder.clone()),
                process: Process(recorder.clone()),
            },
            facts::Provider::new(
                bench,
                std::env::temp_dir().join(format!("factory-no-vault-{}", uuid::Uuid::new_v4())),
            ),
            ScopeTree {
                scopes: [
                    ("parent", "projects/work"),
                    ("child", "projects/work/deep"),
                    ("child/fake", "projects/work-other"),
                ]
                .into_iter()
                .map(|(name, path)| ScopeNode {
                    name: name.into(),
                    path: path.into(),
                })
                .collect(),
            },
        )
    }
    fn subject(evidence: Vec<Check>) -> EvaluationSubject {
        EvaluationSubject {
            control: "test/check".parse().unwrap(),
            title: "a check".into(),
            kind: (),
            maps_to: Vec::new(),
            evidence,
            max_age: None,
            not_applicable: None,
        }
    }
    fn time() -> DateTime<Utc> {
        "2026-10-16T12:00:00Z".parse().unwrap()
    }

    #[tokio::test]
    async fn empty_and_unneeded_evidence_never_probes_lower_owners() {
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        let applied = [subject(vec![Check::Knowledge { tag: None }])];
        let shared = service.shared(&[("child", &applied)]).await.unwrap();
        let evidence = service
            .for_scope(
                &service.scopes.scopes[1],
                &applied,
                &BTreeSet::new(),
                &[],
                &shared,
                None,
                time(),
            )
            .await
            .unwrap();
        assert_eq!(evidence, Evidence::default());
        assert_eq!(
            recorder.calls.lock().unwrap().as_slice(),
            &[
                ("tasks".into(), vec!["child".into()]),
                ("workflows".into(), vec!["child".into()])
            ]
        );
    }

    #[tokio::test]
    async fn shared_union_reads_once_and_scope_reads_are_exact_and_live() {
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        let checks = [subject(vec![
            Check::Task {
                task: "named".into(),
                max_age: None,
            },
            Check::Task {
                task: "named".into(),
                max_age: None,
            },
            Check::Workflow {
                workflow: "flow".into(),
                max_age: None,
            },
            Check::Sandbox,
            Check::Secrets {
                absent: vec!["scope_env".into()],
            },
            Check::Daemon {
                fact: "backup_recent".into(),
            },
            Check::Dependencies {
                sbom_max_age: None,
                built_sbom: false,
                max_open: BTreeMap::new(),
                exploited_open: None,
            },
        ])];
        let plain = [subject(vec![Check::Daemon {
            fact: "power_assertion".into(),
        }])];
        let shared = service
            .shared(&[
                ("parent", &checks),
                ("child", &checks),
                ("child/fake", &plain),
            ])
            .await
            .unwrap();
        assert_eq!(recorder.calls("daemon").len(), 1);
        assert_eq!(recorder.calls("backup").len(), 1);
        assert_eq!(
            recorder.calls("secrets"),
            vec![vec!["child".to_string(), "parent".to_string()]]
        );
        for generation in [1, 2] {
            recorder.generation.store(generation, Ordering::SeqCst);
            let evidence = service
                .for_scope(
                    &service.scopes.scopes[1],
                    &checks,
                    &BTreeSet::new(),
                    &[],
                    &shared,
                    None,
                    time(),
                )
                .await
                .unwrap();
            assert_eq!(evidence.tasks["named"][0].id, format!("child-{generation}"));
            assert_eq!(evidence.agents, Some(Vec::new()));
            assert!(evidence.dependencies.is_some());
            assert_eq!(evidence.backup, shared.backup);
        }
        assert_eq!(
            recorder.calls("tasks"),
            vec![vec!["child".to_string(), "named".to_string()]; 2]
        );
        assert_eq!(recorder.calls("agents"), vec![vec!["child".to_string()]; 2]);
        assert_eq!(
            recorder.calls("dependencies"),
            vec![vec!["child".to_string()]; 2]
        );
    }

    #[tokio::test]
    async fn attested_query_uses_exact_scope_categories_and_twice_widest_effective_age() {
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        let mut short = subject(vec![Check::Attested {
            category: "feature".into(),
            step: "tests".into(),
            max_age: factory_kernel::Duration::from_hours(24),
        }]);
        short.max_age = Some(factory_kernel::Duration::from_hours(24));
        let mut wide = subject(vec![Check::Attested {
            category: "release".into(),
            step: "build".into(),
            max_age: factory_kernel::Duration::from_hours(48),
        }]);
        wide.max_age = Some(factory_kernel::Duration::from_hours(48));
        assert_eq!(
            service.attested("child", &[short, wide]).await.unwrap(),
            Some(Vec::new())
        );
        let rows = recorder.calls("attested");
        assert_eq!(&rows[0][..3], &["child", "feature", "release"]);
        let from: DateTime<Utc> = rows[0][3].parse().unwrap();
        let to: DateTime<Utc> = rows[0][4].parse().unwrap();
        assert_eq!(to - from, chrono::Duration::hours(96));
    }

    #[tokio::test]
    async fn budget_reads_each_authored_cap_live_but_errors_and_month_start_do_not_read_spend() {
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        let mut intent = BudgetIntent {
            caps: vec![("parent".into(), 5.0), ("child".into(), 3.0)],
            error: None,
        };
        for generation in [1, 6] {
            recorder.generation.store(generation, Ordering::SeqCst);
            let evidence = service.budget(&intent, time()).await.unwrap();
            assert_eq!(
                evidence
                    .caps
                    .iter()
                    .map(|c| c.scope.as_str())
                    .collect::<Vec<_>>(),
                ["parent", "child"]
            );
            assert_eq!(evidence.caps[0].spend.total.cost_usd, generation as f64);
            assert_eq!(
                budget::within(Some(&evidence), time()).0,
                Some(generation == 1)
            );
        }
        assert_eq!(recorder.calls("spend").len(), 4);
        intent.error = Some("malformed authored catalogue".into());
        assert_eq!(
            service.budget(&intent, time()).await.unwrap().error,
            intent.error
        );
        intent.error = None;
        let start: DateTime<Utc> = "2026-10-01T00:00:00Z".parse().unwrap();
        let empty = service.budget(&intent, start).await.unwrap();
        assert_eq!(empty.caps[0].spend.total.cost_usd, 0.0);
        assert_eq!(budget::within(Some(&empty), start).0, Some(true));
        assert_eq!(recorder.calls("spend").len(), 4);
    }

    #[tokio::test]
    async fn attestation_filter_is_path_ancestry_not_name_prefix_and_preserves_order() {
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        let receipts: Vec<Attestation> = ["parent", "child/fake", "child"].into_iter().map(|scope| serde_json::from_value(serde_json::json!({
            "id": scope, "control": "test/check", "scope": scope, "evidence": "doc", "attested_by": "person",
            "attested_at": time(), "expires_at": time() + chrono::Duration::days(1),
        })).unwrap()).collect();
        let evidence = service
            .for_scope(
                &service.scopes.scopes[1],
                &[subject(vec![Check::Attestation])],
                &BTreeSet::new(),
                &receipts,
                &Shared::default(),
                None,
                time(),
            )
            .await
            .unwrap();
        assert_eq!(
            evidence
                .attestations
                .iter()
                .map(|a| a.scope.as_str())
                .collect::<Vec<_>>(),
            ["parent", "child"]
        );
    }

    #[tokio::test]
    async fn a_needed_provider_failure_is_not_swallowed_or_cached() {
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        *recorder.failing.lock().unwrap() = Some("daemon");
        let applied = [subject(vec![Check::Daemon {
            fact: "power_assertion".into(),
        }])];
        assert!(
            matches!(service.shared(&[("child", &applied)]).await, Err(FactoryError::BadRequest(reason)) if reason == "daemon unavailable")
        );
        *recorder.failing.lock().unwrap() = None;
        assert!(service
            .shared(&[("child", &applied)])
            .await
            .unwrap()
            .daemon
            .is_some());
        assert_eq!(recorder.calls("daemon").len(), 2);
        assert!(
            recorder.calls("backup").is_empty(),
            "plain daemon checks never probe backup storage"
        );
    }

    #[tokio::test]
    async fn quality_judgement_gathers_live_l5_evidence_and_keeps_report_order() {
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        let catalogue = quality::QualityCatalogue {
            profiles: [("profile".into(), serde_yaml_ng::from_str("attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n      - { id: foreman, measure: { check: daemon, fact: foreman_enabled } }\n").unwrap())].into_iter().collect(), findings: Vec::new(),
        };
        let trees: Vec<_> = ["child", "parent"]
            .into_iter()
            .map(|scope| {
                quality::applicable(
                    &catalogue,
                    scope,
                    &[quality::QualityLayer {
                        scope: scope.into(),
                        profiles: vec!["profile".into()],
                    }],
                )
                .0
            })
            .collect();
        let scopes: Vec<_> = trees
            .iter()
            .map(|tree| QualityScope {
                scope: service.scopes.scope(&tree.scope).unwrap().clone(),
                tree,
                budget: None,
            })
            .collect();
        for (generation, expected) in [
            (0, quality::ScenarioStatus::NotMet),
            (1, quality::ScenarioStatus::Met),
        ] {
            recorder.generation.store(generation, Ordering::SeqCst);
            let (reports, findings) = service
                .judge_quality(&scopes, &BTreeMap::new(), time())
                .await
                .unwrap();
            assert!(findings.is_empty());
            assert_eq!(
                reports.iter().map(|r| r.scope.as_str()).collect::<Vec<_>>(),
                ["child", "parent"]
            );
            assert!(reports
                .iter()
                .all(|r| r.attributes[0].scenarios[0].status == expected));
        }
        assert_eq!(
            recorder.calls("daemon").len(),
            2,
            "one shared read per judgement, not per scope"
        );
        for unneeded in [
            "secrets",
            "backup",
            "dependencies",
            "spend",
            "attested",
            "agents",
        ] {
            assert!(recorder.calls(unneeded).is_empty(), "unneeded {unneeded}");
        }
    }

    #[tokio::test]
    async fn shared_gate_read_observes_actual_own_store_changes_on_the_next_read() {
        use crate::bench::{BenchAttempt, BenchRun, BenchRunStatus, Verdict};
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let service = service(&recorder, &bench);
        let applied = [subject(vec![Check::Gate {
            dataset: "demo".into(),
            case: None,
            max_age: None,
        }])];
        let targets = [
            ("parent", applied.as_slice()),
            ("child", applied.as_slice()),
        ];
        assert!(service.shared(&targets).await.unwrap().gates.is_empty());
        let run = BenchRun {
            id: "bench-run".into(),
            dataset: "demo".into(),
            dataset_revision: 1,
            cases: serde_json::from_value(
                serde_json::json!([{"id":"gated", "title":"gate", "scope":"child", "gate":"true"}]),
            )
            .unwrap(),
            case_bases: BTreeMap::new(),
            agents: vec!["builder".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts: Vec::new(),
            started_at: time(),
            ended_at: None,
        };
        bench.put_run(&run).await.unwrap();
        let mut attempt =
            BenchAttempt::pending("attempt".into(), "gated".into(), "builder".into(), 1);
        attempt.verdict = Some(Verdict::Pass);
        bench.put_attempt(&run.id, &attempt).await.unwrap();
        let passed = service.shared(&targets).await.unwrap();
        assert_eq!(passed.gates["demo"].cases[0].verdicts, [Verdict::Pass]);
        attempt.verdict = Some(Verdict::Fail);
        bench.put_attempt(&run.id, &attempt).await.unwrap();
        assert_eq!(
            service.shared(&targets).await.unwrap().gates["demo"].cases[0].verdicts,
            [Verdict::Fail]
        );
        assert!(
            recorder.calls.lock().unwrap().is_empty(),
            "gate-only subjects never probe lower owners"
        );
    }

    #[tokio::test]
    async fn quality_knowledge_check_observes_real_vault_file_changes_without_events() {
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let root = Scratch(
            std::env::temp_dir().join(format!("factory-quality-evidence-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir_all(root.0.join(".factory/knowledge")).unwrap();
        let page = root.0.join(".factory/knowledge/page.md");
        let recorder = Arc::new(Recorder::default());
        let bench = BenchStore::in_memory().unwrap();
        let mut service = service(&recorder, &bench);
        service.own = facts::Provider::new(&bench, root.0.clone());
        let catalogue = quality::QualityCatalogue {
            profiles: [("profile".into(), serde_yaml_ng::from_str("attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n      - { id: documented, measure: { check: knowledge, tag: assurance/demo } }\n").unwrap())].into_iter().collect(), findings: Vec::new(),
        };
        let tree = quality::applicable(
            &catalogue,
            "child",
            &[quality::QualityLayer {
                scope: "child".into(),
                profiles: vec!["profile".into()],
            }],
        )
        .0;
        let scopes = [QualityScope {
            scope: service.scopes.scopes[1].clone(),
            tree: &tree,
            budget: None,
        }];
        for (tags, expected) in [
            ("other", quality::ScenarioStatus::NotMet),
            ("assurance/demo", quality::ScenarioStatus::Met),
            ("other", quality::ScenarioStatus::NotMet),
        ] {
            std::fs::write(&page, format!("---\ntags: [{tags}]\n---\n# Evidence\n")).unwrap();
            let (reports, findings) = service
                .judge_quality(&scopes, &BTreeMap::new(), time())
                .await
                .unwrap();
            assert!(findings.is_empty());
            assert_eq!(reports[0].attributes[0].scenarios[0].status, expected);
        }
        assert_eq!(
            recorder.calls.lock().unwrap().len(),
            6,
            "only empty task/workflow owner fast paths were called"
        );
    }
}
