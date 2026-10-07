//! `#275`: where suggestion requests are served. L5 owns the store and the
//! state machine (`factory_assurance::suggestion`); this module resolves
//! what Factory already knows about a run into a filing, folds the live
//! config into a scope filter the same way `operations.rs`/`quality/mod.rs`
//! do, and is the one place "create improvement task" and "ask the agent"
//! reach into L4's task creation and dispatch.

use crate::access::Caller;
use crate::engine::{ContinueOutcome, Due};
use chrono::Utc;
use factory_assurance::remediation::Intent;
use factory_assurance::suggestion::{self, Filing, Suggestion, SuggestionKind, SuggestionReport, SuggestionState, UsageSnapshot};
use factory_core::adapter::agent::UpstreamOutput;
use factory_core::config::Sandbox;
use factory_core::run::{FailKind, Run, Trigger};
use factory_core::task::{Task, TaskEntry};
use factory_kernel::{FactoryError, Result};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) use factory_assurance::suggestion_store::SuggestionStore;

use crate::l5_service::L5Service;
#[cfg(test)]
use crate::engine::Engine;

impl L5Service<'_> {
    /// The scope subtree `scope` names, resolved from the live config --
    /// `None` is every scope, `Some` is that scope and its descendants
    /// (`Scope.path` ancestry, never a name prefix), the same rule
    /// `Request::Quality`/`Request::Operations` already read a scope by.
    fn suggestion_scope_set(&self, scope: Option<&str>) -> Result<Option<BTreeSet<String>>> {
        let Some(name) = scope else { return Ok(None) };
        let snapshot = self.wiring.snapshot();
        let (asked, subtree) = factory_core::config::subtree_scopes(&snapshot, Some(name))?;
        let asked = asked.ok_or_else(|| FactoryError::BadRequest(format!("no such scope: {name:?}")))?;
        let mut members: BTreeSet<String> = subtree.into_iter().map(|s| s.name).collect();
        members.insert(asked.name);
        Ok(Some(members))
    }

    /// `Request::TaskSuggest`: file a suggestion against this task's active
    /// run. Authenticated exactly like `report` -- the run token in
    /// `suggestion.token`, checked by `check_run_token` -- and needs no
    /// grant (`access::needs`): every role that can hold a run at all can
    /// use this on it. Run, task, scope, agent, harness, session and usage
    /// are read off the run itself; the agent states only `suggestion`'s
    /// own fields.
    pub(crate) async fn file_suggestion(&self, task_id: &str, report: SuggestionReport) -> Result<Suggestion> {
        let run = self.active_run(task_id).await?.ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "task {task_id} has no run in progress; a suggestion is filed against an active run"
            ))
        })?;
        self.wiring.l4().port().check_run_token(&run, report.token.as_deref(), task_id)?;
        let task = self.require_task(task_id).await?;

        let session_id = run
            .session
            .as_ref()
            .or(run.last_session.as_ref())
            .map(|s| s.handle.clone());
        let usage = run.usage.as_ref().map(|u| UsageSnapshot {
            total_tokens: u.tokens.total(),
            cost_usd: u.cost_usd,
        });
        let filing = Filing {
            run_id: run.id.clone(),
            task_id: task.id.clone(),
            scope: task.scope.clone(),
            agent: run.agent.clone(),
            harness: run.adapter.clone(),
            session_id,
            usage,
            kind: report.kind,
            target: report.target,
            summary: report.summary,
            detail: report.detail,
            wasted_tokens: report.wasted_tokens,
        };
        let suggestion = filing.file(uuid::Uuid::new_v4().to_string(), Utc::now());
        self.state.suggestions.put(&suggestion).await?;

        // `#275`'s acceptance criterion: a suggestion "appears on the run's
        // journal" -- the same task journal every other agent-reported
        // event lands on, so a person reading the run already sees it.
        self.wiring.l4().port().journal(
            task_id,
            TaskEntry::new(
                "agent",
                "suggestion",
                format!("[{}] {} -- {}", suggestion.kind, suggestion.target, suggestion.summary),
            )
            .in_run(&run.id)
            .with_data(serde_json::json!({ "suggestion_id": suggestion.id })),
        )
        .await;
        Ok(suggestion)
    }

    /// `Request::Suggestions`: every suggestion matching the filters,
    /// newest first, folded by target. Read-only, computed fresh.
    pub(crate) async fn suggestions_report(
        &self,
        scope: Option<&str>,
        kind: Option<SuggestionKind>,
        target: Option<String>,
        state: Option<SuggestionState>,
    ) -> Result<suggestion::Report> {
        let scopes = self.suggestion_scope_set(scope)?;
        let filter = suggestion::Filter { scopes, kind, target, state };
        let all = self.state.suggestions.all().await?;
        let matching: Vec<Suggestion> = filter.apply(&all).into_iter().cloned().collect();
        Ok(suggestion::report(matching))
    }

    pub(crate) async fn suggestion_get(&self, id: &str) -> Result<Suggestion> {
        self.state.suggestions
            .get(id)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no such suggestion: {id:?}")))
    }

    /// `Request::SuggestionTask`: the only way a suggestion becomes work.
    /// Creates one task through L5's existing adjacent L4 creation port
    /// (`factory_assurance::remediation::Service::remediate`, the same door
    /// `Request::QualityRemediate`/`Request::PolicyRemediate` open),
    /// addressed to the root scope's declared improvement agent, and links
    /// it back to every named suggestion. Every id must exist and be open
    /// or already tasked -- never a suggestion already dismissed or done.
    /// A group is keyed by target alone, so its suggestions routinely span
    /// more than one scope (the same blocked domain reported from two
    /// projects) -- there is no "must share a scope" restriction: the task
    /// always lands in the root regardless, `access.rs` already checks
    /// reach against each named suggestion's own scope individually, and
    /// every source scope is named in the task body instead.
    pub(crate) async fn suggestion_task(&self, caller: &Caller, ids: Vec<String>) -> Result<Task> {
        if ids.is_empty() {
            return Err(FactoryError::BadRequest("suggestion.task needs at least one suggestion id".into()));
        }
        let mut suggestions = Vec::with_capacity(ids.len());
        for id in &ids {
            suggestions.push(self.suggestion_get(id).await?);
        }
        if let Some(terminal) = suggestions.iter().find(|s| s.state.is_terminal()) {
            return Err(FactoryError::BadRequest(format!(
                "suggestion {} is already {}; nothing can tie a new task to it",
                terminal.id,
                terminal.state.as_str()
            )));
        }

        let snapshot = self.wiring.snapshot();
        let root = snapshot.config.scope.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(
                "this instance declares no root scope, so there is nowhere for an improvement task to land".into(),
            )
        })?;
        let improver = snapshot.config.daemon.improvement_agent.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(
                "no improvement agent is configured: set daemon.improvement_agent in the root scope's \
                 config.yaml to the name of a standing agent declared there"
                    .into(),
            )
        })?;
        let declared = root.agents_with(&snapshot.config.daemon.foreman);
        if !declared.iter().any(|a| a.name() == improver.as_str()) {
            return Err(FactoryError::BadRequest(format!(
                "daemon.improvement_agent names {improver:?}, which root scope {:?} does not declare as an agent",
                root.name
            )));
        }

        let mut scopes: BTreeSet<&str> = BTreeSet::new();
        for s in &suggestions {
            scopes.insert(s.scope.as_str());
        }
        let scopes: Vec<&str> = scopes.into_iter().collect();

        let title = format!("Improve: {}", suggestions[0].target);
        let mut body = format!(
            "{} run(s) flagged this as friction, escalated from {}. You are the \
             company-wide improvement agent -- this is not the complaining agent's to fix, \
             and it is yours to reach what it could not: sandbox policy, roles, the secrets \
             catalogue, docs, specs or workflows.\n",
            suggestions.len(),
            scopes.join(", "),
        );
        for s in &suggestions {
            body.push_str(&format!(
                "\n- [{}] target: {}\n  summary: {}\n  from: {}\n",
                s.kind, s.target, s.summary, s.scope,
            ));
            if let Some(detail) = &s.detail {
                body.push_str(&format!("  detail: {detail}\n"));
            }
        }
        let mut labels = BTreeMap::new();
        labels.insert("suggestion".to_string(), ids.join(","));

        let task_id = self
            .wiring
            .l4()
            .port()
            .create_remediation(Intent {
                title,
                instructions: body,
                scope: root.name.clone(),
                agent: Some(improver.clone()),
                labels,
            })
            .await?;

        let now = Utc::now();
        let by = crate::policies::caller_name(caller);
        for mut s in suggestions {
            if let Err(e) = s.task(&task_id, &by, now) {
                tracing::warn!(suggestion = s.id, "{e}");
                continue;
            }
            self.state.suggestions.put(&s).await?;
        }
        self.require_task(&task_id).await
    }

    pub(crate) async fn suggestion_dismiss(&self, caller: &Caller, id: &str, reason: String) -> Result<Suggestion> {
        let mut suggestion = self.suggestion_get(id).await?;
        suggestion
            .dismiss(reason, &crate::policies::caller_name(caller), Utc::now())
            .map_err(FactoryError::BadRequest)?;
        self.state.suggestions.put(&suggestion).await?;
        Ok(suggestion)
    }

    pub(crate) async fn suggestion_done(&self, caller: &Caller, id: &str) -> Result<Suggestion> {
        let mut suggestion = self.suggestion_get(id).await?;
        suggestion
            .done(&crate::policies::caller_name(caller), Utc::now())
            .map_err(FactoryError::BadRequest)?;
        self.state.suggestions.put(&suggestion).await?;
        Ok(suggestion)
    }

    /// `Request::SuggestionAsk` (`#275`'s "Ask the agent", building on
    /// session resumption, `#178`): starts a continuation run of the
    /// suggestion's own task that resumes its recorded session with
    /// `question`, the same way `factory task run --continue` would.
    /// Refuses outright -- never dispatching, let alone falling back to a
    /// fresh session -- when the task has a run in progress, when there is
    /// no previous terminal run, or when `resolve_continue` (the same
    /// resumability check `dispatch` itself asks, including `#274`'s
    /// preserved-sandbox check) says the harness cannot resume. `dispatch`
    /// applies a few guard conditions of its own before it ever reaches
    /// `resolve_continue` (a workflow node declaring `session: fresh`, an
    /// unknown harness version, the guide/role/declaration fingerprint
    /// changing since, or the eight-round resume cap) that this pre-check
    /// does not replicate -- an edge case here, since ask targets a task's
    /// own just-ended run rather than a drifted one. If `dispatch` still
    /// falls back to fresh for one of those, the freshly started run is
    /// cancelled rather than silently asked a question in a conversation
    /// that never saw it.
    pub(crate) async fn suggestion_ask(&self, caller: &Caller, id: &str, question: String) -> Result<Suggestion> {
        if question.trim().is_empty() {
            return Err(FactoryError::BadRequest("a question cannot be empty".into()));
        }
        let mut suggestion = self.suggestion_get(id).await?;
        let task_id = suggestion.task_id.clone();
        if self.active_run(&task_id).await?.is_some() {
            return Err(FactoryError::BadRequest(format!(
                "task {task_id} has a run in progress; ask once it ends"
            )));
        }
        let task = self.require_task(&task_id).await?;
        let Some(prev) = self.latest_run(&task_id).await? else {
            return Err(FactoryError::BadRequest(format!("task {task_id} has no previous run to ask")));
        };
        if !prev.status.is_terminal() {
            return Err(FactoryError::BadRequest(format!(
                "attempt {} of task {task_id} is still {}",
                prev.attempt,
                prev.status.as_str()
            )));
        }
        let (agent_name, adapter_name, declaration) = self.wiring.resolve_agent(&task.scope, &task.agent)?;
        let agent = self.wiring.registry().agent(&adapter_name)?;
        let runtime = self.wiring.registry().runtime(&task.runtime)?;
        let scope_path = self.wiring.snapshot().scope_path(&task.scope)?;
        // `#274`: a sandboxed run's conversation may have been preserved in
        // its OpenShell sandbox rather than deleted with it; `resolve_continue`
        // itself decides, from what was actually preserved, exactly as
        // `dispatch` asks it the same question.
        let sandboxed = declaration
            .as_ref()
            .filter(|d| d.sandbox == Sandbox::Openshell)
            .and_then(|d| d.openshell.as_ref());
        let outcome = self
            .wiring
            .l4()
            .port()
            .resolve_continue(&task, (&agent_name, &adapter_name), agent.as_ref(), runtime.as_ref(), &prev, &scope_path, sandboxed)
            .await;
        if let ContinueOutcome::Fresh { reason } = &outcome {
            return Err(FactoryError::BadRequest(format!(
                "the harness cannot resume this session, so asking is refused rather than starting a fresh \
                 one: {reason}"
            )));
        }

        // The question has nowhere else to reach the resumed prompt: it
        // rides in as an extra `UpstreamOutput`, the same channel a
        // workflow's upstream results and `#178`'s "what changed since
        // your previous run" note already use (`Engine::dispatch_with`).
        let ask_note = UpstreamOutput {
            node_id: "ask".into(),
            task_id: task.id.clone(),
            title: "A person is asking about a suggestion you filed".into(),
            result: Some(format!(
                "You filed this suggestion: [{}] {} -- {}\n\n\
                 A person is now asking you a follow-up question about it:\n\n{question}\n\n\
                 This run exists only to answer that question. Change nothing and do not \
                 continue the task's ordinary work. Report done with your answer as --result.",
                suggestion.kind, suggestion.target, suggestion.summary,
            )),
        };
        let run = Box::pin(self.wiring.l4().port().dispatch_with(&task_id, Trigger::Manual, Due::now(), Some(prev), vec![ask_note])).await?;
        if run.resumed_session.is_none() {
            // `dispatch` applies a few guards of its own beyond
            // `resolve_continue` (guide/role fingerprint drift, an unknown
            // harness version, a sandboxed run's deleted workspace, the
            // eight-round resume cap) that this pre-check does not
            // replicate. Caught here rather than never: the run is
            // cancelled before it does any work, so the question is never
            // put to a conversation that never saw it.
            self.wiring.l4().port().cancel_task_run(&task_id, Some(&run.id), FailKind::CancelledByPerson).await.ok();
            return Err(FactoryError::BadRequest(
                "the harness fell back to a fresh session instead of resuming; the attempt was cancelled \
                 rather than silently asking a new conversation"
                    .into(),
            ));
        }

        let now = Utc::now();
        suggestion.ask(question, &crate::policies::caller_name(caller), &run.id, now);
        self.state.suggestions.put(&suggestion).await?;
        Ok(suggestion)
    }

    /// Called from `finish_run` for every run that ends: if this run was
    /// started to ask a suggestion's agent a question, record its result as
    /// the answer. A no-op for every other run (`find_by_ask_run` matches
    /// nothing) and never fails the run it is settling for -- a suggestion
    /// is evidence, not part of any run's own outcome.
    pub(crate) async fn settle_suggestion_ask(&self, run: &Run) {
        let found = match self.state.suggestions.find_by_ask_run(&run.id).await {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(run = run.id, "could not look up a pending suggestion ask: {e}");
                return;
            }
        };
        let Some(mut suggestion) = found else { return };
        let answer = run
            .result
            .clone()
            .or_else(|| run.error.clone())
            .unwrap_or_else(|| format!("(the run ended {} with no result or error message)", run.status.as_str()));
        if !suggestion.answer(&run.id, answer, Utc::now()) {
            return;
        }
        if let Err(e) = self.state.suggestions.put(&suggestion).await {
            tracing::warn!(suggestion = suggestion.id, run = run.id, "could not record a suggestion's answer: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use super::*;
    use crate::access::Caller;
    use factory_core::agent::Lifetime;
    use factory_core::config::{Config, DaemonConfig, Instance, Sandbox, Scope, ScopeAgent};
    use factory_core::protocol::{Envelope, Payload, Request, Response};
    use factory_core::role::Role;
    use factory_core::run::NewRun;
    use factory_core::task::NewTask;
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    /// One project scope (`demo`) and, when `root` is given, a second,
    /// separate root scope -- enough to exercise filing, scope-subtree
    /// filtering and improvement-task addressing without a real herdr or a
    /// real agent. Mirrors `engine.rs`'s own `test_engine`, which this file
    /// cannot reach (it is private to that module's tests).
    fn test_engine(improvement_agent: Option<&str>, root_declares_improver: bool) -> Arc<Engine> {
        let demo = Scope {
            id: "demo-id".into(),
            name: "demo".into(),
            path: PathBuf::from("/tmp"),
            agent: None,
            agents: vec![
                ScopeAgent {
                    name: Some("nogrant-worker".into()),
                    harness: "shell".into(),
                    lifetime: Lifetime::Temporary,
                    role: Role::new("undeclared-role"),
                    autostart: None,
                    args: Vec::new(),
                    sandbox: Sandbox::None,
                    openshell: None,
                    provider: None,
                    max_sessions: None,
                },
                ScopeAgent {
                    name: Some("plain-worker".into()),
                    harness: "shell".into(),
                    lifetime: Lifetime::Temporary,
                    role: Role::worker(),
                    autostart: None,
                    args: Vec::new(),
                    sandbox: Sandbox::None,
                    openshell: None,
                    provider: None,
                    max_sessions: None,
                },
            ],
            runtime: None,
            git: None,
            task_store: None,
            max_sessions: None,
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            intake: Default::default(),
            dependencies: Default::default(),
            environments: Vec::new(),
            renewals: Vec::new(),
            backup: None,
            metrics: None,
        };
        let root = Scope {
            id: "root-id".into(),
            name: "root".into(),
            path: PathBuf::from("/tmp"),
            agent: None,
            agents: if root_declares_improver {
                vec![ScopeAgent {
                    name: Some("improver".into()),
                    harness: "shell".into(),
                    lifetime: Lifetime::Permanent,
                    role: Role::foreman(),
                    autostart: None,
                    args: Vec::new(),
                    sandbox: Sandbox::None,
                    openshell: None,
                    provider: None,
                    max_sessions: None,
                }]
            } else {
                Vec::new()
            },
            runtime: None,
            git: None,
            task_store: None,
            max_sessions: None,
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            intake: Default::default(),
            dependencies: Default::default(),
            environments: Vec::new(),
            renewals: Vec::new(),
            backup: None,
            metrics: None,
        };
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig {
                power_assertion: false,
                improvement_agent: improvement_agent.map(str::to_string),
                ..DaemonConfig::default()
            },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: Some(root.clone()),
            scopes: vec![demo, root],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let factory = factory_core::config::Factory {
            root: std::env::temp_dir().join(format!("factory-suggestions-test-{}", uuid::Uuid::new_v4())),
            config,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn factory_core::adapter::TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        Arc::new(Engine::new(factory, registry, store, PathBuf::from("factory"), Vec::new()))
    }

    async fn task_in(engine: &Engine, scope: &str, agent: &str) -> Task {
        let task = factory_process::store::task_from_new(
            NewTask { title: "a task".into(), ..Default::default() },
            scope.into(),
            agent.into(),
            "herdr".into(),
        );
        engine.l4.store.create(&task).await.unwrap()
    }

    async fn active_run(engine: &Engine, task_id: &str, agent: &str, token: &str) -> Run {
        engine
            .l4.store
            .create_run(&NewRun {
                task_id: task_id.into(),
                trigger: Trigger::Manual,
                agent: agent.into(),
                adapter: "shell".into(),
                runtime: "herdr".into(),
                token: token.into(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap()
    }

    /// L5 reads L4's task and run records as facts (`TaskSnapshotFact`, `RunSnapshotFact`), never from L4's store.
    #[tokio::test]
    async fn l5_reads_task_and_run_records_as_facts_and_a_missing_one_is_none() {
        let engine = test_engine(None, true);
        let l5 = engine.l5_service();
        assert!(l5.task_record("nope").await.unwrap().is_none());
        assert!(l5.latest_run("nope").await.unwrap().is_none());
        assert!(l5.active_run("nope").await.unwrap().is_none());
        assert!(matches!(l5.require_task("nope").await, Err(FactoryError::TaskNotFound(_))));

        let task = task_in(&engine, "demo", "a").await;
        assert_eq!(l5.task_record(&task.id).await.unwrap().unwrap().id, task.id);
        assert!(l5.active_run(&task.id).await.unwrap().is_none(), "no run yet");

        let run = active_run(&engine, &task.id, "a", "tok").await;
        assert_eq!(l5.active_run(&task.id).await.unwrap().unwrap().id, run.id);
        assert_eq!(l5.latest_run(&task.id).await.unwrap().unwrap().id, run.id);
        let by_id = l5.run_record(factory_kernel::RunSnapshotQuery::ById(run.id.clone())).await.unwrap().unwrap();
        assert_eq!((by_id.id, by_id.token), (run.id, run.token), "the whole record comes through");
    }

    fn report(token: Option<&str>) -> SuggestionReport {
        SuggestionReport {
            kind: SuggestionKind::Capability,
            target: "L2 secret stripe_key".into(),
            summary: "a blocked web call".into(),
            detail: Some("needed outbound https to api.stripe.com".into()),
            wasted_tokens: Some(2000),
            token: token.map(str::to_string),
        }
    }

    #[tokio::test]
    async fn a_role_holding_no_grant_at_all_can_still_file_against_its_own_run() {
        let engine = test_engine(None, false);
        let task = task_in(&engine, "demo", "nogrant-worker").await;
        let run = active_run(&engine, &task.id, "nogrant-worker", "tok-1").await;

        // The envelope's own token is this exact run's -- `caller_for`
        // resolves `undeclared-role`, a role nothing in the instance
        // defines. If filing needed a grant, `authorize` would have to
        // resolve that role first and refuse "demo does not define the
        // role"; it never does, because `needs()` answers `Needs::Nothing`.
        let response = engine
            .handle(Envelope { request: Request::TaskSuggest { id: task.id.clone(), suggestion: report(Some("tok-1")) }, token: Some("tok-1".into()) })
            .await;
        let Response::Ok { data: payload, .. } = response else { panic!("expected an ok response") };
        let Payload::Suggestion { suggestion } = payload else { panic!("expected a suggestion payload") };
        assert_eq!(suggestion.state, SuggestionState::Open);
        assert_eq!(suggestion.run_id, run.id);
        assert_eq!(suggestion.task_id, task.id);
        assert_eq!(suggestion.scope, "demo");
        assert_eq!(suggestion.agent, "nogrant-worker");
        assert_eq!(suggestion.harness, "shell");
        assert_eq!(suggestion.target, "L2 secret stripe_key");

        // And it is on the run's own journal (#275's acceptance criterion).
        let entries = engine.l4.store.entries(&task.id, 10).await.unwrap();
        assert!(entries.iter().any(|e| e.kind == "suggestion" && e.run_id.as_deref() == Some(run.id.as_str())));
    }

    #[tokio::test]
    async fn the_embedded_run_token_is_checked_even_with_no_envelope_token_at_all() {
        // `handle_request` (what the HTTP `/api/tasks/{id}/suggest`
        // shortcut uses) carries no envelope token, so `check_run_token`
        // inside `file_suggestion` is the only thing standing guard.
        let engine = test_engine(None, false);
        let task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &task.id, "plain-worker", "the-real-token").await;

        let wrong = engine
            .handle_request(Request::TaskSuggest { id: task.id.clone(), suggestion: report(Some("not-it")) })
            .await;
        assert!(matches!(wrong, Response::Error { .. }));

        let right = engine
            .handle_request(Request::TaskSuggest { id: task.id.clone(), suggestion: report(Some("the-real-token")) })
            .await;
        assert!(matches!(right, Response::Ok { .. }));
    }

    #[tokio::test]
    async fn filing_against_a_task_with_no_run_in_progress_is_a_clear_refusal() {
        let engine = test_engine(None, false);
        let task = task_in(&engine, "demo", "plain-worker").await;
        let response = engine.handle_request(Request::TaskSuggest { id: task.id, suggestion: report(None) }).await;
        assert!(matches!(response, Response::Error { .. }));
    }

    #[tokio::test]
    async fn suggestions_report_narrows_by_scope_subtree_and_filters() {
        let engine = test_engine(None, false);
        let demo_task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &demo_task.id, "plain-worker", "tok-demo").await;
        engine.l5_service().file_suggestion(&demo_task.id, report(Some("tok-demo"))).await.unwrap();

        let root_task = task_in(&engine, "root", "improver").await;
        active_run(&engine, &root_task.id, "improver", "tok-root").await;
        let mut other = report(Some("tok-root"));
        other.target = "a different target".into();
        engine.l5_service().file_suggestion(&root_task.id, other).await.unwrap();

        let demo_only = engine.l5_service().suggestions_report(Some("demo"), None, None, None).await.unwrap();
        assert_eq!(demo_only.suggestions.len(), 1);
        assert_eq!(demo_only.suggestions[0].scope, "demo");

        let everything = engine.l5_service().suggestions_report(None, None, None, None).await.unwrap();
        assert_eq!(everything.suggestions.len(), 2);

        let by_target = engine
            .l5_service()
            .suggestions_report(None, None, Some("a different target".into()), None)
            .await
            .unwrap();
        assert_eq!(by_target.suggestions.len(), 1);
        assert_eq!(by_target.suggestions[0].scope, "root");
    }

    #[tokio::test]
    async fn dismiss_and_done_are_refused_once_a_suggestion_is_already_terminal() {
        let engine = test_engine(None, false);
        let task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &task.id, "plain-worker", "tok-1").await;
        let s = engine.l5_service().file_suggestion(&task.id, report(Some("tok-1"))).await.unwrap();

        let dismissed = engine.l5_service().suggestion_dismiss(&Caller::Owner, &s.id, "not worth it".into()).await.unwrap();
        assert_eq!(dismissed.state, SuggestionState::Dismissed);
        assert!(engine.l5_service().suggestion_dismiss(&Caller::Owner, &s.id, "again".into()).await.is_err());
        assert!(engine.l5_service().suggestion_done(&Caller::Owner, &s.id).await.is_err());
    }

    #[tokio::test]
    async fn suggestion_task_refuses_clearly_without_an_improvement_agent_configured() {
        let engine = test_engine(None, false);
        let task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &task.id, "plain-worker", "tok-1").await;
        let s = engine.l5_service().file_suggestion(&task.id, report(Some("tok-1"))).await.unwrap();

        let err = engine.l5_service().suggestion_task(&Caller::Owner, vec![s.id]).await.unwrap_err().to_string();
        assert!(err.contains("improvement_agent"), "{err}");
    }

    #[tokio::test]
    async fn suggestion_task_refuses_clearly_when_the_configured_agent_is_not_declared_in_root() {
        let engine = test_engine(Some("improver"), false);
        let task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &task.id, "plain-worker", "tok-1").await;
        let s = engine.l5_service().file_suggestion(&task.id, report(Some("tok-1"))).await.unwrap();

        let err = engine.l5_service().suggestion_task(&Caller::Owner, vec![s.id]).await.unwrap_err().to_string();
        assert!(err.contains("improver"), "{err}");
    }

    #[tokio::test]
    async fn suggestion_task_lands_in_the_root_scope_addressed_to_the_improver_never_the_complaining_agent() {
        let engine = test_engine(Some("improver"), true);
        let task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &task.id, "plain-worker", "tok-1").await;
        let s = engine.l5_service().file_suggestion(&task.id, report(Some("tok-1"))).await.unwrap();

        // A non-owner caller, to prove the history names whoever actually
        // pressed the button (`#275` QA) rather than a hardcoded "owner".
        let foreman = Caller::Agent { scope: "demo".into(), name: "boss".into(), role: Role::foreman(), run_id: None };
        let created = engine.l5_service().suggestion_task(&foreman, vec![s.id.clone()]).await.unwrap();
        assert_eq!(created.scope, "root", "the task lands in the root scope, not demo");
        assert_eq!(created.agent, "improver", "addressed to the improver, never the complaining project agent");
        assert_eq!(created.labels.get("suggestion").map(String::as_str), Some(s.id.as_str()));

        let updated = engine.l5_service().suggestion_get(&s.id).await.unwrap();
        assert_eq!(updated.state, SuggestionState::Tasked);
        assert_eq!(updated.improvement_task_id.as_deref(), Some(created.id.as_str()));
        assert_eq!(updated.history.last().unwrap().by, "boss", "names whoever pressed the button, not a hardcoded owner");
    }

    #[tokio::test]
    async fn suggestion_task_accepts_a_group_spanning_more_than_one_scope_and_names_every_source() {
        let engine = test_engine(Some("improver"), true);
        let demo_task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &demo_task.id, "plain-worker", "tok-demo").await;
        let mut from_demo = report(Some("tok-demo"));
        from_demo.target = "shared target".into();
        let a = engine.l5_service().file_suggestion(&demo_task.id, from_demo).await.unwrap();

        let root_task = task_in(&engine, "root", "improver").await;
        active_run(&engine, &root_task.id, "improver", "tok-root").await;
        let mut from_root = report(Some("tok-root"));
        from_root.target = "shared target".into();
        let b = engine.l5_service().file_suggestion(&root_task.id, from_root).await.unwrap();

        // No "must share a scope" refusal: a group keyed by target alone
        // routinely spans more than one scope, and the task always lands in
        // the root regardless of where its suggestions came from.
        let created = engine.l5_service().suggestion_task(&Caller::Owner, vec![a.id.clone(), b.id.clone()]).await.unwrap();
        assert_eq!(created.scope, "root");
        assert!(created.instructions.contains("demo"), "{}", created.instructions);
        assert!(created.instructions.contains("root"), "{}", created.instructions);
        assert_eq!(created.labels.get("suggestion").map(String::as_str), Some(format!("{},{}", a.id, b.id).as_str()));

        assert_eq!(engine.l5_service().suggestion_get(&a.id).await.unwrap().state, SuggestionState::Tasked);
        assert_eq!(engine.l5_service().suggestion_get(&b.id).await.unwrap().state, SuggestionState::Tasked);
    }

    #[tokio::test]
    async fn ask_refuses_rather_than_dispatch_when_there_is_no_previous_run_to_resume() {
        let engine = test_engine(None, false);
        let task = task_in(&engine, "demo", "plain-worker").await;
        // File against an active run, then let it end so the task has no
        // run in progress -- but note `active_run` alone never gives this
        // task a *terminal* previous run, so there is nothing to resume.
        let run = active_run(&engine, &task.id, "plain-worker", "tok-1").await;
        let s = engine.l5_service().file_suggestion(&task.id, report(Some("tok-1"))).await.unwrap();
        engine
            .l4.store
            .update_run(&run.id, &factory_core::run::RunPatch {
                status: Some(factory_core::run::RunStatus::Failed),
                ended_at: Some(Utc::now()),
                ..Default::default()
            })
            .await
            .unwrap();

        let err = engine.l5_service().suggestion_ask(&Caller::Owner, &s.id, "why did this fail?".into()).await.unwrap_err();
        // `resolve_continue` refuses: `shell` declares no resume spec, and
        // either way the run was never confirmed gone. Never dispatched.
        assert!(err.to_string().to_lowercase().contains("resume") || err.to_string().contains("harness"), "{err}");
        assert!(engine.l4.store.active_run(&task.id).await.unwrap().is_none(), "ask must never have dispatched a fresh run");
    }

    #[tokio::test]
    async fn ask_refuses_while_the_task_has_a_run_in_progress() {
        let engine = test_engine(None, false);
        let task = task_in(&engine, "demo", "plain-worker").await;
        active_run(&engine, &task.id, "plain-worker", "tok-1").await;
        let s = engine.l5_service().file_suggestion(&task.id, report(Some("tok-1"))).await.unwrap();

        let err = engine.l5_service().suggestion_ask(&Caller::Owner, &s.id, "why?".into()).await.unwrap_err();
        assert!(err.to_string().contains("in progress"), "{err}");
    }
}
