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
use factory_core::config::{ForemanConfig, Scope};
use factory_core::error::{FactoryError, Result};
use factory_core::policy::{self, Attestation, ControlRef, Withdrawal};
use factory_core::protocol::{CatalogueSummary, CredentialRow, NotApplicableEntry, PolicyControlDetail, PolicyReport, ScopePolicy};
use factory_core::role::Roles;
use factory_core::task::{Task, TaskFilter};
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

impl Engine {
    /// Every catalogue on disk, and the knowledge vault's tags -- the two
    /// blocking filesystem walks every policy request needs, done together
    /// in one `spawn_blocking` rather than one each.
    async fn load_catalogues_and_tags(
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

    /// The L6 Policy tab: `Request::Policy`.
    pub(crate) async fn policy_report(&self, scope: Option<&str>) -> Result<PolicyReport> {
        let snapshot = self.factory_snapshot();
        let (catalogues, mut findings, tags) = self.load_catalogues_and_tags().await?;

        let asked = scope.map(|name| snapshot.scope(name)).transpose()?.cloned();
        let target_scopes: Vec<factory_core::config::Scope> = match &asked {
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

        let all_attestations = self.policies.all().await?;
        let now = Utc::now();

        let mut rows = Vec::new();
        let mut per_scope_statuses: Vec<Vec<policy::ControlStatus>> = Vec::new();
        let mut not_applicable: BTreeSet<(ControlRef, String, String)> = BTreeSet::new();

        // First pass: resolve applicability for every scope, and collect
        // every dataset a `gate` check anywhere in this report names.
        // Datasets have no scope of their own, so `gate_facts_for` is called
        // once below over the union, rather than once per scope -- the same
        // dataset's bench runs would otherwise be walked again for every
        // scope that happens to name it.
        let mut per_scope_applied: Vec<(&factory_core::config::Scope, Vec<policy::Applied>)> = Vec::new();
        let mut dataset_names: BTreeSet<String> = BTreeSet::new();

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

            // A scope whose whole chain applies no framework has nothing to
            // show -- omitted rather than an empty row nobody asked to see.
            if applied.is_empty() {
                continue;
            }

            dataset_names.extend(gate_dataset_names(&applied));
            per_scope_applied.push((t, applied));
        }

        let gates = self.gate_facts_for(&dataset_names).await?;

        // `daemon` is the same fact set for every scope, so it is resolved
        // once, only when some scope's catalogue actually asks a `daemon`
        // question. `secrets`' credential inventory is the one part of this
        // that touches the filesystem, so it gets the same "only if named"
        // treatment `gate_facts_for`'s own dataset union already follows.
        let daemon_fact = per_scope_applied
            .iter()
            .any(|(_, applied)| needs_daemon_facts(applied))
            .then(|| self.daemon_facts());
        let credential_rows = if per_scope_applied.iter().any(|(_, applied)| needs_secrets_facts(applied)) {
            self.credential_inventory().await
        } else {
            Vec::new()
        };

        // Second pass: task/workflow resolution is still per scope (they do
        // have one), but every scope's `Evidence` shares the `gates`/`daemon`
        // facts resolved above, and `agents`/`secrets` -- resolved per scope,
        // since a scope's own roster and its own `.env` are its own -- only
        // when that scope's applicable checks actually ask for them.
        for (t, applied) in &per_scope_applied {
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
                secrets_fact_map(&credential_rows, &t.name)
            } else {
                BTreeMap::new()
            };
            let evidence = policy::Evidence {
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
            };
            findings.extend(policy::evidence_findings(&evidence, &t.name));

            let statuses = policy::evaluate(applied, &evidence, now);
            let scope_rollup = policy::rollup(&statuses);
            per_scope_statuses.push(statuses.clone());
            rows.push(ScopePolicy {
                scope: t.name.clone(),
                statuses,
                rollup: scope_rollup,
            });
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
        let evidence = policy::Evidence {
            tags,
            attestations: history.clone(),
            tasks,
            workflows,
            gates,
            agents,
            secrets,
            daemon,
        };
        let evaluated = policy::evaluate(&applied, &evidence, Utc::now())
            .into_iter()
            .find(|s| s.control == control)
            .ok_or_else(|| FactoryError::Other(anyhow::anyhow!("{control} evaluated to no status")))?;

        Ok(PolicyControlDetail {
            control: found.control.clone(),
            title: found.title.clone(),
            kind: found.kind,
            checks: found.evidence.clone(),
            maps_to: found.maps_to.clone(),
            max_age: found.max_age,
            not_applicable: found.not_applicable.clone(),
            refs: evaluated.refs,
            status: evaluated.status,
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
}

/// `attested_by`/`Withdrawal.by`: the owner speaks as `"owner"`, an agent as
/// its own name -- not `scope/name`, since an attestation's `scope` field
/// already says where it was recorded for.
fn caller_name(caller: &Caller) -> String {
    match caller {
        Caller::Owner => "owner".to_string(),
        Caller::Agent { name, .. } => name.clone(),
    }
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
            policies: PolicyDeclaration {
                frameworks: vec!["cra".to_string()],
                ..Default::default()
            },
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
            policies: PolicyDeclaration { frameworks: vec!["house".to_string()], ..Default::default() },
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
}
