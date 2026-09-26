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

mod store;
pub use store::PolicyStore;

use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use factory_core::config::{Factory, ForemanConfig, Scope};
use factory_core::error::{FactoryError, Result};
use factory_core::policy::{self, Attestation, ControlRef, Withdrawal};
use factory_core::policy_export;
use factory_core::protocol::{CatalogueSummary, CredentialRow, NotApplicableEntry, PolicyControlDetail, PolicyReport, ScopePolicy};
use factory_core::role::Roles;
use factory_core::task::{NewTask, Task, TaskFilter};
use factory_core::workflow::WorkflowDefinition;

use crate::access::Caller;
use crate::engine::Engine;

/// How many of a task's or workflow's most recent runs `task_fact`/
/// `workflow_fact` fetch, newest first, so `evaluate` can skip past any
/// still in progress to the newest one that actually finished (`policy::
/// TaskFact`/`WorkflowFact`'s own doc comments). A task or workflow
/// ordinarily has at most one run in flight at a time, so this only has to
/// cover that plus headroom for the unusual case -- not the whole history,
/// which `TaskStore::runs`/`WorkflowStore::runs` would otherwise have to
/// load in full.
const RUN_LOOKBACK: u32 = 20;

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

fn needs_dependencies_facts(applied: &[policy::Applied]) -> bool {
    applied.iter().flat_map(|a| &a.evidence)
        .any(|check| matches!(check, policy::Check::Dependencies { .. }))
}

/// Every agent Factory would actually dispatch in `scope` --
/// `Scope::agents_with`, which folds in a synthesised foreman when
/// `daemon.foreman` covers this scope. That is a deliberate choice, not an
/// oversight of the issue's literal `Scope::declared_agents`: a synthesised
/// foreman is a real agent Factory starts and hands work to, and it is
/// hard-coded `Sandbox::None` (`ScopeAgent`'s own doc comment on
/// `Scope::agents_with`), so a scope that turns the foreman on without
/// giving it a sandbox of its own now shows up in a `sandbox` check instead
/// of being silently exempt because it was never "declared". See the
/// README's "Policies" section.
///
/// Unlike `resolve_task_and_workflow_facts`, nothing here touches a store --
/// `roles` is `Engine::roles_for`'s live, in-memory read of the config
/// snapshot -- so this is a plain sync function, not spawned or awaited.
fn agent_facts_for(scope: &Scope, foreman: &ForemanConfig, roles: &Roles) -> Vec<policy::AgentFact> {
    scope
        .agents_with(foreman)
        .into_iter()
        .map(|agent| {
            let grants = roles.get(&agent.role).map(|def| def.grants.clone());
            policy::AgentFact {
                name: agent.name(),
                role: agent.role.as_str().to_string(),
                grants,
                has_sandbox: !agent.sandbox.is_none(),
            }
        })
        .collect()
}

/// The `secrets` fact for one scope, out of the L2 Secrets tab's own
/// inventory (`Engine::credential_inventory`): the five machine-wide
/// locations (an agent runs as the daemon's owner, so these are identical
/// for every scope) plus `scope`'s own `.env`, mapped to the location ids
/// [`policy::KNOWN_SECRETS_LOCATIONS`] names. `rows` is fetched once per
/// report (`policy_report`/`policy_control` each call `credential_inventory`
/// at most once), not once per scope -- it already walks every scope's
/// `.env` in one pass.
fn secrets_fact_map(rows: &[CredentialRow], scope: &str) -> BTreeMap<String, bool> {
    let mut map = BTreeMap::new();
    for row in rows {
        let id = match (row.integration.as_str(), row.scope.as_deref()) {
            ("anthropic", None) => "anthropic",
            ("github", None) => "github",
            ("aws", None) => "aws",
            ("netrc", None) => "netrc",
            ("ssh", None) => "ssh",
            ("scope env", Some(s)) if s == scope => "scope_env",
            _ => continue,
        };
        map.insert(id.to_string(), row.present);
    }
    map
}

/// Every scope in `snapshot.config.scopes` that is `scope` itself or a
/// descendant of it (`Config::ancestors_of`), or every configured scope when
/// `scope` is `None` -- the "roll up the subtree" resolution `policy_report`
/// and `scenarios::Engine::scenarios_report` both need, extracted here since
/// `policy_report` was its first caller but no longer its only one. A free
/// function, not an `Engine` method: it is a pure read of an already-cloned
/// `Factory` snapshot, nothing a caller could not do itself, just done once
/// rather than twice. Refuses an unknown scope name the same way
/// `Factory::scope` itself does.
pub(crate) fn subtree_scopes(snapshot: &Factory, scope: Option<&str>) -> Result<(Option<Scope>, Vec<Scope>)> {
    let asked = scope.map(|name| snapshot.scope(name)).transpose()?.cloned();
    let target_scopes: Vec<Scope> = match &asked {
        Some(asked) => snapshot
            .config
            .scopes
            .iter()
            .filter(|s| {
                s.name == asked.name
                    || snapshot
                        .config
                        .ancestors_of(s)
                        .iter()
                        .any(|ancestor| ancestor.name == asked.name)
            })
            .cloned()
            .collect(),
        None => snapshot.config.scopes.clone(),
    };
    Ok((asked, target_scopes))
}

impl Engine {
    /// Every catalogue on disk, and the knowledge vault's tags -- the two
    /// blocking filesystem walks every policy request needs, done together
    /// in one `spawn_blocking` rather than one each.
    pub(crate) async fn load_catalogues_and_tags(
        &self,
    ) -> Result<(Vec<policy::Catalogue>, Vec<policy::Finding>, BTreeSet<String>)> {
        let snapshot = self.factory_snapshot();
        let policies_dir = snapshot.policies_dir();
        let root = snapshot.root.clone();
        tokio::task::spawn_blocking(move || {
            let (catalogues, findings) = policy::load_all(&policies_dir);
            let index = factory_core::knowledge::index(&root);
            let tags: BTreeSet<String> = index.tags.into_iter().map(|t| t.name).collect();
            (catalogues, findings, tags)
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("policy catalogue/knowledge walk: {e}")))
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

        let mut tasks = BTreeMap::new();
        if !task_names.is_empty() {
            let scoped = self
                .store
                .list(&TaskFilter {
                    scope: Some(scope.to_string()),
                    ..Default::default()
                })
                .await?;
            for name in task_names {
                tasks.insert(name.to_string(), self.task_facts_for(&scoped, name).await?);
            }
        }

        let mut workflows = BTreeMap::new();
        if !workflow_names.is_empty() {
            let defs = self.workflows.definitions(Some(scope)).await?;
            for name in workflow_names {
                workflows.insert(name.to_string(), self.workflow_facts_for(&defs, scope, name).await?);
            }
        }

        Ok((tasks, workflows))
    }

    /// The `gate` fact for every dataset name in `names` that has ever had a
    /// settled bench run -- one lookup per name, however many scopes end up
    /// sharing the result. Datasets have no scope of their own, so a caller
    /// evaluating several scopes in one report calls this once, over the
    /// union of every scope's `gate` checks, rather than once per scope.
    async fn gate_facts_for(&self, names: &BTreeSet<String>) -> Result<BTreeMap<String, policy::GateFact>> {
        let mut gates = BTreeMap::new();
        for name in names {
            if let Some(fact) = self.gate_fact_for(name).await? {
                gates.insert(name.clone(), fact);
            }
        }
        Ok(gates)
    }

    /// [`policy::KNOWN_DAEMON_FACTS`]'s whole vocabulary, read off the live
    /// config snapshot -- the same for every scope a report evaluates
    /// (`Evidence::gates`' own reasoning), so this is resolved once, not per
    /// scope, and it is a plain sync read: no store, no filesystem walk,
    /// just the config `Engine::infrastructure` already reads for
    /// `DaemonFacts.interfaces`.
    fn daemon_facts(&self) -> policy::DaemonFact {
        let snapshot = self.factory_snapshot();
        let daemon_config = &snapshot.config.daemon;

        let http_binds: Vec<String> = daemon_config
            .interfaces
            .iter()
            .filter(|i| i.kind == "http")
            .map(|i| {
                i.string("bind")
                    .unwrap_or_else(|| crate::interfaces::http::DEFAULT_BIND.to_string())
            })
            .collect();
        // No `http` interface at all is vacuously loopback-only -- nothing
        // is exposed beyond loopback either way. A `bind` that does not
        // parse as a socket address (a bare hostname, say) is left `None`:
        // this module makes no DNS lookup and no guess about what a name
        // resolves to.
        let http_loopback_only = if http_binds.is_empty() {
            Some(true)
        } else {
            http_binds
                .iter()
                .map(|bind| bind.parse::<std::net::SocketAddr>().map(|addr| addr.ip().is_loopback()))
                .collect::<std::result::Result<Vec<bool>, _>>()
                .ok()
                .map(|loopback| loopback.iter().all(|l| *l))
        };

        policy::DaemonFact {
            foreman_enabled: daemon_config.foreman.enabled,
            http_loopback_only,
            power_assertion: daemon_config.power_assertion,
        }
    }

    /// Every task in `scoped` (the evaluated scope's own tasks) that `name`
    /// -- a `task` check's own string -- could mean: by id first (unique, so
    /// at most one match), and otherwise by exact title, which may match
    /// more than one. `evaluate` treats more than one match as an ambiguous
    /// name (`TaskFact`'s own doc comment), so only the sole unambiguous
    /// match's runs are worth fetching -- a lookup for every candidate of an
    /// ambiguous name would cost something nothing ever reads.
    async fn task_facts_for(&self, scoped: &[Task], name: &str) -> Result<Vec<policy::TaskFact>> {
        if let Some(task) = scoped.iter().find(|t| t.id == name) {
            return Ok(vec![self.task_fact(task).await?]);
        }
        let matches: Vec<&Task> = scoped.iter().filter(|t| t.title == name).collect();
        if let [only] = matches.as_slice() {
            return Ok(vec![self.task_fact(only).await?]);
        }
        Ok(matches
            .into_iter()
            .map(|t| policy::TaskFact {
                id: t.id.clone(),
                title: t.title.clone(),
                runs: Vec::new(),
            })
            .collect())
    }

    async fn task_fact(&self, task: &Task) -> Result<policy::TaskFact> {
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
            .map(|r| policy::RunFact {
                id: r.id,
                status: r.status,
                started_at: r.started_at,
                ended_at: r.ended_at,
            })
            .collect();
        Ok(policy::TaskFact {
            id: task.id.clone(),
            title: task.title.clone(),
            runs,
        })
    }

    /// The workflow-side twin of `task_facts_for`.
    async fn workflow_facts_for(&self, defs: &[WorkflowDefinition], scope: &str, name: &str) -> Result<Vec<policy::WorkflowFact>> {
        if let Some(def) = defs.iter().find(|d| d.id == name) {
            return Ok(vec![self.workflow_fact(def, scope).await?]);
        }
        let matches: Vec<&WorkflowDefinition> = defs.iter().filter(|d| d.name == name).collect();
        if let [only] = matches.as_slice() {
            return Ok(vec![self.workflow_fact(only, scope).await?]);
        }
        Ok(matches
            .into_iter()
            .map(|d| policy::WorkflowFact {
                id: d.id.clone(),
                name: d.name.clone(),
                runs: Vec::new(),
            })
            .collect())
    }

    /// The workflow-side twin of `task_fact` -- see `RUN_LOOKBACK`.
    async fn workflow_fact(&self, def: &WorkflowDefinition, scope: &str) -> Result<policy::WorkflowFact> {
        let runs = self
            .workflows
            .runs(Some(&def.id), Some(scope), RUN_LOOKBACK)
            .await?
            .into_iter()
            .map(|r| policy::WorkflowRunFact {
                id: r.id,
                status: r.status,
                updated_at: r.updated_at,
            })
            .collect();
        Ok(policy::WorkflowFact {
            id: def.id.clone(),
            name: def.name.clone(),
            runs,
        })
    }

    /// The newest *settled* bench run of `dataset` (`BenchRun::settled`),
    /// `None` when it has never had one -- `evaluate`'s `gate` check treats
    /// an entry missing from `Evidence::gates` the same way.
    async fn gate_fact_for(&self, dataset: &str) -> Result<Option<policy::GateFact>> {
        // `BenchStore::runs` loads every returned run's attempts eagerly, so
        // this is bounded rather than "every run this dataset ever had" --
        // the same 200 `Request::BenchRuns` already asks for, which a
        // dataset gated often enough to bury its newest settled run past
        // could still, in principle, outrun; the same edge case that bound
        // already accepts.
        let runs = self.bench.runs(Some(dataset), 200).await?;
        let Some(run) = runs.into_iter().find(|r| r.settled()) else {
            return Ok(None);
        };
        let cases = run
            .cases
            .iter()
            .map(|case| policy::GateCase {
                id: case.id.clone(),
                gated: case.gate.is_some(),
                verdicts: run
                    .attempts
                    .iter()
                    .filter(|a| a.case_id == case.id)
                    .filter_map(|a| a.verdict)
                    .collect(),
            })
            .collect();
        Ok(Some(policy::GateFact {
            run_id: run.id,
            ended_at: run.ended_at,
            cases,
        }))
    }

    /// The facts every scope in a report's subtree shares, resolved once
    /// over the whole subtree rather than once per scope: every dataset a
    /// `gate` check anywhere in `per_scope_applied` names (datasets have no
    /// scope of their own, so a dataset named by two scopes' catalogues is
    /// still only walked once), the `daemon` fact (the same for every scope,
    /// resolved only when some scope's catalogue actually asks a `daemon`
    /// question), and the credential inventory behind `secrets` (the one
    /// part of this that touches the filesystem, gated the same way).
    /// Shared by `policy_report` and `scenarios::Engine::scenarios_report`
    /// (`#100`), which calls this once for the baseline `Applied` sets and
    /// again for each scenario's own -- a scenario's overlay can name a
    /// `gate`/`daemon`/`secrets` check the baseline never did (an
    /// `add_frameworks` draft, say), and this is what picks up the extra
    /// fact lazily rather than the caller having to know in advance.
    pub(crate) async fn dataset_level_facts(
        &self,
        per_scope_applied: &[(&Scope, Vec<policy::Applied>)],
    ) -> Result<(BTreeMap<String, policy::GateFact>, Option<policy::DaemonFact>, Vec<CredentialRow>)> {
        let mut dataset_names: BTreeSet<String> = BTreeSet::new();
        for (_, applied) in per_scope_applied {
            dataset_names.extend(gate_dataset_names(applied));
        }
        let gates = self.gate_facts_for(&dataset_names).await?;
        let daemon_fact = per_scope_applied
            .iter()
            .any(|(_, applied)| needs_daemon_facts(applied))
            .then(|| self.daemon_facts());
        let credential_rows = if per_scope_applied.iter().any(|(_, applied)| needs_secrets_facts(applied)) {
            self.credential_inventory().await
        } else {
            Vec::new()
        };
        Ok((gates, daemon_fact, credential_rows))
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
        credential_rows: &[CredentialRow],
    ) -> Result<policy::Evidence> {
        let ancestor_names: BTreeSet<&str> = snapshot
            .config
            .ancestors_of(t)
            .iter()
            .map(|ancestor| ancestor.name.as_str())
            .collect();
        let (tasks, workflows) = self.resolve_task_and_workflow_facts(&t.name, applied).await?;
        let agents = needs_agent_facts(applied).then(|| {
            let roles = self.roles_for(&t.name);
            agent_facts_for(t, &snapshot.config.daemon.foreman, &roles)
        });
        let secrets = if needs_secrets_facts(applied) {
            secrets_fact_map(credential_rows, &t.name)
        } else {
            BTreeMap::new()
        };
        let dependencies = if needs_dependencies_facts(applied) {
            Some(crate::dependencies::fact(&self.dependencies_report(&t.name).await?))
        } else {
            None
        };
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
        })
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
        let (gates, daemon_fact, credential_rows) = self.dataset_level_facts(&per_scope_applied).await?;
        for (t, applied) in &per_scope_applied {
            let evidence = self
                .evidence_for_scope(&snapshot, t, applied, &tags, &all_attestations, &gates, daemon_fact, &credential_rows)
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
        let agents = needs_agent_facts(&applied).then(|| {
            let roles = self.roles_for(&scope_obj.name);
            agent_facts_for(&scope_obj, &snapshot.config.daemon.foreman, &roles)
        });
        let secrets = if needs_secrets_facts(&applied) {
            secrets_fact_map(&self.credential_inventory().await, &scope_obj.name)
        } else {
            BTreeMap::new()
        };
        let daemon = needs_daemon_facts(&applied).then(|| self.daemon_facts());
        let dependencies = if needs_dependencies_facts(&applied) {
            Some(crate::dependencies::fact(&self.dependencies_report(&scope_obj.name).await?))
        } else {
            None
        };
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
        };
        let evaluated = policy::evaluate(&applied, &evidence, Utc::now())
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
    /// control that does not apply -- or is `n/a` -- at `scope`.
    pub(crate) async fn policy_attest(
        &self,
        caller: &Caller,
        control: ControlRef,
        scope: String,
        evidence: String,
        note: Option<String>,
        expires_at: chrono::DateTime<Utc>,
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
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, PolicyDeclaration, Scope};
    use factory_core::policy::{ControlStatus, StatusKind};
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
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

    #[test]
    fn daemon_facts_reads_the_default_http_bind_as_loopback_only() {
        let engine = l123_test_engine();
        let facts = engine.daemon_facts();
        assert!(facts.foreman_enabled);
        assert_eq!(facts.http_loopback_only, Some(true), "the default bind, 127.0.0.1:8787, is loopback");
        assert!(facts.power_assertion);
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
}
