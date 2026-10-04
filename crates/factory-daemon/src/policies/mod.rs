//! Where policy requests are served. Like `datasets.rs`: the authored
//! catalogues under `<root>/.factory/policies/` are the source of truth for
//! *what* a control is (`factory_core::policy`, pure and tested on its
//! own), re-read on every request; this module only assembles the evidence
//! that already lives elsewhere in the daemon -- the knowledge index, the
//! attestations store, tasks, workflows, bench runs, the live config
//! snapshot and the L2 Secrets tab's own credential inventory -- and folds
//! it against them.
//!
//! The one piece of state this module owns is the attestations themselves,
//! kept in `PolicyStore` (`store.rs`), append-only.

mod clock;
mod store;
pub use store::PolicyStore;

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use factory_core::config::{Factory, Scope};
use factory_core::error::{FactoryError, Result};
use factory_core::policy::{self, Attestation, ControlRef, Withdrawal};
use factory_core::policy_export;
use factory_core::protocol::{CatalogueSummary, NotApplicableEntry, PolicyControlDetail, PolicyReport, ScopePolicy, WorkflowEnforcement, WorkflowEnforcementFinding};
use factory_core::reporting_clock::{self, ClockDeadlineState, ClockMark};
use factory_core::task::{NewTask, Task, TaskFilter};
use factory_kernel::{DaemonConfigFact, BackupFact, SecretsPresence, AgentFact, TaskFact, WorkflowFact, GateFact, DependenciesFact, AttestedRun, L6};
use crate::facts::{Facts, NamedQuery, AttestedQuery};

use crate::access::Caller;
use crate::engine::Engine;

/// Every distinct dataset name a `gate` check among `applied`'s controls
/// names -- what a caller resolving `gate` facts (`Engine::gate_facts_for`)
/// has to ask about, and no more.
fn gate_dataset_names(applied: &[policy::Applied]) -> BTreeSet<String> {
    applied
        .iter()
        .flat_map(|a| &a.evidence)
        .filter_map(|check| match check {
            policy::Check::Gate { dataset, .. } => Some(dataset.clone()),
            _ => None,
        })
        .collect()
}

/// Whether any check among `applied` is `roles` or `sandbox` -- both read
/// `Evidence::agents`, so a scope whose catalogue never asks either
/// question never pays for `Engine::agent_facts_for`'s `roles_for` lookup.
fn needs_agent_facts(applied: &[policy::Applied]) -> bool {
    applied
        .iter()
        .flat_map(|a| &a.evidence)
        .any(|check| matches!(check, policy::Check::Roles { .. } | policy::Check::Sandbox))
}

/// Whether any check among `applied` is `secrets` -- gates
/// `Engine::credential_inventory`, the one part of this module's evidence
/// gathering that touches the filesystem, behind an actual need for it.
fn needs_secrets_facts(applied: &[policy::Applied]) -> bool {
    applied
        .iter()
        .flat_map(|a| &a.evidence)
        .any(|check| matches!(check, policy::Check::Secrets { .. }))
}

/// Whether any check among `applied` is `daemon`.
fn needs_daemon_facts(applied: &[policy::Applied]) -> bool {
    applied
        .iter()
        .flat_map(|a| &a.evidence)
        .any(|check| matches!(check, policy::Check::Daemon { .. }))
}

/// Whether any check among `applied` is `daemon` naming one of the three
/// `backup_*` facts (`#154`) -- gates `Engine::backup_fact`, the one part of
/// this module's evidence gathering that reads the destination and the
/// backup store, behind an actual need for it. Distinct from
/// `needs_daemon_facts`: a catalogue asking only `power_assertion` must
/// never pay for it.
fn needs_backup_facts(applied: &[policy::Applied]) -> bool {
    applied.iter().flat_map(|a| &a.evidence).any(|check| {
        matches!(
            check,
            policy::Check::Daemon { fact } if matches!(fact.as_str(), "backup_recent" | "backup_offsite" | "backup_verified")
        )
    })
}

fn needs_dependencies_facts(applied: &[policy::Applied]) -> bool {
    applied.iter().flat_map(|a| &a.evidence)
        .any(|check| matches!(check, policy::Check::Dependencies { .. }))
}

/// Whether any check among `applied` is `attested` -- gates
/// `Engine::attested_runs`, the one part of this module's evidence
/// gathering that reads finished runs and their `StepAttestation`s (`#158`).
fn needs_attested_facts(applied: &[policy::Applied]) -> bool {
    applied
        .iter()
        .flat_map(|a| &a.evidence)
        .any(|check| matches!(check, policy::Check::Attested { .. }))
}

fn needs_budget_facts(applied: &[policy::Applied]) -> bool {
    applied.iter().flat_map(|a| &a.evidence).any(|check| matches!(check, policy::Check::BudgetWithin))
}

/// Every category an `attested` check among `applied`'s controls names, and
/// the widest of those controls' own effective `max_age`
/// (`Applied::max_age`, already folded and tightened) -- what
/// `Engine::attested_runs`'s window covers, `now - 2*that`, so
/// `direct_status`'s own `2*W` stale lookback never reads short of what was
/// fetched, however many `attested` checks the scope's controls carry, each
/// with its own window. `Applied::max_age` is always `Some` on a control
/// carrying an `attested` check -- `Check::own_max_age` always returns one
/// for it -- so this never has to fall back to a default.
fn attested_categories(
    applied: &[policy::Applied],
) -> (BTreeSet<String>, Option<factory_core::policy::Duration>) {
    let mut categories = BTreeSet::new();
    let mut widest: Option<factory_core::policy::Duration> = None;
    for a in applied {
        if !a
            .evidence
            .iter()
            .any(|c| matches!(c, policy::Check::Attested { .. }))
        {
            continue;
        }
        for check in &a.evidence {
            if let policy::Check::Attested { category, .. } = check {
                categories.insert(category.clone());
            }
        }
        if let Some(w) = a.max_age {
            widest = Some(widest.map_or(w, |cur| cur.max(w)));
        }
    }
    (categories, widest)
}

pub(crate) use factory_core::config::subtree_scopes;

impl Engine {
    /// Own catalogue read plus the L5 knowledge-tag port. Both filesystem
    /// walks stay off the async executor; providers own their fact reads.
    pub(crate) async fn load_catalogues_and_tags(
        &self,
    ) -> Result<(Vec<policy::Catalogue>, Vec<policy::Finding>, BTreeSet<String>)> {
        let snapshot = self.factory_snapshot();
        let policies_dir = snapshot.policies_dir();
        let (catalogues, findings) = tokio::task::spawn_blocking(move || {
            let (catalogues, findings) = policy::load_all(&policies_dir);
            (catalogues, findings)
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("policy catalogue walk: {e}")))?;
        let tags = Facts::<L6>::new(self).get::<factory_kernel::KnowledgeTags>(&()).await?.tags;
        Ok((catalogues, findings, tags))
    }

    /// Resolve every `task` and `workflow` check name `applied` actually
    /// references into `Evidence`'s fact maps -- `factory_core::policy`
    /// stays pure (`#81`), so this is where a check's name becomes a real
    /// task or workflow run. Both are resolved against `scope` alone, per
    /// the issue's own rule ("names a task in the evaluated scope"). Never
    /// looks beyond the names `applied`'s own checks mention -- not every
    /// task or workflow the scope has. `gate` facts are a separate call
    /// (`gate_facts_for`): datasets are instance-wide, so a caller looping
    /// over scopes resolves them once, not once per scope.
    async fn resolve_task_and_workflow_facts(
        &self,
        scope: &str,
        applied: &[policy::Applied],
    ) -> Result<(BTreeMap<String, Vec<policy::TaskFact>>, BTreeMap<String, Vec<policy::WorkflowFact>>)> {
        let mut task_names: BTreeSet<&str> = BTreeSet::new();
        let mut workflow_names: BTreeSet<&str> = BTreeSet::new();
        for a in applied {
            for check in &a.evidence {
                match check {
                    policy::Check::Task { task, .. } => {
                        task_names.insert(task.as_str());
                    }
                    policy::Check::Workflow { workflow, .. } => {
                        workflow_names.insert(workflow.as_str());
                    }
                    _ => {}
                }
            }
        }

        let facts = Facts::<L6>::new(self);
        let tasks = facts.get::<TaskFact>(&NamedQuery {
            scope: scope.to_string(), names: task_names.into_iter().map(str::to_string).collect(),
        }).await?;
        let workflows = facts.get::<WorkflowFact>(&NamedQuery {
            scope: scope.to_string(), names: workflow_names.into_iter().map(str::to_string).collect(),
        }).await?;
        Ok((tasks, workflows))
    }

    /// The `gate` fact for every dataset name in `names` that has ever had a
    /// settled bench run -- one lookup per name, however many scopes end up
    /// sharing the result. Datasets have no scope of their own, so a caller
    /// evaluating several scopes in one report calls this once, over the
    /// union of every scope's `gate` checks, rather than once per scope.
    async fn gate_facts_for(&self, names: &BTreeSet<String>) -> Result<BTreeMap<String, policy::GateFact>> {
        Facts::<L6>::new(self).get::<GateFact>(names).await
    }

    /// The facts every scope in a report's subtree shares, resolved once
    /// over the whole subtree rather than once per scope: every dataset a
    /// `gate` check anywhere in `per_scope_applied` names (datasets have no
    /// scope of their own, so a dataset named by two scopes' catalogues is
    /// still only walked once), the `daemon` fact (the same for every scope,
    /// resolved only when some scope's catalogue actually asks a `daemon`
    /// question), the `backup_*` fact (`#154`, gated the same way but on its
    /// own three names -- `needs_backup_facts` -- since it reads the
    /// destination and the backup store, not just the config snapshot
    /// `daemon_facts` does), and the credential inventory behind `secrets`
    /// (the one part of this that touches the filesystem, gated the same
    /// way). Shared by `policy_report` and `scenarios::Engine::scenarios_report`
    /// (`#100`), which calls this once for the baseline `Applied` sets and
    /// again for each scenario's own -- a scenario's overlay can name a
    /// `gate`/`daemon`/`secrets` check the baseline never did (an
    /// `add_frameworks` draft, say), and this is what picks up the extra
    /// fact lazily rather than the caller having to know in advance.
    pub(crate) async fn dataset_level_facts(
        &self,
        per_scope_applied: &[(&Scope, Vec<policy::Applied>)],
    ) -> Result<(
        BTreeMap<String, policy::GateFact>,
        Option<policy::DaemonFact>,
        BTreeMap<String, SecretsPresence>,
        Option<factory_core::backup::BackupFact>,
        Option<factory_core::budget::PolicyConfig>,
    )> {
        let mut dataset_names: BTreeSet<String> = BTreeSet::new();
        for (_, applied) in per_scope_applied {
            dataset_names.extend(gate_dataset_names(applied));
        }
        let gates = self.gate_facts_for(&dataset_names).await?;
        let facts = Facts::<L6>::new(self);
        let daemon_fact = if per_scope_applied.iter().any(|(_, applied)| needs_daemon_facts(applied)) {
            Some(facts.get::<DaemonConfigFact>(&()).await?)
        } else { None };
        let credential_rows = if per_scope_applied.iter().any(|(_, applied)| needs_secrets_facts(applied)) {
            let scopes = per_scope_applied.iter().filter(|(_, a)| needs_secrets_facts(a))
                .map(|(s, _)| s.name.clone()).collect();
            facts.get::<SecretsPresence>(&scopes).await?
        } else {
            BTreeMap::new()
        };
        let backup_fact = if per_scope_applied.iter().any(|(_, applied)| needs_backup_facts(applied)) {
            Some(facts.get::<BackupFact>(&Utc::now()).await?)
        } else {
            None
        };
        let budget_config = if per_scope_applied.iter().any(|(_, applied)| needs_budget_facts(applied)) {
            let root = self.factory_snapshot().root.clone();
            let loaded = tokio::task::spawn_blocking(move || factory_core::budget::load(&root)).await
                .map_err(|error| FactoryError::Other(anyhow::anyhow!("budget intent read: {error}")))?;
            Some(match loaded {
                Ok(catalogue) => factory_core::budget::PolicyConfig { catalogue: Some(catalogue), error: None },
                Err(error) => factory_core::budget::PolicyConfig { catalogue: None, error: Some(error) },
            })
        } else { None };
        Ok((gates, daemon_fact, credential_rows, backup_fact, budget_config))
    }

    /// `#158`: `scope`'s own `attested` evidence, gathered only when
    /// `applied` actually names the check -- `None` when it does not,
    /// distinct from the empty `Vec` a scope with nothing attested yet
    /// would carry. Exact scope, no ancestor roll-up -- the same rule
    /// `task`/`workflow` checks already follow -- over
    /// `now - 2*widest(applied)`, so `direct_status`'s own stale lookback
    /// (`2*W`) always has enough history behind it, whichever `attested`
    /// check on this scope needs the longest window.
    async fn attested_evidence(
        &self,
        scope: &str,
        applied: &[policy::Applied],
    ) -> Result<Option<Vec<factory_core::conformance::AttestedRun>>> {
        if !needs_attested_facts(applied) {
            return Ok(None);
        }
        let (categories, widest) = attested_categories(applied);
        let w = widest
            .unwrap_or(factory_core::policy::Duration::from_hours(0))
            .as_time_delta();
        let now = Utc::now();
        let window = factory_core::operations::Window {
            from: now - (w + w),
            to: now,
        };
        let scopes: BTreeSet<String> = std::iter::once(scope.to_string()).collect();
        Ok(Some(
            Facts::<L6>::new(self).get::<AttestedRun>(&AttestedQuery {
                scopes: Some(scopes), categories: Some(categories), window,
            })
                .await?,
        ))
    }

    /// One scope's own `Evidence`, built from `applied` (its own applicable
    /// controls) plus the subtree-wide facts `dataset_level_facts` already
    /// resolved -- task/workflow resolution and the `agents`/`secrets` facts
    /// stay per scope, since a scope's own roster and its own `.env` are its
    /// own, gathered only when `applied`'s own checks actually ask for them.
    /// The body of `policy_report`'s former second pass, unchanged, so its
    /// own tests (and `scenarios::Engine::scenarios_report`'s, `#100`) see
    /// exactly the evidence a real request would.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn evidence_for_scope(
        &self,
        snapshot: &Factory,
        t: &Scope,
        applied: &[policy::Applied],
        tags: &BTreeSet<String>,
        all_attestations: &[Attestation],
        gates: &BTreeMap<String, policy::GateFact>,
        daemon_fact: Option<policy::DaemonFact>,
        credential_rows: &BTreeMap<String, SecretsPresence>,
        backup_fact: Option<factory_core::backup::BackupFact>,
        budget_config: Option<&factory_core::budget::PolicyConfig>,
        now: chrono::DateTime<Utc>,
    ) -> Result<policy::Evidence> {
        let ancestor_names: BTreeSet<&str> = snapshot
            .config
            .ancestors_of(t)
            .iter()
            .map(|ancestor| ancestor.name.as_str())
            .collect();
        let (tasks, workflows) = self.resolve_task_and_workflow_facts(&t.name, applied).await?;
        let facts = Facts::<L6>::new(self);
        let agents = if needs_agent_facts(applied) {
            Some(facts.get::<AgentFact>(&t.name).await?)
        } else { None };
        let secrets = if needs_secrets_facts(applied) {
            credential_rows.get(&t.name).cloned().unwrap_or_default()
        } else {
            SecretsPresence::default()
        };
        let dependencies = if needs_dependencies_facts(applied) {
            Some(facts.get::<DependenciesFact>(&t.name).await?)
        } else {
            None
        };
        let attested = self.attested_evidence(&t.name, applied).await?;
        let budget = if needs_budget_facts(applied) {
            match budget_config {
                Some(config) => Some(self.budget_policy_input(snapshot, t, config, now).await?),
                None => None,
            }
        } else { None };
        Ok(policy::Evidence {
            tags: tags.clone(),
            attestations: all_attestations
                .iter()
                .filter(|att| att.scope == t.name || ancestor_names.contains(att.scope.as_str()))
                .cloned()
                .collect(),
            tasks,
            workflows,
            gates: gates.clone(),
            agents,
            secrets,
            daemon: daemon_fact,
            dependencies,
            backup: backup_fact,
            attested,
            budget,
        })
    }

    async fn policy_workflow_enforcement(
        &self,
        target_scopes: &[factory_core::config::Scope],
    ) -> Result<(Vec<WorkflowEnforcement>, Vec<WorkflowEnforcementFinding>)> {
        let target_names: BTreeSet<_> = target_scopes.iter().map(|scope| scope.name.as_str()).collect();
        let mut enforcement = Vec::new();
        let mut findings = Vec::new();
        for definition in self.workflows.definitions(None).await? {
            if !target_names.contains(definition.scope.as_str()) {
                continue;
            }
            let lint = match self.workflow_lint(Some(definition.id.clone()), None, None, None).await {
                Ok(lint) => lint,
                Err(error) => {
                    findings.push(WorkflowEnforcementFinding {
                        workflow: definition.id.clone(),
                        name: definition.name.clone(),
                        scope: definition.scope.clone(),
                        detail: error.to_string(),
                    });
                    continue;
                }
            };
            if let Some(injected) = lint.injected.as_ref() {
                for injection in &lint.injections {
                    let Some(control) = injected.nodes.iter().find(|node| node.id == injection.gate_node_id) else { continue };
                    let Some(spec) = control.gate.as_ref() else { continue };
                    let kind = match control.kind {
                        factory_core::workflow::WorkflowNodeKind::Gate => factory_core::control_plan::StepKind::Gate,
                        factory_core::workflow::WorkflowNodeKind::Review => factory_core::control_plan::StepKind::Review,
                        factory_core::workflow::WorkflowNodeKind::Approval => factory_core::control_plan::StepKind::Approval,
                        factory_core::workflow::WorkflowNodeKind::Task | factory_core::workflow::WorkflowNodeKind::Expand => continue,
                    };
                    enforcement.push(WorkflowEnforcement {
                        workflow: definition.id.clone(),
                        name: definition.name.clone(),
                        scope: definition.scope.clone(),
                        node: injection.node_id.clone(),
                        step: injection.step.clone(),
                        kind,
                        required_by: injection.required_by.clone(),
                        actor: spec.actor.clone(),
                    });
                }
            }
            findings.extend(lint.violations.into_iter().map(|detail| WorkflowEnforcementFinding {
                workflow: definition.id.clone(),
                name: definition.name.clone(),
                scope: definition.scope.clone(),
                detail,
            }));
        }
        Ok((enforcement, findings))
    }

    /// The L6 Policy tab: `Request::Policy`.
    pub(crate) async fn policy_report(&self, scope: Option<&str>) -> Result<PolicyReport> {
        let snapshot = self.factory_snapshot();
        let (catalogues, mut findings, tags) = self.load_catalogues_and_tags().await?;
        let (asked, target_scopes) = subtree_scopes(&snapshot, scope)?;

        let all_attestations = self.policies.all().await?;
        let now = Utc::now();

        let mut rows = Vec::new();
        let mut per_scope_statuses: Vec<Vec<policy::ControlStatus>> = Vec::new();
        let mut not_applicable: BTreeSet<(ControlRef, String, String)> = BTreeSet::new();

        // First pass: resolve applicability for every scope. A scope whose
        // whole chain applies no framework has nothing to show -- omitted
        // rather than an empty row nobody asked to see.
        let mut per_scope_applied: Vec<(&factory_core::config::Scope, Vec<policy::Applied>)> = Vec::new();
        for t in &target_scopes {
            // `Engine::policy_chain` (#76) is the one place a scope name
            // becomes the chain `applicable` folds -- root first, through
            // every ancestor, to `t` itself.
            let chain = self.policy_chain(&t.name);
            let (applied, chain_findings) = policy::applicable(&catalogues, &chain);
            findings.extend(chain_findings);

            for a in &applied {
                if let Some(na) = &a.not_applicable {
                    not_applicable.insert((a.control.clone(), na.scope.clone(), na.rationale.clone()));
                }
            }

            if applied.is_empty() {
                continue;
            }
            per_scope_applied.push((t, applied));
        }

        // Second pass: `dataset_level_facts` resolves what every scope
        // shares (gate datasets, the daemon fact, the credential inventory)
        // in one pass over the whole subtree, and `evidence_for_scope`
        // builds each scope's own `Evidence` from those plus its own
        // per-scope facts (tasks, workflows, agents, secrets) -- the same
        // two calls `scenarios::Engine::scenarios_report` (`#100`) makes
        // against its own, larger `applied` sets, so a policy fact is
        // gathered by exactly one function regardless of which report is
        // asking for it.
        let (gates, daemon_fact, credential_rows, backup_fact, budget_config) = self.dataset_level_facts(&per_scope_applied).await?;
        for (t, applied) in &per_scope_applied {
            let evidence = self
                .evidence_for_scope(
                    &snapshot,
                    t,
                    applied,
                    &tags,
                    &all_attestations,
                    &gates,
                    daemon_fact,
                    &credential_rows,
                    backup_fact.clone(),
                    budget_config.as_ref(),
                    now,
                )
                .await?;
            findings.extend(policy::evidence_findings(&evidence, &t.name));

            let statuses = policy::evaluate(applied, &evidence, now);
            let scope_rollup = policy::rollup(&statuses);
            per_scope_statuses.push(statuses.clone());
            rows.push(ScopePolicy {
                scope: t.name.clone(),
                statuses,
                rollup: scope_rollup,
                open_tasks: BTreeMap::new(),
            });
        }

        // One unscoped read for every open remediation task, rather than one
        // per row -- the same read `quality_report` makes. A `policy=` label
        // names no scope (unlike `quality=`), so the task's own scope is half
        // the key; the store already filters on exactly that column when
        // `policy_remediate` asks it for one scope, so the two agree.
        if !rows.is_empty() {
            let mut open: BTreeMap<(String, String), String> = BTreeMap::new();
            for task in self.store.list(&TaskFilter::default()).await? {
                if let Some(label) = open_policy_label(&task) {
                    // Newest first: keep the one `policy_remediate`'s own
                    // `find` would name, should two ever carry one label.
                    open.entry((task.scope.clone(), label.to_string())).or_insert_with(|| task.id.clone());
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

        let subtree_rollup = policy::rollup(&policy::worst_across_scopes(&per_scope_statuses));

        findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));
        findings.dedup();

        let catalogues_summary = catalogues
            .iter()
            .map(|c| CatalogueSummary {
                framework: c.framework.clone(),
                title: c.title.clone(),
                kind: c.kind,
                controls: c.controls.len(),
            })
            .collect();

        // Show what policy `requires:` entries do to stored workflows. Box
        // this sizeable lint pass so `handle_request`'s future stays small --
        // every request shares that future's stack layout.
        let (workflow_enforcement, workflow_findings) =
            Box::pin(self.policy_workflow_enforcement(&target_scopes)).await?;

        Ok(PolicyReport {
            // The canonical name, like `RoleBoard.scope` -- a caller that
            // asked by a legacy bare name still gets back exactly what
            // `rows[].scope` uses, not the spelling it happened to type.
            scope: asked.as_ref().map(|s| s.name.clone()),
            rows,
            rollup: subtree_rollup,
            not_applicable: not_applicable
                .into_iter()
                .map(|(control, scope, rationale)| NotApplicableEntry { control, scope, rationale })
                .collect(),
            findings,
            workflow_enforcement,
            workflow_findings,
            catalogues: catalogues_summary,
        })
    }

    /// One control's full detail at `scope`: `Request::PolicyControl`.
    pub(crate) async fn policy_control(&self, control: ControlRef, scope: &str) -> Result<PolicyControlDetail> {
        let snapshot = self.factory_snapshot();
        let scope_obj = snapshot.scope(scope)?.clone();
        let (catalogues, _findings, tags) = self.load_catalogues_and_tags().await?;

        let chain = self.policy_chain(&scope_obj.name);
        let (applied, _findings) = policy::applicable(&catalogues, &chain);
        let found = applied.iter().find(|a| a.control == control).ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "{control} does not apply at {:?}, or is not a control any loaded catalogue defines",
                scope_obj.name
            ))
        })?;

        let ancestor_names: BTreeSet<&str> = snapshot
            .config
            .ancestors_of(&scope_obj)
            .iter()
            .map(|ancestor| ancestor.name.as_str())
            .collect();
        let mut history: Vec<Attestation> = self
            .policies
            .all()
            .await?
            .into_iter()
            .filter(|att| att.control == control && (att.scope == scope_obj.name || ancestor_names.contains(att.scope.as_str())))
            .collect();
        history.sort_by_key(|a| std::cmp::Reverse(a.attested_at));

        // The full `applied` set, not just `found`, the same as `policy_report`:
        // a `maps_to` neighbour's own `task`/`workflow`/`gate` check can
        // still decide this control's status, so its name has to resolve
        // too, or propagation would see it as wrongly `open`.
        let (tasks, workflows) = self.resolve_task_and_workflow_facts(&scope_obj.name, &applied).await?;
        let gates = self.gate_facts_for(&gate_dataset_names(&applied)).await?;
        let facts = Facts::<L6>::new(self);
        let agents = if needs_agent_facts(&applied) {
            Some(facts.get::<AgentFact>(&scope_obj.name).await?)
        } else { None };
        let secrets = if needs_secrets_facts(&applied) {
            facts.get::<SecretsPresence>(&std::iter::once(scope_obj.name.clone()).collect()).await?
                .remove(&scope_obj.name).unwrap_or_default()
        } else {
            SecretsPresence::default()
        };
        let daemon = if needs_daemon_facts(&applied) {
            Some(facts.get::<DaemonConfigFact>(&()).await?)
        } else { None };
        let backup = if needs_backup_facts(&applied) {
            Some(facts.get::<BackupFact>(&Utc::now()).await?)
        } else {
            None
        };
        let dependencies = if needs_dependencies_facts(&applied) {
            Some(facts.get::<DependenciesFact>(&scope_obj.name).await?)
        } else {
            None
        };
        let attested = self.attested_evidence(&scope_obj.name, &applied).await?;
        let now = Utc::now();
        let budget = if needs_budget_facts(&applied) {
            let (_, _, _, _, config) = self.dataset_level_facts(&[(&scope_obj, applied.clone())]).await?;
            match config {
                Some(config) => Some(self.budget_policy_input(&snapshot, &scope_obj, &config, now).await?),
                None => None,
            }
        } else { None };
        let evidence = policy::Evidence {
            tags,
            attestations: history.clone(),
            tasks,
            workflows,
            gates,
            agents,
            secrets,
            daemon,
            dependencies,
            backup,
            attested,
            budget,
        };
        let evaluated = policy::evaluate(&applied, &evidence, now)
            .into_iter()
            .find(|s| s.control == control)
            .ok_or_else(|| FactoryError::Other(anyhow::anyhow!("{control} evaluated to no status")))?;
        let open_task = self.open_policy_task(&control, &scope_obj.name).await?.map(|t| t.id);

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

    /// Record an attestation: `Request::PolicyAttest`. Refuses empty
    /// evidence, a past or missing expiry, an unknown control, and a
    /// control that does not apply -- or is `n/a` -- at `scope`. With
    /// `clock` or `corrective` set (CRA Art. 14 reporting evidence), also
    /// refuses any control but `cra/art-14`, an
    /// unknown clock item, one whose own scope is not exactly `scope` (a
    /// subtree read would otherwise let a root-scope attestation cover a
    /// child's item), an excluded item, and a deadline that already has a
    /// live (unwithdrawn) submission. Corrective measures additionally refuse
    /// future availability and duplicate live anchors; a final submission
    /// requires an evidenced anchor. Expiry never erases either clock record.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn policy_attest(
        &self,
        caller: &Caller,
        control: ControlRef,
        scope: String,
        evidence: String,
        note: Option<String>,
        expires_at: chrono::DateTime<Utc>,
        clock: Option<ClockMark>,
        corrective: Option<reporting_clock::CorrectiveMeasureMark>,
    ) -> Result<Attestation> {
        if evidence.trim().is_empty() {
            return Err(FactoryError::BadRequest("evidence must not be empty".into()));
        }
        let now = Utc::now();
        if expires_at <= now {
            return Err(FactoryError::BadRequest(format!(
                "expires_at {expires_at} must be in the future"
            )));
        }
        if clock.is_some() && corrective.is_some() {
            return Err(FactoryError::BadRequest("record a submission or a corrective measure, not both".into()));
        }
        if corrective.as_ref().is_some_and(|m| m.available_at > now) {
            return Err(FactoryError::BadRequest("available_at must not be in the future".into()));
        }
        if clock.is_some() || corrective.is_some() {
            let art_14 = reporting_clock::art_14();
            if control != art_14 {
                return Err(FactoryError::BadRequest(format!(
                    "a reporting-clock submission may only be recorded against {art_14}, not {control}"
                )));
            }
        }

        let snapshot = self.factory_snapshot();
        // Canonicalizes the name the same way every other scoped write does,
        // and refuses one that names no scope at all.
        let scope = snapshot.scope(&scope)?.name.clone();
        let (catalogues, _findings, _tags) = self.load_catalogues_and_tags().await?;
        let chain = self.policy_chain(&scope);
        let (applied, _findings) = policy::applicable(&catalogues, &chain);
        let found = applied.iter().find(|a| a.control == control).ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "{control} does not apply at {scope:?}, or is not a control any loaded catalogue defines"
            ))
        })?;
        if let Some(na) = &found.not_applicable {
            return Err(FactoryError::BadRequest(format!(
                "{control} is marked not applicable at {:?}: {}",
                na.scope, na.rationale
            )));
        }

        if let Some(mark_item) = clock.as_ref().map(|m| &m.item).or_else(|| corrective.as_ref().map(|m| &m.item)) {
            // The item's own scope must *equal* the canonical `scope` --
            // `policy_clock(Some(&scope))` rolls up the subtree the same way
            // `Request::Policy` does, so a descendant's item can appear in
            // it too; only an exact match may be attested here.
            let clock_now = self.policy_clock(Some(&scope)).await?;
            let item = clock_now.items.iter().find(|i| &i.item == mark_item).ok_or_else(|| {
                FactoryError::BadRequest(format!(
                    "{} is not a reporting-clock item in {scope:?}'s subtree",
                    mark_item
                ))
            })?;
            if item.scope != scope {
                return Err(FactoryError::BadRequest(format!(
                    "{} belongs to scope {:?}, not {scope:?} -- attest it there",
                    mark_item, item.scope
                )));
            }
            if let Some(state) = &item.excluded {
                return Err(FactoryError::BadRequest(format!(
                    "{} is excluded ({state}); there is nothing left to report",
                    mark_item
                )));
            }
            if let Some(mark) = &clock {
                let deadline = item
                    .deadlines
                    .iter()
                    .find(|d| d.deadline == mark.deadline)
                    .ok_or_else(|| FactoryError::BadRequest("record an evidenced corrective measure before submitting the final report".into()))?;
                if matches!(deadline.state, ClockDeadlineState::Met | ClockDeadlineState::Late) {
                    return Err(FactoryError::BadRequest(format!(
                        "{} already has a live submission for its {} deadline",
                        mark.item, mark.deadline
                    )));
                }
            }
            if corrective.is_some() && item.corrective_measure.is_some() {
                return Err(FactoryError::BadRequest("this item already has a live corrective-measure record; withdraw it before correcting it".into()));
            }
        }

        let attestation = Attestation {
            id: uuid::Uuid::new_v4().to_string(),
            control,
            scope,
            evidence,
            note,
            attested_by: caller_name(caller),
            attested_at: now,
            expires_at,
            withdrawn: None,
            clock,
            corrective,
        };
        self.policies.append_attestation(&attestation).await?;
        Ok(attestation)
    }

    /// Withdraw a previously recorded attestation: `Request::PolicyWithdraw`.
    /// Appends a new row referencing `id` -- the store refuses a second
    /// withdrawal of the same attestation on its own, so this only refuses
    /// the friendlier way, before the write is even attempted.
    pub(crate) async fn policy_withdraw(&self, caller: &Caller, id: String, reason: Option<String>) -> Result<Attestation> {
        let existing = self
            .policies
            .get(&id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such attestation: {id:?}")))?;
        if existing.withdrawn.is_some() {
            return Err(FactoryError::BadRequest(format!("attestation {id:?} is already withdrawn")));
        }
        let withdrawal = Withdrawal {
            at: Utc::now(),
            by: caller_name(caller),
            reason,
        };
        self.policies
            .append_withdrawal(&id, &existing.control, &existing.scope, &withdrawal)
            .await?;
        let mut withdrawn = existing;
        withdrawn.withdrawn = Some(withdrawal);
        Ok(withdrawn)
    }

    /// Close a gap: `Request::PolicyRemediate`. Creates the task through the
    /// exact path `Request::TaskCreate` itself uses -- `Engine::create`, the
    /// same function that request's own dispatch arm calls -- so every
    /// validation it does (agent/runtime resolution, an empty title, a
    /// schedule/retry mismatch) and its `Event::TaskCreated` apply here too.
    /// Never routed through `self.handle_request(Request::TaskCreate(..))`,
    /// which would re-enter as the owner and launder whatever grant the
    /// real caller holds into full, unscoped task-creation authority.
    ///
    /// Refused outright when `control` is already `satisfied`, `attested`,
    /// or `n/a` at `scope` -- there is no gap to remediate. Otherwise,
    /// refused, naming the existing task, when a non-terminal task already
    /// carries the label `policy=<framework>/<id>` in `scope` -- an exact
    /// match against `control`'s own `framework/id` spelling, never a
    /// title guess the way `task`/`workflow` checks have to fall back to.
    pub(crate) async fn policy_remediate(&self, control: ControlRef, scope: String, agent: Option<String>) -> Result<Task> {
        let snapshot = self.factory_snapshot();
        let scope = snapshot.scope(&scope)?.name.clone();
        // Reuses the same evaluation `Request::PolicyControl` itself
        // answers with -- status, reasons, refs, the catalogue's own
        // `remediation:` text -- rather than a second, parallel pass over
        // the catalogue and the evidence stores.
        let detail = self.policy_control(control.clone(), &scope).await?;

        match detail.status.kind() {
            policy::StatusKind::Satisfied => {
                return Err(FactoryError::BadRequest(format!(
                    "{control} is already satisfied at {scope:?}; nothing to remediate"
                )));
            }
            policy::StatusKind::Attested => {
                return Err(FactoryError::BadRequest(format!(
                    "{control} is already attested at {scope:?}; nothing to remediate"
                )));
            }
            policy::StatusKind::NotApplicable => {
                let rationale = detail.not_applicable.as_ref().map(|na| na.rationale.as_str()).unwrap_or("");
                return Err(FactoryError::BadRequest(format!(
                    "{control} is not applicable at {scope:?}: {rationale}"
                )));
            }
            policy::StatusKind::Open | policy::StatusKind::Stale => {}
        }

        if let Some(task) = self.open_policy_task(&control, &scope).await? {
            return Err(FactoryError::BadRequest(format!(
                "a task to close {control} at {scope:?} is already open: {} ({:?})",
                task.id, task.title
            )));
        }

        let mut labels = BTreeMap::new();
        labels.insert("policy".to_string(), control.to_string());
        let new_task = NewTask {
            title: format!("Close {control}: {}", detail.title),
            instructions: policy::remediation_instructions(
                &control,
                detail.remediation.as_deref(),
                &detail.status,
                &detail.checks,
            ),
            scope: Some(scope),
            agent,
            labels,
            ..Default::default()
        };
        self.create(new_task).await
    }

    /// The non-terminal task in `scope` labelled `policy=<control>`, if one
    /// is open: what `policy_remediate` refuses a second task over, and what
    /// `policy_control` reports as `open_task` (`#98`). An exact match on
    /// the label, never a title guess.
    async fn open_policy_task(&self, control: &ControlRef, scope: &str) -> Result<Option<Task>> {
        let label = control.to_string();
        Ok(self
            .store
            .list(&TaskFilter {
                scope: Some(scope.to_string()),
                ..Default::default()
            })
            .await?
            .into_iter()
            .find(|t| open_policy_label(t) == Some(label.as_str())))
    }

    /// `Request::PolicyExport`'s data: `policy_report(scope)` plus, per row,
    /// every attestation recorded at that row's own scope or an ancestor of
    /// it -- the same "scope or ancestor" rule this module's own `Evidence`
    /// assembly already applies per scope, kept here rather than folded
    /// into a status and discarded. An audit wants a control's *whole*
    /// attestation history at a scope (withdrawn and expired included), not
    /// only whichever one currently decides its status the way
    /// `ControlStatus.refs` does.
    pub(crate) async fn policy_export(&self, scope: Option<&str>) -> Result<policy_export::PolicyExport> {
        let snapshot = self.factory_snapshot();
        let report = self.policy_report(scope).await?;
        let all_attestations = self.policies.all().await?;

        let mut attestations: BTreeMap<String, Vec<Attestation>> = BTreeMap::new();
        for row in &report.rows {
            let scope_obj = snapshot.scope(&row.scope)?.clone();
            let ancestor_names: BTreeSet<&str> = snapshot
                .config
                .ancestors_of(&scope_obj)
                .iter()
                .map(|ancestor| ancestor.name.as_str())
                .collect();
            let relevant: Vec<Attestation> = all_attestations
                .iter()
                .filter(|att| att.scope == row.scope || ancestor_names.contains(att.scope.as_str()))
                .cloned()
                .collect();
            attestations.insert(row.scope.clone(), relevant);
        }

        Ok(policy_export::PolicyExport {
            instance: snapshot.config.instance.name.clone(),
            scope: report.scope.clone(),
            produced_at: Utc::now(),
            report,
            attestations,
        })
    }

    /// `policy_export`, rendered as `format` (`"md"` or `"json"`; anything
    /// else is refused) -- the filename and body `Payload::PolicyExport`
    /// carries. Kept beside `policy_export` rather than in `engine.rs`'s
    /// dispatch, since the `http` interface's own export route needs the
    /// same two values without going through the ordinary `Payload`
    /// envelope (`interfaces/http.rs`'s `policy_export` handler builds a
    /// download response from them instead).
    pub(crate) async fn policy_export_render(&self, scope: Option<&str>, format: &str) -> Result<(String, String)> {
        let export = self.policy_export(scope).await?;
        let filename = policy_export::export_filename(&export, format);
        let body = match format {
            "md" => policy_export::export_markdown(&export),
            "json" => serde_json::to_string_pretty(&export)
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("rendering policy export: {e}")))?,
            other => {
                return Err(FactoryError::BadRequest(format!(
                    "unknown export format {other:?}; expected \"md\" or \"json\""
                )));
            }
        };
        Ok((filename, body))
    }
}

/// `attested_by`/`Withdrawal.by`: the owner speaks as `"owner"`, an agent as
/// its own name -- not `scope/name`, since an attestation's `scope` field
/// already says where it was recorded for.
/// Who a request is recorded as coming from, in words fit for an audit
/// trail -- shared with `goals/mod.rs`'s check-ins, the same "who" a
/// `CheckIn.by`/`Attestation.attested_by` both want.
pub(crate) fn caller_name(caller: &Caller) -> String {
    match caller {
        Caller::Owner => "owner".to_string(),
        Caller::Agent { name, .. } => name.clone(),
    }
}

/// A task's `policy=` label while it is still open -- `None` once it is
/// terminal, or when it carries no such label. The one predicate
/// `policy_report`'s `open_tasks` and `open_policy_task` both read, so the
/// tab's "Task open" and the remediation refusal can never disagree.
fn open_policy_label(task: &Task) -> Option<&str> {
    if task.status.is_terminal() {
        return None;
    }
    task.labels.get("policy").map(String::as_str)
}

#[cfg(test)]
mod tests {
    //! The engine-level report and write paths, on a temporary instance --
    //! not `policy::` itself (covered on its own in `factory-core`) and not
    //! `PolicyStore` (covered in `store.rs`), but the two glued together the
    //! way a real request sees them: the catalogues and the knowledge vault
    //! on disk, a real scope tree, and the store behind `Engine`.

    use super::*;
    use factory_core::config::ForemanConfig;
    use chrono::DateTime;
    use factory_core::adapter::store::task_from_new;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, InterfaceConfig, PolicyDeclaration, Scope};
    use factory_core::control_plan::{
        AttestationVerdict, RequiredStep, StepAttestation, StepKind, GATE_ACTOR,
    };
    use factory_core::dependencies::AttachmentKind;
    use factory_core::policy::{ControlRef, ControlStatus, StatusKind};
    use factory_core::protocol::Payload;
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
    use factory_core::task::NewTask;
    use factory_plugins::{Registry, SqliteStore};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A scope at `path` named `name`, declaring `policies` inline --
    /// mirrors `factory_core::config`'s own `scope_with_policies` test
    /// helper. Names and paths deliberately differ so a test that gets the
    /// tree wrong by name rather than by path fails loudly.
    fn scope_at(id: &str, name: &str, path: &str, policies_yaml: &str) -> Scope {
        let mut yaml = format!("id: {id}\nname: {name}\n");
        if !policies_yaml.is_empty() {
            yaml.push_str(&format!("policies:\n{policies_yaml}"));
        }
        let mut scope: Scope = serde_yaml_ng::from_str(&yaml).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// `company` (`.`) is root and commits the instance to `cra` for every
    /// scope, via the top-level `policies:` every chain always prepends.
    /// `engineering` (`projects`) adds nothing of its own; `demo-app`
    /// (`projects/demo`), below it, marks control `c` not applicable.
    /// `sibling` (`other`) sits outside `projects` entirely, to prove a
    /// scope query never reaches sideways.
    ///
    /// The catalogue: `a` is a `knowledge` check whose tag the vault
    /// carries (so it is `satisfied` everywhere), `b` is an `attestation`
    /// check (so it starts `open` everywhere), `c` is a `knowledge` check
    /// whose tag nothing carries (so it is `open` wherever it still
    /// applies, and `not_applicable` at `demo-app` alone), and `d`/`e`/`f`
    /// are `task`/`workflow`/`gate` checks naming a task, workflow and
    /// dataset no test creates by default (so they start `open` everywhere
    /// too -- see the tests below, which create one in the real store).
    fn test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-policies-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::write(
            root.join(".factory/policies/cra.yaml"),
            "framework: cra\n\
             title: Cyber Resilience Act\n\
             kind: regulation\n\
             controls:\n\
             \x20\x20- id: a\n\x20\x20\x20\x20title: Control A\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n\
             \x20\x20- id: b\n\x20\x20\x20\x20title: Control B\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: attestation\n\
             \x20\x20- id: c\n\x20\x20\x20\x20title: Control C\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: knowledge\n\
             \x20\x20- id: d\n\x20\x20\x20\x20title: Control D\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: task\n\x20\x20\x20\x20\x20\x20\x20\x20task: sbom export\n\x20\x20\x20\x20\x20\x20\x20\x20max_age: 7d\n\
             \x20\x20- id: e\n\x20\x20\x20\x20title: Control E\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: workflow\n\x20\x20\x20\x20\x20\x20\x20\x20workflow: release train\n\x20\x20\x20\x20\x20\x20\x20\x20max_age: 7d\n\
             \x20\x20- id: f\n\x20\x20\x20\x20title: Control F\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: gate\n\x20\x20\x20\x20\x20\x20\x20\x20dataset: smoke\n\x20\x20\x20\x20\x20\x20\x20\x20max_age: 7d\n",
        )
        .unwrap();

        std::fs::create_dir_all(root.join(".factory/knowledge")).unwrap();
        std::fs::write(
            root.join(".factory/knowledge/page.md"),
            "---\ntags: [control/cra/a]\n---\n# Page\n",
        )
        .unwrap();

        let mut config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig::default(),
            scope: None,
            scopes: Vec::new(),
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration {
                frameworks: vec!["cra".to_string()],
                ..Default::default()
            },
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        config.scopes = vec![
            scope_at("company-id", "company", ".", ""),
            scope_at("engineering-id", "engineering", "projects", ""),
            scope_at(
                "demo-app-id",
                "demo-app",
                "projects/demo",
                "  not_applicable:\n    - control: cra/c\n      rationale: \"we ship no hardware\"\n",
            ),
            scope_at("sibling-id", "sibling", "other", ""),
        ];

        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    fn by_id(statuses: &[ControlStatus]) -> BTreeMap<&str, &ControlStatus> {
        statuses.iter().map(|s| (s.control.id.as_str(), s)).collect()
    }

    #[tokio::test]
    async fn a_scope_query_covers_the_asked_scope_and_its_descendants_only() {
        let engine = test_engine();

        let whole = engine.policy_report(None).await.unwrap();
        let scopes: std::collections::BTreeSet<&str> = whole.rows.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["company", "engineering", "demo-app", "sibling"]),
            "cra applies everywhere via the root's top-level policies:, so every scope gets a row"
        );

        let narrowed = engine.policy_report(Some("engineering")).await.unwrap();
        let scopes: std::collections::BTreeSet<&str> =
            narrowed.rows.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(
            scopes,
            std::collections::BTreeSet::from(["engineering", "demo-app"]),
            "excludes company (an ancestor) and sibling (not under projects at all)"
        );
    }

    #[tokio::test]
    async fn statuses_reflect_the_knowledge_tag_and_the_declared_not_applicable() {
        let engine = test_engine();
        let report = engine.policy_report(Some("engineering")).await.unwrap();

        let engineering = &report.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses;
        let e = by_id(engineering);
        assert_eq!(e["a"].status.kind(), StatusKind::Satisfied, "the vault carries a's tag");
        assert_eq!(e["b"].status.kind(), StatusKind::Open, "no attestation yet");
        assert_eq!(e["c"].status.kind(), StatusKind::Open, "not marked n/a at engineering itself");

        let demo_app = &report.rows.iter().find(|r| r.scope == "demo-app").unwrap().statuses;
        let d = by_id(demo_app);
        assert_eq!(d["c"].status.kind(), StatusKind::NotApplicable);

        assert_eq!(report.not_applicable.len(), 1);
        assert_eq!(report.not_applicable[0].control, "cra/c".parse().unwrap());
        assert_eq!(report.not_applicable[0].scope, "demo-app");

        // The subtree rollup takes the worst across scopes: c is open for
        // real at engineering even though demo-app opts out of it, so the
        // n/a scope must not paper over the one that still answers for it.
        let cra = report.rollup.iter().find(|r| r.framework == "cra").unwrap();
        assert!(!cra.compliant, "b is open everywhere and c is open at engineering");
    }

    #[tokio::test]
    async fn attesting_and_withdrawing_moves_a_controls_status_and_stops_at_its_own_scope() {
        let engine = test_engine();
        let owner = Caller::Owner;

        let attestation = engine
            .policy_attest(
                &owner,
                "cra/b".parse().unwrap(),
                "demo-app".to_string(),
                "https://example.com/policy".to_string(),
                None,
                Utc::now() + chrono::Duration::days(30),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(attestation.attested_by, "owner");
        assert_eq!(attestation.scope, "demo-app");

        let report = engine.policy_report(None).await.unwrap();
        let demo_app = by_id(&report.rows.iter().find(|r| r.scope == "demo-app").unwrap().statuses);
        assert_eq!(demo_app["b"].status.kind(), StatusKind::Attested);

        let engineering = by_id(&report.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses);
        assert_eq!(
            engineering["b"].status.kind(),
            StatusKind::Open,
            "an attestation recorded at a descendant does not reach its ancestor"
        );

        let withdrawn = engine
            .policy_withdraw(&owner, attestation.id.clone(), Some("superseded".to_string()))
            .await
            .unwrap();
        assert!(withdrawn.withdrawn.is_some());

        let report = engine.policy_report(Some("demo-app")).await.unwrap();
        let demo_app = by_id(&report.rows.iter().find(|r| r.scope == "demo-app").unwrap().statuses);
        assert_eq!(demo_app["b"].status.kind(), StatusKind::Open, "withdrawn, so back to open");

        let err = engine
            .policy_withdraw(&owner, attestation.id.clone(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already withdrawn"), "{err}");
    }

    #[tokio::test]
    async fn policy_attest_refuses_empty_evidence_past_expiry_and_a_control_that_does_not_apply() {
        let engine = test_engine();
        let owner = Caller::Owner;

        let err = engine
            .policy_attest(
                &owner,
                "cra/b".parse().unwrap(),
                "demo-app".to_string(),
                "".to_string(),
                None,
                Utc::now() + chrono::Duration::days(1),
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("evidence"), "{err}");

        let err = engine
            .policy_attest(
                &owner,
                "cra/b".parse().unwrap(),
                "demo-app".to_string(),
                "https://example.com".to_string(),
                None,
                Utc::now() - chrono::Duration::days(1),
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("future"), "{err}");

        let err = engine
            .policy_attest(
                &owner,
                "cra/zzz".parse().unwrap(),
                "demo-app".to_string(),
                "https://example.com".to_string(),
                None,
                Utc::now() + chrono::Duration::days(1),
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not apply"), "{err}");

        // `c` is marked not applicable at demo-app -- attesting it there is
        // refused too, even though the control itself exists and applies
        // elsewhere in the tree.
        let err = engine
            .policy_attest(
                &owner,
                "cra/c".parse().unwrap(),
                "demo-app".to_string(),
                "https://example.com".to_string(),
                None,
                Utc::now() + chrono::Duration::days(1),
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not applicable"), "{err}");
    }

    #[tokio::test]
    async fn policy_control_carries_full_history_including_a_withdrawn_attestation() {
        let engine = test_engine();
        let owner = Caller::Owner;

        let first = engine
            .policy_attest(
                &owner,
                "cra/b".parse().unwrap(),
                "demo-app".to_string(),
                "https://one.example.com".to_string(),
                None,
                Utc::now() + chrono::Duration::days(30),
                None,
                None,
            )
            .await
            .unwrap();
        engine.policy_withdraw(&owner, first.id.clone(), None).await.unwrap();
        engine
            .policy_attest(
                &owner,
                "cra/b".parse().unwrap(),
                "demo-app".to_string(),
                "https://two.example.com".to_string(),
                None,
                Utc::now() + chrono::Duration::days(30),
                None,
                None,
            )
            .await
            .unwrap();

        let detail = engine.policy_control("cra/b".parse().unwrap(), "demo-app").await.unwrap();
        assert_eq!(detail.control, "cra/b".parse().unwrap());
        assert_eq!(detail.attestations.len(), 2, "both attestations, including the withdrawn one");
        assert!(detail.attestations.iter().any(|a| a.id == first.id && a.withdrawn.is_some()));
        assert_eq!(detail.status.kind(), StatusKind::Attested, "the second, unwithdrawn one still covers it");
    }

    /// `#81`, through the real store: `d`'s `task` check names `sbom export`
    /// by title. With no such task at all it is `open`; once one exists in
    /// `engineering` with a fresh `done` run, `resolve_task_and_workflow_facts`
    /// resolves it, `evaluate` calls it `satisfied`, and both
    /// `policy_report` and `policy_control` carry a `refs` entry pointing at
    /// the task and the run -- not just the pure `factory_core::policy`
    /// logic (covered on its own), but the whole path from a real
    /// `TaskStore` through the engine.
    #[tokio::test]
    async fn a_task_check_resolves_against_a_real_task_and_run_in_the_store() {
        let engine = test_engine();

        let before = engine.policy_report(Some("engineering")).await.unwrap();
        let before_engineering = by_id(&before.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses);
        assert_eq!(
            before_engineering["d"].status.kind(),
            StatusKind::Open,
            "no task named `sbom export` exists yet"
        );

        let new_task = factory_core::adapter::store::task_from_new(
            factory_core::task::NewTask {
                title: "sbom export".to_string(),
                ..Default::default()
            },
            "engineering".to_string(),
            "assistant".to_string(),
            "shell".to_string(),
        );
        let task = engine.store.create(&new_task).await.unwrap();
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "assistant".to_string(),
                adapter: "shell".to_string(),
                runtime: "shell".to_string(),
                token: "test-token".to_string(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(Utc::now()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        // The whole tree, so `sibling` (which `Some("engineering")` would
        // exclude) is in the same report to compare against.
        let report = engine.policy_report(None).await.unwrap();
        let engineering = by_id(&report.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses);
        assert_eq!(engineering["d"].status.kind(), StatusKind::Satisfied, "{:?}", engineering["d"].status);
        assert!(engineering["d"].refs.iter().any(|r| r.id == task.id));
        assert!(engineering["d"].refs.iter().any(|r| r.id == run.id));

        // A sibling scope's own task list never satisfies `engineering`'s
        // check -- `task` is resolved per evaluated scope, not instance-wide.
        let sibling = by_id(&report.rows.iter().find(|r| r.scope == "sibling").unwrap().statuses);
        assert_eq!(sibling["d"].status.kind(), StatusKind::Open);

        let detail = engine.policy_control("cra/d".parse().unwrap(), "engineering").await.unwrap();
        assert_eq!(detail.status.kind(), StatusKind::Satisfied);
        assert!(detail.refs.iter().any(|r| r.id == task.id));
    }

    /// The same, for `e`'s `workflow` check, through the real
    /// `WorkflowStore` `Engine::new` already gives a test engine.
    #[tokio::test]
    async fn a_workflow_check_resolves_against_a_real_workflow_and_run_in_the_store() {
        let engine = test_engine();

        let before = engine.policy_report(Some("engineering")).await.unwrap();
        let before_engineering = by_id(&before.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses);
        assert_eq!(before_engineering["e"].status.kind(), StatusKind::Open, "no workflow named `release train` exists yet");

        let draft = factory_core::workflow::WorkflowDraft {
            name: "release train".to_string(),
            description: "ships things".to_string(),
            scope: "engineering".to_string(),
            ..Default::default()
        };
        let definition = factory_core::workflow::WorkflowDefinition::from_draft(draft);
        engine.workflows.put_definition(&definition).await.unwrap();

        let mut run = factory_core::workflow::WorkflowRun::new(definition, factory_core::workflow::WorkflowActor::Owner);
        run.status = factory_core::workflow::WorkflowRunStatus::Done;
        engine.workflows.put_run(&run).await.unwrap();

        let report = engine.policy_report(Some("engineering")).await.unwrap();
        let engineering = by_id(&report.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses);
        assert_eq!(engineering["e"].status.kind(), StatusKind::Satisfied, "{:?}", engineering["e"].status);
        assert!(engineering["e"].refs.iter().any(|r| r.id == run.id));
    }

    /// The same, for `f`'s `gate` check, through the real `BenchStore` --
    /// `evidence.gates` is resolved once per report (`gate_facts_for`), not
    /// once per scope, so this also exercises that a scope that never named
    /// `smoke` still gets a status for it once `engineering` does.
    #[tokio::test]
    async fn a_gate_check_resolves_against_a_real_bench_run_in_the_store() {
        let engine = test_engine();

        let before = engine.policy_report(Some("engineering")).await.unwrap();
        let before_engineering = by_id(&before.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses);
        assert_eq!(before_engineering["f"].status.kind(), StatusKind::Open, "no settled bench run of `smoke` exists yet");

        let case = factory_core::dataset::Case {
            id: "case-1".to_string(),
            title: "Case 1".to_string(),
            scope: "engineering".to_string(),
            instructions: String::new(),
            base: None,
            reset: None,
            gate: Some("exit 0".to_string()),
            timeout_seconds: None,
            origin: None,
        };
        let run = factory_core::bench::BenchRun {
            id: "bench-1".to_string(),
            dataset: "smoke".to_string(),
            dataset_revision: 1,
            cases: vec![case],
            case_bases: BTreeMap::new(),
            agents: vec!["assistant".to_string()],
            attempts_per_case: 1,
            concurrency: 1,
            status: factory_core::bench::BenchRunStatus::Done,
            attempts: Vec::new(),
            started_at: Utc::now() - chrono::Duration::hours(1),
            ended_at: Some(Utc::now()),
        };
        engine.bench.put_run(&run).await.unwrap();
        let mut attempt =
            factory_core::bench::BenchAttempt::pending("attempt-1".to_string(), "case-1".to_string(), "assistant".to_string(), 1);
        attempt.verdict = Some(factory_core::bench::Verdict::Pass);
        engine.bench.put_attempt(&run.id, &attempt).await.unwrap();

        let report = engine.policy_report(None).await.unwrap();
        let engineering = by_id(&report.rows.iter().find(|r| r.scope == "engineering").unwrap().statuses);
        assert_eq!(engineering["f"].status.kind(), StatusKind::Satisfied, "{:?}", engineering["f"].status);
        assert!(engineering["f"].refs.iter().any(|r| r.id == "bench-1"));

        // `demo-app` names the same dataset -- the shared `gates` resolution
        // (`gate_facts_for`, called once for the whole report) must answer
        // it too, not just the scope that happened to trigger the lookup.
        let demo_app = by_id(&report.rows.iter().find(|r| r.scope == "demo-app").unwrap().statuses);
        assert_eq!(demo_app["f"].status.kind(), StatusKind::Satisfied, "{:?}", demo_app["f"].status);
    }

    // -- roles / sandbox / secrets / daemon ----------------------------------

    /// A second small instance, just for `roles`/`sandbox`/`secrets`/`daemon`:
    /// `root` (the instance root -- named exactly `root` so `ForemanConfig`'s
    /// own default `exclude: ["root"]` keeps it foreman-free) and `team`
    /// (`projects/team`), which declares one `worker` agent with
    /// `sandbox: docker`. `daemon.foreman` is turned on, so `team` also gets
    /// a synthesised foreman -- hard-coded `Sandbox::None` -- proving both
    /// halves of the documented choice to count it in `Scope::agents_with`:
    /// it holds `policy.attest` (the `foreman` preset's grants are
    /// `Grant::ALL`), so `g` (forbidding it) goes `open` at `team` even
    /// though the one agent someone actually *declared* holds nothing of the
    /// kind; and it is the reason `h` (sandbox) still finds something
    /// missing at `team` even though `worker` itself has one.
    ///
    /// `root`'s own `.env` is created on disk so `i` (secrets, default
    /// `absent: [scope_env]`) is `open` there and `satisfied` at `team`,
    /// which has none -- both branches in one engine, and neither reads a
    /// home-directory credential a developer's machine might or might not
    /// have.
    fn l123_test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-policies-l123-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::write(
            root.join(".factory/policies/house.yaml"),
            "framework: house\n\
             title: House rules\n\
             kind: best-practice\n\
             controls:\n\
             \x20\x20- id: g\n\x20\x20\x20\x20title: No role may attest\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: roles\n\x20\x20\x20\x20\x20\x20\x20\x20forbid: [policy.attest]\n\
             \x20\x20- id: h\n\x20\x20\x20\x20title: Every agent is sandboxed\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: sandbox\n\
             \x20\x20- id: i\n\x20\x20\x20\x20title: No plaintext .env\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: secrets\n\
             \x20\x20- id: j\n\x20\x20\x20\x20title: Power assertion is held\n\x20\x20\x20\x20evidence:\n\x20\x20\x20\x20\x20\x20- check: daemon\n\x20\x20\x20\x20\x20\x20\x20\x20fact: power_assertion\n",
        )
        .unwrap();
        std::fs::write(root.join(".env"), "SECRET=shh\n").unwrap();

        let mut config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig {
                foreman: ForemanConfig { enabled: true, ..ForemanConfig::default() },
                ..DaemonConfig::default()
            },
            scope: None,
            scopes: Vec::new(),
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration { frameworks: vec!["house".to_string()], ..Default::default() },
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let root_scope = scope_at("root-id", "root", ".", "");
        let mut team_scope: Scope = serde_yaml_ng::from_str(
            "id: team-id\nname: team\nagents:\n  - name: worker\n    harness: shell\n    role: worker\n    sandbox: docker\n",
        )
        .unwrap();
        team_scope.path = PathBuf::from("projects/team");
        config.scopes = vec![root_scope, team_scope];

        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    #[tokio::test]
    async fn fact_ports_read_secret_presence_live_and_keep_scopes_separate() {
        let engine = l123_test_engine();
        let facts = Facts::<L6>::new(&engine);
        let scopes = ["root".to_string(), "team".to_string()].into_iter().collect();
        let before = facts.get::<SecretsPresence>(&scopes).await.unwrap();
        assert_eq!(before["root"].get("scope_env"), Some(&true));
        assert_eq!(before["team"].get("scope_env"), Some(&false));
        // Only this test's throwaway .env, never the real instance.
        std::fs::remove_file(engine.factory_snapshot().root.join(".env")).unwrap();
        let after = facts.get::<SecretsPresence>(&scopes).await.unwrap();
        assert_eq!(after["root"].get("scope_env"), Some(&false), "not cached");
        assert!(facts.get::<SecretsPresence>(&BTreeSet::new()).await.unwrap().is_empty());
        assert!(facts.get::<SecretsPresence>(&["missing".into()].into_iter().collect()).await.is_err());
    }

    #[tokio::test]
    async fn knowledge_tag_port_observes_file_changes_without_an_event_or_cache() {
        let engine = test_engine();
        let facts = Facts::<L6>::new(&engine);
        assert!(facts.get::<factory_kernel::KnowledgeTags>(&()).await.unwrap().tags.contains("control/cra/a"));
        std::fs::write(engine.factory_snapshot().root.join(".factory/knowledge/page.md"),
            "---\ntags: [control/cra/changed]\n---\n# Updated\n").unwrap();
        let tags = facts.get::<factory_kernel::KnowledgeTags>(&()).await.unwrap().tags;
        assert!(tags.contains("control/cra/changed"));
        assert!(!tags.contains("control/cra/a"));
    }

    #[tokio::test]
    async fn task_fact_port_preserves_id_priority_ambiguity_and_exact_scope() {
        let engine = test_engine();
        let mut ids = Vec::new();
        for scope in ["engineering", "engineering", "sibling"] {
            let task = task_from_new(NewTask { title: "shared".into(), ..Default::default() },
                scope.into(), "shell".into(), "quiet".into());
            ids.push(engine.store.create(&task).await.unwrap().id);
        }
        let facts = Facts::<L6>::new(&engine).get::<TaskFact>(&NamedQuery {
            scope: "engineering".into(),
            names: ["shared".into(), ids[0].clone(), "absent".into()].into_iter().collect(),
        }).await.unwrap();
        assert_eq!(facts["shared"].len(), 2);
        assert!(facts["shared"].iter().all(|t| t.runs.is_empty() && t.id != ids[2]));
        assert_eq!(facts[&ids[0]].len(), 1);
        assert_eq!(facts[&ids[0]][0].id, ids[0]);
        assert!(facts["absent"].is_empty());
        assert!(Facts::<L6>::new(&engine).get::<TaskFact>(&NamedQuery {
            scope: "missing".into(), names: ["shared".into()].into_iter().collect(),
        }).await.is_err());
    }

    /// A bare-bones engine for the F8 mutation test below (`#193`, phase 1):
    /// no policy catalogue at all -- `infrastructure()` and `daemon_facts()`
    /// need none -- just an interfaces list the caller sets directly.
    fn engine_with_interfaces(interfaces: Vec<InterfaceConfig>) -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-policies-f8-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { interfaces, ..DaemonConfig::default() },
            scope: None,
            scopes: Vec::new(),
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    fn http_interface(bind: Option<&str>) -> InterfaceConfig {
        let mut settings = BTreeMap::new();
        if let Some(bind) = bind {
            settings.insert("bind".to_string(), serde_yaml_ng::Value::String(bind.to_string()));
        }
        InterfaceConfig { kind: "http".into(), settings }
    }

    #[tokio::test]
    async fn roles_is_satisfied_with_no_agent_and_open_once_a_synthesised_foreman_holds_the_forbidden_grant() {
        let engine = l123_test_engine();
        let report = engine.policy_report(None).await.unwrap();

        let root = by_id(&report.rows.iter().find(|r| r.scope == "root").unwrap().statuses);
        assert_eq!(root["g"].status.kind(), StatusKind::Satisfied, "{:?}", root["g"].status);
        assert!(root["g"].status.reasons().iter().any(|r| r.contains("no agent declared")), "{:?}", root["g"].status);

        let team = by_id(&report.rows.iter().find(|r| r.scope == "team").unwrap().statuses);
        assert_eq!(team["g"].status.kind(), StatusKind::Open, "{:?}", team["g"].status);
        assert!(
            team["g"].status.reasons().iter().any(|r| r.contains("foreman") && r.contains("policy.attest")),
            "the synthesised foreman, not the declared worker, is what holds it: {:?}",
            team["g"].status
        );
    }

    #[tokio::test]
    async fn sandbox_is_open_at_a_scope_whose_only_gap_is_its_synthesised_foreman() {
        let engine = l123_test_engine();
        let report = engine.policy_report(None).await.unwrap();

        let root = by_id(&report.rows.iter().find(|r| r.scope == "root").unwrap().statuses);
        assert_eq!(root["h"].status.kind(), StatusKind::Satisfied, "{:?}", root["h"].status);

        let team = by_id(&report.rows.iter().find(|r| r.scope == "team").unwrap().statuses);
        assert_eq!(team["h"].status.kind(), StatusKind::Open, "{:?}", team["h"].status);
        assert!(
            team["h"].status.reasons().iter().any(|r| r.contains("foreman")),
            "worker declares docker; only the synthesised foreman is missing one: {:?}",
            team["h"].status
        );
    }

    #[tokio::test]
    async fn secrets_reads_each_scopes_own_env_never_a_sibling_scopes() {
        let engine = l123_test_engine();
        let report = engine.policy_report(None).await.unwrap();

        let root = by_id(&report.rows.iter().find(|r| r.scope == "root").unwrap().statuses);
        assert_eq!(root["i"].status.kind(), StatusKind::Open, "{:?}", root["i"].status);
        assert!(root["i"].status.reasons().iter().any(|r| r.contains("scope_env")), "{:?}", root["i"].status);

        let team = by_id(&report.rows.iter().find(|r| r.scope == "team").unwrap().statuses);
        assert_eq!(team["i"].status.kind(), StatusKind::Satisfied, "team has no .env of its own: {:?}", team["i"].status);
    }

    #[tokio::test]
    async fn daemon_reads_the_same_fact_for_every_scope() {
        let engine = l123_test_engine();
        let report = engine.policy_report(None).await.unwrap();

        for scope in ["root", "team"] {
            let statuses = by_id(&report.rows.iter().find(|r| r.scope == scope).unwrap().statuses);
            assert_eq!(
                statuses["j"].status.kind(),
                StatusKind::Satisfied,
                "power_assertion defaults to true: {:?} ({scope})",
                statuses["j"].status
            );
        }
    }

    #[tokio::test]
    async fn daemon_facts_reads_the_default_http_bind_as_loopback_only() {
        let engine = l123_test_engine();
        let facts = Facts::<L6>::new(&engine).get::<DaemonConfigFact>(&()).await.unwrap();
        assert!(facts.foreman_enabled);
        assert_eq!(facts.http_loopback_only, Some(true), "the default bind, 127.0.0.1:8787, is loopback");
        assert!(facts.power_assertion);
    }

    /// `#193`, phase 1, F8's mutation test: `daemon_facts`'s
    /// `http_loopback_only` must equal "all HTTP binds in
    /// `infrastructure().interfaces` are loopback" over four configs -- no
    /// `http` interface at all, an explicit loopback bind, a wide-open
    /// `0.0.0.0` bind, and a bare hostname that cannot be parsed as a socket
    /// address. Checking against both an oracle computed straight from
    /// `infrastructure()`'s own output *and* a hardcoded expectation catches
    /// either side silently going back to deriving the bind on its own --
    /// two derivations that drifted the same wrong way would still agree
    /// with each other, but not with the hardcoded value.
    #[tokio::test]
    async fn http_loopback_only_matches_infrastructures_own_interfaces_over_four_configs() {
        async fn check(interfaces: Vec<InterfaceConfig>, want: Option<bool>) {
            let engine = engine_with_interfaces(interfaces);

            let Payload::Infrastructure { daemon, .. } = engine.infrastructure().await else {
                panic!("not an infrastructure payload");
            };
            let oracle: Option<bool> = {
                let http_binds: Vec<&str> =
                    daemon.interfaces.iter().filter(|i| i.kind == "http").filter_map(|i| i.bind.as_deref()).collect();
                if http_binds.is_empty() {
                    Some(true)
                } else {
                    http_binds
                        .iter()
                        .map(|bind| bind.parse::<std::net::SocketAddr>().map(|addr| addr.ip().is_loopback()))
                        .collect::<std::result::Result<Vec<bool>, _>>()
                        .ok()
                        .map(|loopback| loopback.iter().all(|l| *l))
                }
            };
            assert_eq!(oracle, want, "the oracle itself must match the fixed expectation for this config");
            assert_eq!(
                Facts::<L6>::new(&engine).get::<DaemonConfigFact>(&()).await.unwrap().http_loopback_only,
                oracle,
                "policy's http_loopback_only must equal infrastructure()'s own interfaces"
            );
        }

        check(vec![], Some(true)).await;
        check(vec![http_interface(Some("127.0.0.1:9000"))], Some(true)).await;
        check(vec![http_interface(Some("0.0.0.0:8787"))], Some(false)).await;
        check(vec![http_interface(Some("example.internal:8787"))], None).await;
    }

    /// `#154`: `dataset_level_facts` never gathers the backup fact when no
    /// scope's applicable controls name a `backup_*` fact -- `house.yaml`'s
    /// `j` names only `power_assertion`. The positive side (a catalogue that
    /// does name one gathers it, and correctly) is `backup::tests`'
    /// `backup_and_verify_satisfy_a_backup_verified_control_and_leave_backup_offsite_open`,
    /// which would see `art-32-restore` stuck at "not resolved" rather than
    /// `Satisfied` if the gather were ever skipped there.
    #[tokio::test]
    async fn dataset_level_facts_never_gathers_the_backup_fact_when_no_check_names_one() {
        let engine = l123_test_engine();
        let snapshot = engine.factory_snapshot();
        let (catalogues, _findings, _tags) = engine.load_catalogues_and_tags().await.unwrap();
        let chain = engine.policy_chain("root");
        let (applied, _cf) = policy::applicable(&catalogues, &chain);
        assert!(!needs_backup_facts(&applied), "house.yaml names no backup_* fact");

        let root_scope = snapshot.config.scopes.iter().find(|s| s.name == "root").unwrap();
        let per_scope_applied = vec![(root_scope, applied)];
        let (_, _, _, backup_fact, _) = engine.dataset_level_facts(&per_scope_applied).await.unwrap();
        assert!(backup_fact.is_none(), "no check here names a backup_* fact, so it must never be gathered");
    }

    // -- policy_remediate / policy_export (#83) ------------------------------

    #[tokio::test]
    async fn policy_remediate_creates_a_labeled_task_from_the_controls_own_reasons() {
        let engine = test_engine();
        let task = engine
            .policy_remediate("cra/b".parse().unwrap(), "engineering".to_string(), None)
            .await
            .unwrap();
        assert_eq!(task.scope, "engineering");
        assert_eq!(task.labels.get("policy").map(String::as_str), Some("cra/b"));
        assert_eq!(task.title, "Close cra/b: Control B");
        assert!(task.instructions.contains("Missing evidence:"), "{}", task.instructions);
        assert!(task.instructions.contains("attestation: none recorded"), "{}", task.instructions);
        assert!(
            task.instructions.contains("Any one of these checks passing closes cra/b:"),
            "{}",
            task.instructions
        );
    }

    #[tokio::test]
    async fn policy_remediate_honours_an_explicit_agent() {
        // `test_engine`'s scopes declare no agents of their own, so the one
        // name guaranteed to resolve (`resolve_agent`'s own adapter
        // fallback) is a builtin adapter's -- `shell`, the same one the
        // rest of this file's tasks use.
        let engine = test_engine();
        let task = engine
            .policy_remediate("cra/b".parse().unwrap(), "engineering".to_string(), Some("shell".to_string()))
            .await
            .unwrap();
        assert_eq!(task.agent, "shell");
    }

    #[tokio::test]
    async fn policy_remediate_refuses_a_second_call_naming_the_open_task_and_creates_no_duplicate() {
        let engine = test_engine();
        let first = engine
            .policy_remediate("cra/b".parse().unwrap(), "engineering".to_string(), None)
            .await
            .unwrap();

        let err = engine
            .policy_remediate("cra/b".parse().unwrap(), "engineering".to_string(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains(&first.id), "{err}");
        assert!(err.to_string().contains("already open"), "{err}");

        let tasks = engine
            .store
            .list(&factory_core::task::TaskFilter {
                scope: Some("engineering".to_string()),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            tasks
                .iter()
                .filter(|t| t.labels.get("policy").map(String::as_str) == Some("cra/b"))
                .count(),
            1,
            "the refusal must never create a second task"
        );
    }

    /// `#122`: a failed remediation task is blocked, not closed, so it is
    /// still the open one and no duplicate is made.
    #[tokio::test]
    async fn policy_remediate_treats_a_failed_remediation_task_as_still_open() {
        let engine = test_engine();
        let first = engine
            .policy_remediate("cra/b".parse().unwrap(), "engineering".to_string(), None)
            .await
            .unwrap();
        engine.fail_task_for_test(&first.id, factory_core::run::FailKind::SessionGone).await;

        let err = engine
            .policy_remediate("cra/b".parse().unwrap(), "engineering".to_string(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already open") && err.to_string().contains(&first.id), "{err}");
    }

    #[tokio::test]
    async fn the_report_and_the_control_detail_name_the_open_remediation_task_until_it_ends() {
        // #98: after a reload the tab reads this, not a task list, to offer
        // "Task open" instead of "Create task".
        let engine = test_engine();
        let control: ControlRef = "cra/b".parse().unwrap();

        let before = engine.policy_report(None).await.unwrap();
        assert!(before.rows.iter().all(|r| r.open_tasks.is_empty()), "nothing open yet");
        assert_eq!(engine.policy_control(control.clone(), "engineering").await.unwrap().open_task, None);

        let task = engine.policy_remediate(control.clone(), "engineering".to_string(), None).await.unwrap();

        let report = engine.policy_report(None).await.unwrap();
        let row = |scope: &str| report.rows.iter().find(|r| r.scope == scope).unwrap().open_tasks.clone();
        assert_eq!(row("engineering").get("cra/b"), Some(&task.id));
        assert_eq!(row("engineering").len(), 1, "only the control the task is labelled for");
        // Scoped to exactly the task's own scope, like the refusal: an
        // ancestor or a descendant still has its own gap to close.
        assert!(row("company").is_empty(), "{:?}", row("company"));
        assert!(row("demo-app").is_empty(), "{:?}", row("demo-app"));
        assert!(row("sibling").is_empty(), "{:?}", row("sibling"));

        let narrowed = engine.policy_report(Some("engineering")).await.unwrap();
        assert_eq!(
            narrowed.rows.iter().find(|r| r.scope == "engineering").unwrap().open_tasks.get("cra/b"),
            Some(&task.id),
            "a scope query carries it too"
        );
        assert_eq!(
            engine.policy_control(control.clone(), "engineering").await.unwrap().open_task,
            Some(task.id.clone())
        );
        assert_eq!(engine.policy_control(control.clone(), "company").await.unwrap().open_task, None);

        // Once the task is terminal it is no longer open: the tab offers
        // "Create task" again, and the daemon lets it.
        engine
            .store
            .update(
                &task.id,
                &factory_core::task::TaskPatch {
                    status: Some(factory_core::task::TaskStatus::Done),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let after = engine.policy_report(None).await.unwrap();
        assert!(after.rows.iter().all(|r| r.open_tasks.is_empty()), "a done task is not open");
        assert_eq!(engine.policy_control(control.clone(), "engineering").await.unwrap().open_task, None);
        engine.policy_remediate(control, "engineering".to_string(), None).await.unwrap();
    }

    #[tokio::test]
    async fn the_report_leaves_open_tasks_out_of_the_wire_when_there_are_none() {
        let engine = test_engine();
        let report = engine.policy_report(None).await.unwrap();
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["rows"][0].get("open_tasks").is_none(), "{json}");
    }

    #[tokio::test]
    async fn policy_remediate_refuses_for_satisfied_attested_and_not_applicable_controls() {
        let engine = test_engine();
        let owner = Caller::Owner;

        let err = engine
            .policy_remediate("cra/a".parse().unwrap(), "engineering".to_string(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already satisfied"), "{err}");

        engine
            .policy_attest(
                &owner,
                "cra/b".parse().unwrap(),
                "demo-app".to_string(),
                "https://example.com".to_string(),
                None,
                Utc::now() + chrono::Duration::days(30),
                None,
                None,
            )
            .await
            .unwrap();
        let err = engine
            .policy_remediate("cra/b".parse().unwrap(), "demo-app".to_string(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already attested"), "{err}");

        let err = engine
            .policy_remediate("cra/c".parse().unwrap(), "demo-app".to_string(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not applicable"), "{err}");
    }

    #[tokio::test]
    async fn policy_export_carries_a_controls_full_attestation_history_including_an_ancestors() {
        let engine = test_engine();
        let owner = Caller::Owner;
        engine
            .policy_attest(
                &owner,
                "cra/b".parse().unwrap(),
                "company".to_string(),
                "https://example.com/company".to_string(),
                None,
                Utc::now() + chrono::Duration::days(30),
                None,
                None,
            )
            .await
            .unwrap();

        let export = engine.policy_export(Some("engineering")).await.unwrap();
        assert_eq!(export.scope.as_deref(), Some("engineering"));
        let engineering_atts = export.attestations.get("engineering").expect("engineering has an entry");
        assert!(
            engineering_atts
                .iter()
                .any(|a| a.control == "cra/b".parse().unwrap() && a.scope == "company"),
            "an ancestor's attestation is included in a descendant's own list: {engineering_atts:?}"
        );

        let (filename, body) = engine.policy_export_render(Some("engineering"), "md").await.unwrap();
        assert!(filename.starts_with("policy-engineering-"), "{filename}");
        assert!(body.contains("cra/b"), "{body}");
        assert!(body.contains("https://example.com/company"), "{body}");

        let (_, json) = engine.policy_export_render(Some("engineering"), "json").await.unwrap();
        assert!(json.contains("\"instance\""), "{json}");

        let err = engine.policy_export_render(Some("engineering"), "yaml").await.unwrap_err();
        assert!(err.to_string().contains("unknown export format"), "{err}");
    }

    // ------------------------------------------------ the reporting clock

    /// Writes one dependency document straight to disk in the exact layout
    /// `dependencies::write_attachment` itself produces -- there is no
    /// active run to attach through in these tests, and the daemon's write
    /// path (`Engine::attach_dependency`) needs one.
    fn write_dep_doc(
        root: &std::path::Path,
        scope: &str,
        run_id: &str,
        kind: factory_core::dependencies::AttachmentKind,
        bytes: &[u8],
        attached_at: chrono::DateTime<Utc>,
        id: &str,
    ) {
        let dir = root.join(".factory/dependencies").join(scope).join(run_id);
        std::fs::create_dir_all(&dir).unwrap();
        let filename = format!("{id}.cdx.json");
        std::fs::write(dir.join(&filename), bytes).unwrap();
        let attachment = factory_core::dependencies::Attachment {
            id: id.to_string(),
            kind,
            scope: scope.to_string(),
            run_id: run_id.to_string(),
            task_id: "t1".to_string(),
            attempt: 1,
            attached_at,
            filename,
            spec_version: "1.6".to_string(),
            states: match kind {
                factory_core::dependencies::AttachmentKind::Sbom => {
                    vec![factory_core::dependencies::LifecycleState::Built]
                }
                factory_core::dependencies::AttachmentKind::Vulnerabilities => Vec::new(),
            },
        };
        std::fs::write(
            dir.join(format!("{id}.meta.json")),
            serde_json::to_vec_pretty(&attachment).unwrap(),
        )
        .unwrap();
    }

    /// A single-scope (`demo`) instance whose only control is `cra/art-14`,
    /// backed by a real `PolicyStore` file at `db` -- so a second `Engine`
    /// built over the same `root`/`db` is what a restart looks like, the
    /// same pattern `intake.rs`'s own restart test uses.
    fn clock_engine(root: &std::path::Path, db: &std::path::Path) -> Arc<Engine> {
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: None,
            scopes: vec![scope_at("demo-id", "demo", "demo", "")],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration { frameworks: vec!["cra".to_string()], ..Default::default() },
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root: root.to_path_buf(), config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let policies = PolicyStore::open(db).unwrap();
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()).with_policy_store(policies))
    }

    #[tokio::test]
    async fn policy_attest_with_clock_refuses_the_wrong_control_an_unknown_item_and_an_excluded_one() {
        let root = std::env::temp_dir().join(format!("factory-policies-clock-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::write(
            root.join(".factory/policies/cra.yaml"),
            "framework: cra\ntitle: Cyber Resilience Act\nkind: regulation\ncontrols:\n  - id: art-14\n    title: Reporting\n    evidence:\n      - check: attestation\n",
        )
        .unwrap();

        let now = Utc::now();
        let sbom = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","metadata":{"lifecycles":[{"phase":"build"}]},"components":[{"type":"library","name":"demo-lib","version":"1.2.3","bom-ref":"pkg:cargo/demo-lib@1.2.3"}]}"#;
        let sighting = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[{"id":"CVE-2026-8888","affects":[{"ref":"pkg:cargo/demo-lib@1.2.3"}],"properties":[{"name":"factory:kev","value":"true"}]}]}"#;
        let excluded = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[{"id":"CVE-2026-8888","affects":[{"ref":"pkg:cargo/demo-lib@1.2.3"}],"properties":[{"name":"factory:kev","value":"true"}],"analysis":{"state":"not_affected"}}]}"#;
        write_dep_doc(&root, "demo", "r1", AttachmentKind::Sbom, sbom, now - chrono::Duration::hours(2), "s1");
        write_dep_doc(&root, "demo", "r1", AttachmentKind::Vulnerabilities, sighting, now - chrono::Duration::hours(2), "v1");
        write_dep_doc(&root, "demo", "r2", AttachmentKind::Sbom, sbom, now - chrono::Duration::minutes(5), "s2");
        write_dep_doc(&root, "demo", "r2", AttachmentKind::Vulnerabilities, excluded, now - chrono::Duration::minutes(5), "v2");

        let db = root.join("policies.sqlite");
        let engine = clock_engine(&root, &db);
        let owner = Caller::Owner;
        let item = reporting_clock::ClockItemRef::Finding { scope: "demo".into(), vulnerability: "CVE-2026-8888".into() };
        let mark = |item: reporting_clock::ClockItemRef| ClockMark { item, deadline: reporting_clock::ClockDeadlineKind::EarlyWarning };

        let err = engine
            .policy_attest(
                &owner,
                "cra/other".parse().unwrap(),
                "demo".to_string(),
                "https://example.com/notice".to_string(),
                None,
                now + chrono::Duration::weeks(520),
                Some(mark(item.clone())),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("cra/art-14"), "{err}");

        let unknown = reporting_clock::ClockItemRef::Finding { scope: "demo".into(), vulnerability: "CVE-0000-0000".into() };
        let err = engine
            .policy_attest(
                &owner,
                "cra/art-14".parse().unwrap(),
                "demo".to_string(),
                "https://example.com/notice".to_string(),
                None,
                now + chrono::Duration::weeks(520),
                Some(mark(unknown)),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not a reporting-clock item"), "{err}");

        // CVE-2026-8888's newest built document reports it `not_affected` --
        // excluded, so there is nothing left to attest.
        let err = engine
            .policy_attest(
                &owner,
                "cra/art-14".parse().unwrap(),
                "demo".to_string(),
                "https://example.com/notice".to_string(),
                None,
                now + chrono::Duration::weeks(520),
                Some(mark(item)),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("excluded"), "{err}");
    }

    #[tokio::test]
    async fn a_valid_clock_submission_flips_the_deadline_to_met_refuses_a_duplicate_and_survives_a_restart() {
        let root = std::env::temp_dir().join(format!("factory-policies-clock-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::write(
            root.join(".factory/policies/cra.yaml"),
            "framework: cra\ntitle: Cyber Resilience Act\nkind: regulation\ncontrols:\n  - id: art-14\n    title: Reporting\n    evidence:\n      - check: attestation\n",
        )
        .unwrap();

        let now = Utc::now();
        let sbom = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","metadata":{"lifecycles":[{"phase":"build"}]},"components":[{"type":"library","name":"demo-lib","version":"1.2.3","bom-ref":"pkg:cargo/demo-lib@1.2.3"}]}"#;
        let vulns = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[{"id":"CVE-2026-9999","affects":[{"ref":"pkg:cargo/demo-lib@1.2.3"}],"properties":[{"name":"factory:kev","value":"true"}]}]}"#;
        write_dep_doc(&root, "demo", "r1", AttachmentKind::Sbom, sbom, now - chrono::Duration::hours(1), "s1");
        write_dep_doc(&root, "demo", "r1", AttachmentKind::Vulnerabilities, vulns, now - chrono::Duration::hours(1), "v1");

        let db = root.join("policies.sqlite");
        let engine = clock_engine(&root, &db);
        let owner = Caller::Owner;
        let item = reporting_clock::ClockItemRef::Finding { scope: "demo".into(), vulnerability: "CVE-2026-9999".into() };

        let attestation = engine
            .policy_attest(
                &owner,
                "cra/art-14".parse().unwrap(),
                "demo".to_string(),
                "https://example.com/notice".to_string(),
                None,
                now + chrono::Duration::weeks(520),
                Some(ClockMark { item: item.clone(), deadline: reporting_clock::ClockDeadlineKind::EarlyWarning }),
                None,
            )
            .await
            .unwrap();
        assert_eq!(attestation.clock.as_ref().unwrap().item, item);

        let clock = engine.policy_clock(Some("demo")).await.unwrap();
        let found = clock.items.iter().find(|i| i.item == item).unwrap();
        assert_eq!(found.deadlines[0].state, ClockDeadlineState::Met);

        // A second submission against the same (item, deadline) is refused
        // while the first one is still live.
        let err = engine
            .policy_attest(
                &owner,
                "cra/art-14".parse().unwrap(),
                "demo".to_string(),
                "https://example.com/again".to_string(),
                None,
                now + chrono::Duration::weeks(520),
                Some(ClockMark { item: item.clone(), deadline: reporting_clock::ClockDeadlineKind::EarlyWarning }),
                None,
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already has a live submission"), "{err}");

        // A fresh `Engine` over the same `root` and the same `PolicyStore`
        // file -- what a restart looks like -- still reads `met`.
        let restarted = clock_engine(&root, &db);
        let clock = restarted.policy_clock(Some("demo")).await.unwrap();
        let found = clock.items.iter().find(|i| i.item == item).unwrap();
        assert_eq!(found.deadlines[0].state, ClockDeadlineState::Met);
    }

    #[tokio::test]
    async fn corrective_measure_requires_valid_evidence_and_survives_restart_with_final_report() {
        use reporting_clock::{ClockDeadlineKind, ClockItemRef, CorrectiveMeasureMark};
        let root = std::env::temp_dir().join(format!("factory-clock-measure-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::write(root.join(".factory/policies/cra.yaml"),
            "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - id: art-14\n    title: Reporting\n    evidence:\n      - check: attestation\n").unwrap();
        let now = Utc::now();
        let sbom = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","metadata":{"lifecycles":[{"phase":"build"}]},"components":[{"type":"library","name":"demo-lib","version":"1.2.3","bom-ref":"pkg:cargo/demo-lib@1.2.3"}]}"#;
        let vulns = br#"{"bomFormat":"CycloneDX","specVersion":"1.6","vulnerabilities":[{"id":"CVE-2026-9999","affects":[{"ref":"pkg:cargo/demo-lib@1.2.3"}],"properties":[{"name":"factory:kev","value":"true"}]}]}"#;
        write_dep_doc(&root, "demo", "r1", AttachmentKind::Sbom, sbom, now - chrono::Duration::days(10), "s1");
        write_dep_doc(&root, "demo", "r1", AttachmentKind::Vulnerabilities, vulns, now - chrono::Duration::days(10), "v1");
        let db = root.join("policies.sqlite");
        let engine = clock_engine(&root, &db);
        let owner = Caller::Owner;
        let item = ClockItemRef::Finding { scope: "demo".into(), vulnerability: "CVE-2026-9999".into() };
        let control = reporting_clock::art_14();
        let available = now - chrono::Duration::days(2);
        let mark = CorrectiveMeasureMark { item: item.clone(), available_at: available };
        let expiry = now + chrono::Duration::weeks(520);
        let final_mark = ClockMark { item: item.clone(), deadline: ClockDeadlineKind::FinalReport };
        let error = engine.policy_attest(&owner, control.clone(), "demo".into(), "notice".into(), None, expiry, Some(final_mark.clone()), None).await.unwrap_err();
        assert!(error.to_string().contains("before submitting the final report"), "{error}");
        for (bad, expected) in [
            (CorrectiveMeasureMark { available_at: now + chrono::Duration::days(1), ..mark.clone() }, "future"),
            (CorrectiveMeasureMark { item: ClockItemRef::Report { item: "unknown".into() }, ..mark.clone() }, "not a reporting-clock item"),
        ] {
            let error = engine.policy_attest(&owner, control.clone(), "demo".into(), "fix".into(), None, expiry, None, Some(bad)).await.unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
        let error = engine.policy_attest(&owner, control.clone(), "demo".into(), "".into(), None, expiry, None, Some(mark.clone())).await.unwrap_err();
        assert!(error.to_string().contains("evidence must not be empty"));
        let error = engine.policy_attest(&owner, control.clone(), "demo".into(), "fix".into(), None, expiry, Some(final_mark.clone()), Some(mark.clone())).await.unwrap_err();
        assert!(error.to_string().contains("not both"));
        let anchor = engine.policy_attest(&owner, control.clone(), "demo".into(), "https://example.com/fix".into(), None, expiry, None, Some(mark.clone())).await.unwrap();
        assert!(anchor.clock.is_none());
        let error = engine.policy_attest(&owner, control.clone(), "demo".into(), "fix".into(), None, expiry, None, Some(mark.clone())).await.unwrap_err();
        assert!(error.to_string().contains("already has a live corrective-measure"));
        let clock = engine.policy_clock(Some("demo")).await.unwrap();
        assert_eq!(clock.items[0].deadlines[2].due_at, available + chrono::Duration::days(14));
        let detail = engine.policy_control(control.clone(), "demo").await.unwrap();
        assert_eq!(detail.status.kind(), policy::StatusKind::Open, "measure is not whole-control compliance");
        let submission = engine.policy_attest(&owner, control.clone(), "demo".into(), "notice".into(), None, expiry, Some(final_mark), None).await.unwrap();
        let restarted = clock_engine(&root, &db);
        let clock = restarted.policy_clock(Some("demo")).await.unwrap();
        assert_eq!(clock.items[0].corrective_measure.as_ref().unwrap().attestation, anchor.id);
        assert_eq!(clock.items[0].deadlines[2].state, ClockDeadlineState::Met);
        assert_eq!(clock.items[0].deadlines[2].submission.as_ref().unwrap().attestation, submission.id);
        restarted.policy_withdraw(&owner, anchor.id, Some("incorrect evidence".into())).await.unwrap();
        let clock = restarted.policy_clock(Some("demo")).await.unwrap();
        assert_eq!(clock.items[0].deadlines.len(), 2);
        assert!(clock.items[0].corrective_measure.is_none());
        restarted.policy_attest(&owner, control, "demo".into(), "replacement evidence".into(), None, expiry, None, Some(mark)).await.unwrap();
        assert_eq!(restarted.policy_clock(Some("demo")).await.unwrap().items[0].deadlines[2].state, ClockDeadlineState::Met);
    }

    // -- attested (#158) ------------------------------------------------

    #[tokio::test]
    async fn needs_attested_facts_is_false_for_a_catalogue_with_no_attested_check() {
        let engine = test_engine();
        let (catalogues, _findings, _tags) = engine.load_catalogues_and_tags().await.unwrap();
        let chain = engine.policy_chain("company");
        let (applied, _findings) = policy::applicable(&catalogues, &chain);
        assert!(
            !needs_attested_facts(&applied),
            "cra.yaml here names no attested check"
        );
    }

    /// A single scope, one control with an `attested` check (`category:
    /// feature`, `step: tests`, `max_age: 7d`) -- everything else this
    /// module's evidence gathering could name is left out, so the only
    /// thing that can move the control off `open` is the run this test
    /// builds directly through the store.
    fn attested_test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!(
            "factory-policies-attested-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join(".factory/policies")).unwrap();
        std::fs::write(
            root.join(".factory/policies/house.yaml"),
            "framework: house\n\
             title: House rules\n\
             kind: best-practice\n\
             controls:\n\
             \x20\x20- id: tested\n\x20\x20\x20\x20title: Feature work passes tests\n\x20\x20\x20\x20evidence:\n\
             \x20\x20\x20\x20\x20\x20- check: attested\n\x20\x20\x20\x20\x20\x20\x20\x20category: feature\n\x20\x20\x20\x20\x20\x20\x20\x20step: tests\n\x20\x20\x20\x20\x20\x20\x20\x20max_age: 7d\n",
        )
        .unwrap();

        let config = Config {
            version: 1,
            instance: Instance {
                id: "test".into(),
                name: "test".into(),
            },
            daemon: DaemonConfig::default(),
            scope: None,
            scopes: vec![scope_at("demo-id", "demo", ".", "")],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration {
                frameworks: vec!["house".to_string()],
                ..Default::default()
            },
            quality: Default::default(),
            infrastructure: Default::default(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = Factory { root, config };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(
            factory,
            registry,
            store,
            PathBuf::from("factory"),
            Vec::new(),
        ))
    }

    /// A finished `feature`-category run in `scope`, held to one gate step
    /// (`tests`), attested by `GATE_ACTOR` with `verdict`, both dated
    /// `ended_at` -- built directly through the store and `PolicyStore`,
    /// bypassing dispatch and verification entirely: this module's own
    /// evidence gathering is what is under test, not the verifier.
    async fn attested_feature_run(
        engine: &Arc<Engine>,
        scope: &str,
        ended_at: DateTime<Utc>,
        verdict: AttestationVerdict,
    ) -> factory_core::run::Run {
        let new = NewTask {
            title: "add the thing".into(),
            instructions: "true".into(),
            scope: Some(scope.to_string()),
            agent: Some("shell".into()),
            runtime: Some("quiet".into()),
            worktree: Some(false),
            category: Some("feature".into()),
            ..Default::default()
        };
        let task = task_from_new(new, scope.to_string(), "shell".into(), "quiet".into());
        let task = engine.store.create(&task).await.unwrap();
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "shell".into(),
                adapter: "shell".into(),
                runtime: "quiet".into(),
                token: "tok".into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        let required = vec![RequiredStep {
            step: "tests".into(),
            kind: StepKind::Gate,
            command: Some("true".into()),
            timeout_seconds: None,
            required_by: Vec::new(),
            node_id: None,
            actor: None,
            by: None,
        }];
        let run = engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    status: Some(RunStatus::Done),
                    ended_at: Some(ended_at),
                    required_steps: Some(required),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let attestation = StepAttestation {
            id: uuid::Uuid::new_v4().to_string(),
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            scope: scope.to_string(),
            category: "feature".to_string(),
            step: "tests".to_string(),
            kind: StepKind::Gate,
            actor: GATE_ACTOR.to_string(),
            verdict,
            required_by: Vec::new(),
            command: Some("true".to_string()),
            exit_code: Some(if verdict == AttestationVerdict::Pass {
                0
            } else {
                1
            }),
            output: None,
            dir: "/tmp".to_string(),
            commit: None,
            dirty: None,
            node_id: None,
            at: ended_at,
            findings: None,
            round: 0,
            worktree_digest: None,
        };
        engine
            .policies
            .append_step_attestation(&attestation)
            .await
            .unwrap();
        run
    }

    #[tokio::test]
    async fn policy_report_and_policy_control_agree_on_an_attested_control_through_satisfied_stale_and_open(
    ) {
        let engine = attested_test_engine();
        let control = ControlRef::new("house", "tested");
        let now = Utc::now();

        // A run an hour old: well within the 7d `max_age` -- satisfied.
        let run = attested_feature_run(
            &engine,
            "demo",
            now - chrono::Duration::hours(1),
            AttestationVerdict::Pass,
        )
        .await;
        let report = engine.policy_report(Some("demo")).await.unwrap();
        assert_eq!(
            report.rows[0].statuses[0].status.kind(),
            StatusKind::Satisfied
        );
        let detail = engine
            .policy_control(control.clone(), "demo")
            .await
            .unwrap();
        assert_eq!(detail.status.kind(), StatusKind::Satisfied);

        // Moved back through a `RunPatch` to 10 days: outside 7d, inside
        // 2*7d -- stale, not open.
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    ended_at: Some(now - chrono::Duration::days(10)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let report = engine.policy_report(Some("demo")).await.unwrap();
        assert_eq!(report.rows[0].statuses[0].status.kind(), StatusKind::Stale);
        let detail = engine
            .policy_control(control.clone(), "demo")
            .await
            .unwrap();
        assert_eq!(detail.status.kind(), StatusKind::Stale);

        // Moved back further, past 2*7d -- open.
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch {
                    ended_at: Some(now - chrono::Duration::days(20)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let report = engine.policy_report(Some("demo")).await.unwrap();
        assert_eq!(report.rows[0].statuses[0].status.kind(), StatusKind::Open);
        let detail = engine.policy_control(control, "demo").await.unwrap();
        assert_eq!(detail.status.kind(), StatusKind::Open);
    }

    #[tokio::test]
    async fn policy_report_and_policy_control_agree_that_a_failed_gate_stays_open() {
        let engine = attested_test_engine();
        let control = ControlRef::new("house", "tested");
        let now = Utc::now();
        attested_feature_run(
            &engine,
            "demo",
            now - chrono::Duration::hours(1),
            AttestationVerdict::Fail,
        )
        .await;

        let report = engine.policy_report(Some("demo")).await.unwrap();
        assert_eq!(report.rows[0].statuses[0].status.kind(), StatusKind::Open);
        let detail = engine.policy_control(control, "demo").await.unwrap();
        assert_eq!(detail.status.kind(), StatusKind::Open);
    }
}
