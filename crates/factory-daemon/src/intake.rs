//! Intake (`#119`): the engine's side of the inbound quality gate. The model
//! and every rule are `factory_core::intake`; this is only the transitions --
//! who may hand something in, what a triage run is, and what releasing an
//! item writes.
//!
//! An item is an ordinary task in `TaskStatus::Intake`, so it keeps its id
//! and its journal across release: the triage verdict it was released on is
//! the first thing in the history of the work it became. `#118`'s
//! attestation store has not landed, so that verdict is a journal entry
//! (`TRIAGE_VERDICT_KIND`) carrying the whole assessment in `data`, and the
//! category, priority and estimate travel as labels. Nothing here reaches
//! outside Factory: no comment, label or reply on any external system.

use crate::access::Caller;
use crate::engine::{Due, Engine};
use crate::operations::Asked;
use chrono::Utc;
use factory_core::intake::{
    self, Decision, DecisionRecord, Intake, IntakeBoard, IntakeSource, IntakeStage, NewIntake, SourceKind, Verdict,
};
use factory_core::{Event, FactoryError, NewTask, Result, Task, TaskEntry, TaskFilter, TaskPatch, TaskStatus, Trigger};
use std::collections::BTreeSet;
use std::sync::Arc;

/// The journal kind of a triage verdict -- the first attestation in a
/// released task's evidence trail, until `#118` gives attestations a store.
pub(crate) const TRIAGE_VERDICT_KIND: &str = "triage_verdict";
/// The label a triage task carries: the id of the item it triages.
pub(crate) const TRIAGE_LABEL: &str = "intake-triage";

impl Engine {
    /// `Request::IntakeAdd`.
    pub(crate) async fn intake_add(&self, caller: &Caller, new: NewIntake) -> Result<Task> {
        if new.title.trim().is_empty() {
            return Err(FactoryError::BadRequest("an intake item needs a title".into()));
        }
        let requested_by = caller.describe();
        let (kind, requester, reference) = match caller {
            // The delegation path: where it came from is the caller, not a
            // claim, and the run it came out of is the natural reference.
            Caller::Agent { run_id, .. } => {
                let from_run = match run_id {
                    Some(run) => self.store.get_run(run).await?.map(|r| format!("task {}", r.task_id)),
                    None => None,
                };
                let requester = match clean(new.requester) {
                    Some(on_behalf) => format!("{on_behalf} (via {requested_by})"),
                    None => requested_by,
                };
                (SourceKind::Agent, requester, clean(new.reference).or(from_run))
            }
            Caller::Owner => {
                // A person cannot pass something off as an agent's request.
                let kind = match new.source {
                    Some(SourceKind::Ui) => SourceKind::Ui,
                    _ => SourceKind::Cli,
                };
                (kind, clean(new.requester).unwrap_or(requested_by), clean(new.reference))
            }
        };
        let record = Intake {
            stage: IntakeStage::Received,
            source: IntakeSource { kind, reference },
            requester,
            received_at: Utc::now(),
            triage: None,
            triage_task: None,
            questions: Vec::new(),
            decision: None,
        };
        let task = self
            .create_intake_task(
                NewTask {
                    title: new.title,
                    instructions: new.instructions,
                    scope: new.scope,
                    labels: new.labels,
                    ..Default::default()
                },
                record.clone(),
            )
            .await?;
        self.entry(
            &task.id,
            TaskEntry::new(
                "daemon",
                "intake_received",
                format!("received into intake from {} ({})", record.requester, record.source.kind.as_str()),
            )
            .with_data(serde_json::json!({ "source": record.source, "requester": record.requester })),
        )
        .await;
        Ok(task)
    }

    /// `Request::IntakeBoard`.
    pub(crate) async fn intake_board(&self, scope: Option<&str>) -> Result<IntakeBoard> {
        let snapshot = self.factory_snapshot();
        let members: Option<BTreeSet<String>> = match scope {
            None => None,
            Some(name) => {
                let (asked, subtree) = crate::policies::subtree_scopes(&snapshot, Some(name))?;
                let mut members: BTreeSet<String> = subtree.into_iter().map(|s| s.name).collect();
                if let Some(asked) = asked {
                    members.insert(asked.name);
                }
                Some(members)
            }
        };
        let all = self.store.list(&TaskFilter::default()).await?;
        // Items are filtered by scope; a triage task is looked up by id, so
        // it stays in the list whatever scope it ran in.
        let tasks: Vec<Task> = all
            .into_iter()
            .filter(|t| {
                t.intake.is_none() || members.as_ref().is_none_or(|m| m.contains(&t.scope))
            })
            .collect();
        Ok(intake::board(&tasks, Utc::now()))
    }

    /// `Request::IntakeTriage`: the intake workflow's triage node. A task of
    /// its own, in the item's scope, run at once; the item waits in
    /// `triaging` until the run submits an assessment.
    pub(crate) async fn intake_triage(
        self: &Arc<Self>,
        caller: &Caller,
        id: &str,
        agent: Option<String>,
    ) -> Result<Task> {
        let item = self.require(id).await?;
        let record = open_record(&item)?;
        if let Some(running) = &record.triage_task {
            if let Some(t) = self.store.get(running).await? {
                // Settled rather than closed: a triage run that failed
                // leaves its task blocked on the failure (`#122`), and
                // nothing is working on the item then.
                if !t.is_settled() {
                    return Err(FactoryError::BadRequest(format!(
                        "a triage run is already working on this item: task {} is {}",
                        t.id,
                        t.status.as_str()
                    )));
                }
            }
        }
        let factory = self.factory_snapshot();
        let scopes: Vec<String> = factory.config.scopes.iter().map(|s| s.name.clone()).collect();
        let triage = self
            .create(NewTask {
                title: format!("Triage: {}", item.title),
                instructions: intake::triage_instructions(&item, &scopes, &self.factory_bin.display().to_string()),
                scope: Some(item.scope.clone()),
                agent,
                labels: [(TRIAGE_LABEL.to_string(), item.id.clone())].into_iter().collect(),
                // Triage reads and changes no file, so a fresh worktree is
                // pure cost -- and would refuse a scope that is not a git
                // repository. The one workspace rule this bends, never two
                // live sessions in one tree, is about writers; a read-only
                // run beside one is the tolerable case.
                worktree: Some(false),
                ..Default::default()
            })
            .await?;
        let mut next = record.clone();
        next.stage = IntakeStage::Triaging;
        next.triage_task = Some(triage.id.clone());
        let item = self.write_intake(&item.id, next, TaskPatch::default()).await?;
        let asked = Asked::new(caller, None);
        self.entry(
            &item.id,
            asked.entry(
                "intake_triage_started",
                format!("triage started {} as task {} ({})", asked.words(), triage.id, triage.agent),
                serde_json::json!({ "triage_task": triage.id }),
            ),
        )
        .await;
        let engine = self.clone();
        let triage_id = triage.id.clone();
        tokio::spawn(async move {
            engine.start_run_due(&triage_id, Trigger::Manual, Due::now()).await;
        });
        Ok(triage)
    }

    /// `Request::IntakeAssess`.
    pub(crate) async fn intake_assess(
        self: &Arc<Self>,
        caller: &Caller,
        id: &str,
        mut assessment: intake::Assessment,
        decide: bool,
    ) -> Result<Task> {
        let item = self.require(id).await?;
        let record = open_record(&item)?;
        intake::validate(&assessment).map_err(FactoryError::BadRequest)?;
        // The route has to be somewhere real now, not at release, when
        // whoever assessed it has stopped watching.
        let factory = self.factory_snapshot();
        let scope = factory.scope(&assessment.routing.scope)?.name.clone();
        assessment.routing.scope = scope.clone();
        if let Some(agent) = &assessment.routing.agent {
            let (name, _, _) = self.resolve_agent(&scope, agent)?;
            assessment.routing.agent = Some(name);
        }
        if let Some(workflow) = &assessment.routing.workflow {
            let found = self.find_workflow(&scope, workflow).await?;
            assessment.routing.workflow = Some(found.id);
        }
        let triage = intake::evaluate(&assessment, caller.describe(), Utc::now());
        let mut next = record.clone();
        next.triage = Some(triage.clone());
        next.stage = IntakeStage::Triaging;
        let item = self.write_intake(&item.id, next, TaskPatch::default()).await?;
        let asked = Asked::new(caller, None);
        self.entry(
            &item.id,
            asked.entry(
                "triage_assessed",
                format!(
                    "assessed {}: {} -- {}, {}, {}",
                    asked.words(),
                    verdict_words(&triage.verdict),
                    triage.assessment.category,
                    triage.priority.as_str(),
                    triage.estimate.map(|e| e.describe()).unwrap_or_else(|| "no estimate".into()),
                ),
                serde_json::json!({ "triage": triage }),
            ),
        )
        .await;
        if !decide {
            return Ok(item);
        }
        let decision = match triage.verdict {
            Verdict::Ready => Decision::Ready { run: false },
            Verdict::NeedsInfo { .. } => Decision::NeedsInfo { questions: Vec::new() },
        };
        self.intake_decide(caller, id, decision).await
    }

    /// `Request::IntakeDecide`.
    pub(crate) async fn intake_decide(self: &Arc<Self>, caller: &Caller, id: &str, decision: Decision) -> Result<Task> {
        let item = self.require(id).await?;
        let record = open_record(&item)?.clone();
        let questions = intake::check_decision(&record, &decision).map_err(FactoryError::BadRequest)?;
        let asked = Asked::new(caller, None);
        let now = Utc::now();
        let mut next = record.clone();
        let mut decided = DecisionRecord { decision: decision.clone(), by: caller.describe(), at: now, workflow_run: None };

        match &decision {
            Decision::Ready { run } => {
                let triage = record.triage.clone().expect("check_decision requires one");
                let routing = &triage.assessment.routing;
                let factory = self.factory_snapshot();
                let declared = factory.scope(&routing.scope)?.clone();
                let moved = declared.name != item.scope;
                let agent = match &routing.agent {
                    Some(agent) => agent.clone(),
                    None if moved => declared
                        .agent_adapter()
                        .map(str::to_string)
                        .unwrap_or_else(|| factory.config.daemon.default_agent.clone()),
                    None => item.agent.clone(),
                };
                let (agent, _, _) = self.resolve_agent(&declared.name, &agent)?;
                let mut labels = item.labels.clone();
                labels.extend(intake::release_labels(&triage));
                let mut patch = TaskPatch {
                    scope: Some(declared.name.clone()),
                    agent: Some(agent.clone()),
                    labels: Some(labels),
                    estimate_seconds: triage.estimate.map(|e| e.midpoint()),
                    ..Default::default()
                };
                if moved {
                    patch.runtime = Some(
                        declared.runtime.clone().unwrap_or_else(|| factory.config.daemon.default_runtime.clone()),
                    );
                }
                let released_into = match &routing.workflow {
                    // Routed to a workflow: the workflow run is the work, so
                    // the item is released by starting it and has nothing
                    // left to do itself. `start_workflow` checks the caller
                    // could create and run every node by hand.
                    Some(workflow) => {
                        let run = self.start_workflow(workflow, Default::default(), caller).await?;
                        decided.workflow_run = Some(run.id.clone());
                        patch.status = Some(TaskStatus::Done);
                        patch.result = Some(format!(
                            "released into workflow {} (run {})",
                            run.definition.name, run.id
                        ));
                        format!("workflow {} (run {})", run.definition.name, run.id)
                    }
                    None => {
                        patch.status = Some(TaskStatus::Pending);
                        format!("{} as {}", declared.name, agent)
                    }
                };
                next.stage = IntakeStage::Ready;
                next.questions.clear();
                next.decision = Some(decided.clone());
                let task = self.write_intake(&item.id, next, patch).await?;
                self.journal_verdict(&task, &asked, &decided, &triage, Some(&released_into)).await;
                if *run && routing.workflow.is_none() {
                    let due = Due::now();
                    self.entry(
                        &task.id,
                        asked.entry(
                            crate::operations::RUN_REQUESTED_KIND,
                            format!("run requested {} on release", asked.words()),
                            serde_json::json!({ "queued_at": due.queued_at }),
                        ),
                    )
                    .await;
                    let engine = self.clone();
                    let id = task.id.clone();
                    tokio::spawn(async move {
                        engine.start_run_due(&id, Trigger::Manual, due).await;
                    });
                }
                Ok(task)
            }
            Decision::NeedsInfo { .. } => {
                next.stage = IntakeStage::NeedsInfo;
                next.questions = questions.clone();
                next.decision = Some(decided.clone());
                let task = self.write_intake(&item.id, next, TaskPatch::default()).await?;
                match &record.triage {
                    Some(triage) => self.journal_verdict(&task, &asked, &decided, triage, None).await,
                    None => {
                        self.entry(
                            &task.id,
                            asked.entry(
                                "intake_needs_info",
                                format!("sent back for information {}: {}", asked.words(), questions.join("; ")),
                                serde_json::json!({ "questions": questions }),
                            ),
                        )
                        .await
                    }
                }
                Ok(task)
            }
            Decision::Wontfix { reason, evidence, duplicate_of } => {
                let why = match duplicate_of {
                    Some(of) => format!("wontfix: duplicate of {of} -- {}", evidence.trim()),
                    None => format!("wontfix: {} -- {}", reason.as_str().replace('_', " "), evidence.trim()),
                };
                next.stage = IntakeStage::Wontfix;
                next.questions.clear();
                next.decision = Some(decided.clone());
                let patch = TaskPatch {
                    status: Some(TaskStatus::Cancelled),
                    result: Some(why.clone()),
                    ..Default::default()
                };
                let task = self.write_intake(&item.id, next, patch).await?;
                self.entry(
                    &task.id,
                    asked.entry(
                        "intake_closed",
                        format!("{why} ({})", asked.words()),
                        serde_json::json!({ "decision": decided }),
                    ),
                )
                .await;
                Ok(task)
            }
        }
    }

    /// `Request::IntakeInfo`: more from the requester. It goes into the
    /// item's own text, so a triage run after it reads it with the rest, and
    /// a needs-info item goes back into the queue.
    pub(crate) async fn intake_info(&self, caller: &Caller, id: &str, text: &str) -> Result<Task> {
        let item = self.require(id).await?;
        let record = open_record(&item)?;
        let text = text.trim();
        if text.is_empty() {
            return Err(FactoryError::BadRequest("no information given".into()));
        }
        let by = caller.describe();
        let instructions = format!(
            "{}\n\n---\nMore information from {by}, {}:\n\n{text}",
            item.instructions.trim_end(),
            Utc::now().format("%Y-%m-%d %H:%M UTC"),
        );
        let mut next = record.clone();
        if next.stage == IntakeStage::NeedsInfo {
            next.stage = IntakeStage::Received;
            next.questions.clear();
        }
        let task = self
            .write_intake(&item.id, next, TaskPatch { instructions: Some(instructions), ..Default::default() })
            .await?;
        let asked = Asked::new(caller, None);
        self.entry(
            &task.id,
            asked.entry("intake_info", format!("information added {}", asked.words()), serde_json::json!({})),
        )
        .await;
        Ok(task)
    }

    /// Whether `caller` is the run of this item's own triage task -- the one
    /// reach an agent has over an item it was not assigned, and exactly as
    /// far as `task report` trusts a run's token.
    pub(crate) async fn is_items_triage_run(&self, caller: &Caller, item: &Task) -> Result<bool> {
        let Caller::Agent { run_id: Some(run_id), .. } = caller else { return Ok(false) };
        let Some(triage_task) = item.intake.as_ref().and_then(|i| i.triage_task.as_deref()) else {
            return Ok(false);
        };
        Ok(self.store.get_run(run_id).await?.is_some_and(|run| run.task_id == triage_task))
    }

    async fn find_workflow(&self, scope: &str, wanted: &str) -> Result<factory_core::WorkflowDefinition> {
        let definitions = self.workflows.definitions(Some(scope)).await?;
        definitions
            .iter()
            .find(|d| d.id == wanted)
            .or_else(|| definitions.iter().find(|d| d.name == wanted))
            .cloned()
            .ok_or_else(|| {
                let names: Vec<&str> = definitions.iter().map(|d| d.name.as_str()).collect();
                FactoryError::BadRequest(format!(
                    "no workflow {wanted:?} in {scope}; it has: {}",
                    if names.is_empty() { "none".into() } else { names.join(", ") }
                ))
            })
    }

    /// The one place the intake record is written. Straight to the store,
    /// past `Engine::update`, which refuses the field from any caller.
    async fn write_intake(&self, id: &str, record: Intake, mut patch: TaskPatch) -> Result<Task> {
        patch.intake = Some(record);
        let task = self.store.update(id, &patch).await?;
        if task.intake.is_none() {
            return Err(FactoryError::adapter(
                self.store.name(),
                "the task store did not keep the intake record; it may predate intake",
            ));
        }
        self.bus.publish(Event::TaskUpdated { task: task.clone() });
        Ok(task)
    }

    async fn journal_verdict(
        &self,
        task: &Task,
        asked: &Asked,
        decided: &DecisionRecord,
        triage: &intake::Triage,
        released_into: Option<&str>,
    ) {
        let message = match released_into {
            Some(into) => format!(
                "triage verdict: ready -- {}, {}, {}; released into {into} {}",
                triage.assessment.category,
                triage.priority.as_str(),
                triage.estimate.map(|e| e.describe()).unwrap_or_else(|| "no estimate".into()),
                asked.words(),
            ),
            None => format!(
                "triage verdict: needs-info {}: {}",
                asked.words(),
                task.intake.as_ref().map(|i| i.questions.join("; ")).unwrap_or_default()
            ),
        };
        self.entry(
            &task.id,
            asked.entry(
                TRIAGE_VERDICT_KIND,
                message,
                serde_json::json!({
                    // Shaped as the attestation it stands in for: what was
                    // attested, by whom, on what evidence.
                    "attestation": "triage",
                    "verdict": decided.decision.as_str(),
                    "decision": decided,
                    "triage": triage,
                }),
            ),
        )
        .await;
    }
}

/// The item's record, if it is still inside the gate.
fn open_record(task: &Task) -> Result<&Intake> {
    let record = task
        .intake
        .as_ref()
        .ok_or_else(|| FactoryError::BadRequest(format!("task {} was not handed in through intake", task.id)))?;
    if !record.stage.is_open() {
        return Err(FactoryError::BadRequest(format!(
            "this item already left intake ({})",
            record.stage.as_str()
        )));
    }
    Ok(record)
}

fn clean(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn verdict_words(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Ready => "ready".into(),
        Verdict::NeedsInfo { blockers } => format!("needs-info ({})", blockers.join("; ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::{AgentRuntime, StartRequest};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::intake::{Assessment, Axis, AxisCheck, Level, Routing, WontfixReason};
    use factory_core::protocol::{Payload, Request, Response};
    use factory_core::role::Role;
    use factory_core::task::SessionRef;
    use factory_core::workflow::{CanvasPoint, WorkflowDraft, WorkflowNode, WorkflowNodeKind};
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    struct QuietRuntime;
    #[async_trait::async_trait]
    impl AgentRuntime for QuietRuntime {
        fn name(&self) -> &str {
            "quiet"
        }
        async fn start(&self, req: &StartRequest) -> Result<SessionRef> {
            Ok(SessionRef { runtime: "quiet".into(), handle: req.id.clone(), meta: Default::default() })
        }
        async fn submit(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn status(&self, _: &SessionRef) -> Result<factory_core::adapter::RuntimeStatus> {
            Ok(factory_core::adapter::RuntimeStatus::Working)
        }
        async fn send_text(&self, _: &SessionRef, _: &str) -> Result<()> {
            Ok(())
        }
        async fn send_keys(&self, _: &SessionRef, _: &[String]) -> Result<()> {
            Ok(())
        }
        async fn read(&self, _: &SessionRef, _: u32) -> Result<String> {
            Ok(String::new())
        }
        async fn stop(&self, _: &SessionRef) -> Result<()> {
            Ok(())
        }
    }

    fn scope(name: &str) -> Scope {
        Scope {
            id: format!("{name}-id"),
            name: name.into(),
            path: PathBuf::new(),
            agent: None,
            agents: Vec::new(),
            runtime: Some("quiet".into()),
            git: None,
            task_store: None,
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            dependencies: Default::default(),
        }
    }

    fn engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-intake-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            // No real `caffeinate` from a test that dispatches.
            daemon: DaemonConfig { power_assertion: false, default_agent: "shell".into(), ..DaemonConfig::default() },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes: vec![scope("demo"), scope("web")],
            infrastructure: Default::default(),
            plugins_dir: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime), "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::in_memory().unwrap()),
            PathBuf::from("/bin/factory"),
            Vec::new(),
        ))
    }

    fn worker(run_id: Option<String>) -> Caller {
        Caller::Agent { scope: "demo".into(), name: "w".into(), role: Role::new("worker"), run_id }
    }

    fn assessment(scope: &str) -> Assessment {
        Assessment {
            axes: Axis::ALL
                .into_iter()
                .map(|axis| AxisCheck { axis, pass: true, evidence: format!("{} holds", axis.as_str()), cost: None })
                .collect(),
            category: "bugfix".into(),
            impact: Level::High,
            urgency: Level::Medium,
            complexity: 3,
            estimate: None,
            routing: Routing { scope: scope.into(), agent: Some("shell".into()), workflow: None },
            summary: "bounded".into(),
            questions: vec![],
        }
    }

    fn unclear() -> Assessment {
        let mut a = assessment("demo");
        let v = a.axes.iter_mut().find(|c| c.axis == Axis::Verifiability).unwrap();
        v.pass = false;
        v.evidence = "nothing says what done looks like".into();
        a.questions = vec!["What should the page show when it works?".into()];
        a
    }

    async fn add(engine: &Arc<Engine>, title: &str) -> Task {
        engine
            .intake_add(
                &Caller::Owner,
                NewIntake { title: title.into(), instructions: "fix it".into(), scope: Some("demo".into()), ..Default::default() },
            )
            .await
            .unwrap()
    }

    async fn kinds(engine: &Arc<Engine>, id: &str) -> Vec<String> {
        engine.store.entries(id, 200).await.unwrap().into_iter().map(|e| e.kind).collect()
    }

    fn refused(response: Response) -> String {
        match response {
            Response::Error { message, .. } => message,
            Response::Ok { data } => panic!("expected a refusal, got {data:?}"),
        }
    }

    #[tokio::test]
    async fn an_item_is_held_outside_the_queue_and_task_run_refuses_it() {
        let engine = engine();
        let item = add(&engine, "Broken link").await;
        assert_eq!(item.status, TaskStatus::Intake);
        let record = item.intake.as_ref().unwrap();
        assert_eq!(record.stage, IntakeStage::Received);
        assert_eq!(record.source.kind, SourceKind::Cli);
        assert_eq!(record.requester, "the owner");
        assert_eq!(item.runs, 0);
        assert!(kinds(&engine, &item.id).await.contains(&"intake_received".to_string()));

        let far = Utc::now() + chrono::Duration::days(365);
        assert!(engine.store.due(far).await.unwrap().is_empty(), "never due");
        let why = refused(engine.handle_request(Request::TaskRun { id: item.id.clone(), reason: None }).await);
        assert!(why.contains("still in intake"), "{why}");
        engine.start_run(&item.id, Trigger::Manual).await;
        let after = engine.require(&item.id).await.unwrap();
        assert_eq!(after.status, TaskStatus::Intake, "held, not failed");
        assert!(engine.store.active_run(&item.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_edit_cannot_move_an_item_out_of_intake_or_forge_its_record() {
        let engine = engine();
        let item = add(&engine, "x").await;
        let status = TaskPatch { status: Some(TaskStatus::Pending), ..Default::default() };
        let why = engine.update(&item.id, status, None).await.unwrap_err().to_string();
        assert!(why.contains("by a decision"), "{why}");
        let forged = TaskPatch { intake: item.intake.clone(), ..Default::default() };
        assert!(engine.update(&item.id, forged, None).await.is_err());
    }

    #[tokio::test]
    async fn an_agents_item_is_recorded_as_its_delegation_whatever_it_claims() {
        let engine = engine();
        let item = engine
            .intake_add(
                &worker(None),
                NewIntake {
                    title: "Needs a hand".into(),
                    source: Some(SourceKind::Ui),
                    requester: Some("a customer".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let record = item.intake.unwrap();
        assert_eq!(record.source.kind, SourceKind::Agent);
        assert_eq!(record.requester, "a customer (via w (worker) in demo)");
    }

    #[tokio::test]
    async fn ready_releases_the_item_with_category_priority_estimate_and_a_journaled_verdict() {
        let engine = engine();
        let item = add(&engine, "Broken link").await;
        let released = engine.intake_assess(&Caller::Owner, &item.id, assessment("web"), true).await.unwrap();
        assert_eq!(released.status, TaskStatus::Pending);
        assert_eq!(released.scope, "web", "routed");
        assert_eq!(released.agent, "shell");
        assert_eq!(released.labels["triage"], "ready");
        assert_eq!(released.labels["category"], "bugfix");
        assert_eq!(released.labels["priority"], "P2");
        assert_eq!(released.labels["estimate"], "45m-2h");
        assert_eq!(released.estimate_seconds, Some((45 * 60 + 2 * 3600) / 2));
        let record = released.intake.as_ref().unwrap();
        assert_eq!(record.stage, IntakeStage::Ready);
        assert!(matches!(record.decision.as_ref().unwrap().decision, Decision::Ready { run: false }));

        let entries = engine.store.entries(&item.id, 200).await.unwrap();
        let verdict = entries.iter().find(|e| e.kind == TRIAGE_VERDICT_KIND).expect("the verdict is journaled");
        let data = verdict.data.as_ref().unwrap();
        assert_eq!(data["attestation"], "triage");
        assert_eq!(data["verdict"], "ready");
        assert_eq!(data["triage"]["priority"], "P2");
        assert_eq!(data["triage"]["assessment"]["axes"].as_array().unwrap().len(), 7);

        // And now it is ordinary work: dispatch goes ahead rather than
        // holding it. (It then fails on the test scope not being a git
        // repository -- a released item keeps a task's default worktree.)
        engine.start_run(&item.id, Trigger::Manual).await;
        assert!(!kinds(&engine, &item.id).await.contains(&"intake_held".to_string()));
        let t = engine.require(&item.id).await.unwrap();
        assert!(t.error.as_deref().unwrap_or("").contains("git"), "{:?} {:?}", t.status, t.error);
    }

    #[tokio::test]
    async fn needs_info_sends_it_back_and_information_puts_it_in_the_queue_again() {
        let engine = engine();
        let item = add(&engine, "Make it better").await;
        let back = engine.intake_assess(&Caller::Owner, &item.id, unclear(), true).await.unwrap();
        assert_eq!(back.status, TaskStatus::Intake);
        let record = back.intake.as_ref().unwrap();
        assert_eq!(record.stage, IntakeStage::NeedsInfo);
        assert_eq!(record.questions, vec!["What should the page show when it works?"]);
        assert!(kinds(&engine, &item.id).await.contains(&TRIAGE_VERDICT_KIND.to_string()));

        let why = engine.intake_decide(&Caller::Owner, &item.id, Decision::Ready { run: false }).await.unwrap_err();
        assert!(why.to_string().contains("needs-info"), "{why}");

        let answered = engine.intake_info(&Caller::Owner, &item.id, "It shows the order list.").await.unwrap();
        let record = answered.intake.as_ref().unwrap();
        assert_eq!(record.stage, IntakeStage::Received);
        assert!(record.questions.is_empty());
        assert!(answered.instructions.contains("It shows the order list."));
        assert!(answered.instructions.starts_with("fix it"));

        let released = engine.intake_assess(&Caller::Owner, &item.id, assessment("demo"), true).await.unwrap();
        assert_eq!(released.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn wontfix_closes_it_with_the_reason_and_no_second_decision_follows() {
        let engine = engine();
        let item = add(&engine, "Same as before").await;
        let unnamed = Decision::Wontfix { reason: WontfixReason::Duplicate, evidence: "same".into(), duplicate_of: None };
        assert!(engine.intake_decide(&Caller::Owner, &item.id, unnamed).await.is_err());
        let closed = engine
            .intake_decide(
                &Caller::Owner,
                &item.id,
                Decision::Wontfix {
                    reason: WontfixReason::Duplicate,
                    evidence: "same stack trace".into(),
                    duplicate_of: Some("t-42".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(closed.status, TaskStatus::Cancelled);
        assert_eq!(closed.result.as_deref(), Some("wontfix: duplicate of t-42 -- same stack trace"));
        assert_eq!(closed.intake.as_ref().unwrap().stage, IntakeStage::Wontfix);
        let again = engine.intake_assess(&Caller::Owner, &item.id, assessment("demo"), true).await;
        assert!(again.unwrap_err().to_string().contains("already left intake"));
    }

    #[tokio::test]
    async fn an_assessment_routed_nowhere_real_is_refused_before_anything_is_written() {
        let engine = engine();
        let item = add(&engine, "x").await;
        assert!(engine.intake_assess(&Caller::Owner, &item.id, assessment("nowhere"), false).await.is_err());
        let mut a = assessment("demo");
        a.routing.workflow = Some("no-such-flow".into());
        let why = engine.intake_assess(&Caller::Owner, &item.id, a, false).await.unwrap_err();
        assert!(why.to_string().contains("no workflow"), "{why}");
        assert!(engine.require(&item.id).await.unwrap().intake.unwrap().triage.is_none());
    }

    #[tokio::test]
    async fn routed_to_a_workflow_it_is_released_by_starting_that_workflow() {
        let engine = engine();
        engine
            .create_workflow(WorkflowDraft {
                name: "fix-flow".into(),
                scope: "demo".into(),
                nodes: vec![WorkflowNode {
                    id: "fix".into(),
                    position: CanvasPoint::default(),
                    kind: WorkflowNodeKind::Task,
                    task: NewTask {
                        title: "fix".into(),
                        instructions: "true".into(),
                        scope: Some("demo".into()),
                        agent: Some("shell".into()),
                        worktree: Some(false),
                        ..Default::default()
                    },
                    gate: None,
                    exits: Vec::new(),
                }],
                ..Default::default()
            })
            .await
            .unwrap();
        let item = add(&engine, "Fix it through the flow").await;
        let mut a = assessment("demo");
        a.routing.workflow = Some("fix-flow".into());
        let released = engine.intake_assess(&Caller::Owner, &item.id, a, true).await.unwrap();
        assert_eq!(released.status, TaskStatus::Done);
        let run_id = released.intake.as_ref().unwrap().decision.as_ref().unwrap().workflow_run.clone().unwrap();
        assert!(released.result.as_deref().unwrap().contains(&run_id));
        assert!(engine.workflow_run(&run_id).await.is_ok());
        assert!(released.labels.contains_key("workflow"));
    }

    #[tokio::test]
    async fn the_triage_node_is_a_run_of_its_own_and_only_one_at_a_time() {
        let engine = engine();
        let item = add(&engine, "Broken link").await;
        let triage = engine.intake_triage(&Caller::Owner, &item.id, Some("shell".into())).await.unwrap();
        assert_eq!(triage.status, TaskStatus::Pending);
        assert_eq!(triage.labels[TRIAGE_LABEL], item.id);
        assert_eq!(triage.scope, "demo");
        assert!(!triage.worktree, "triage reads; it gets no worktree of its own");
        assert!(triage.instructions.contains(&format!("/bin/factory intake assess {} --file", item.id)));
        let record = engine.require(&item.id).await.unwrap().intake.unwrap();
        assert_eq!(record.stage, IntakeStage::Triaging);
        assert_eq!(record.triage_task.as_deref(), Some(triage.id.as_str()));

        let again = engine.intake_triage(&Caller::Owner, &item.id, None).await.unwrap_err();
        assert!(again.to_string().contains("already working"), "{again}");
        let board = engine.intake_board(None).await.unwrap();
        assert_eq!(board.columns.triaging.len(), 1);
        assert_eq!(engine.intake_board(Some("web")).await.unwrap().columns.triaging.len(), 0);
    }

    #[tokio::test]
    async fn the_triage_run_may_answer_for_its_item_and_nobody_elses() {
        let engine = engine();
        let item = add(&engine, "Broken link").await;
        let other = add(&engine, "Something else").await;
        let triage = engine.intake_triage(&Caller::Owner, &item.id, Some("shell".into())).await.unwrap();
        let run = loop {
            if let Some(run) = engine.store.active_run(&triage.id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let assess = |id: &str| Request::IntakeAssess { id: id.into(), assessment: assessment("demo"), decide: true };

        // A worker's reach is its own work, and the item is not assigned to it.
        let stranger = worker(None);
        let why = engine.authorize(&stranger, &assess(&item.id)).await.unwrap_err();
        assert!(why.to_string().contains("triage run"), "{why}");

        let triager = worker(Some(run.id.clone()));
        engine.authorize(&triager, &assess(&item.id)).await.unwrap();
        assert!(engine.authorize(&triager, &assess(&other.id)).await.is_err(), "only its own item");
        let elsewhere = Request::IntakeAssess { id: item.id.clone(), assessment: assessment("web"), decide: true };
        assert!(engine.authorize(&triager, &elsewhere).await.is_err(), "and only into its own scope");

        let released = engine.intake_assess(&triager, &item.id, assessment("demo"), true).await.unwrap();
        assert_eq!(released.status, TaskStatus::Pending);
        assert_eq!(released.intake.unwrap().triage.unwrap().by, "w (worker) in demo");
    }

    #[tokio::test]
    async fn the_board_answers_over_the_wire_and_intake_add_answers_the_task() {
        let engine = engine();
        let response = engine
            .handle_request(Request::IntakeAdd(NewIntake { title: "via the wire".into(), ..Default::default() }))
            .await;
        let Response::Ok { data: Payload::Task { task } } = response else { panic!("{response:?}") };
        assert_eq!(task.status, TaskStatus::Intake);
        let response = engine.handle_request(Request::IntakeBoard { scope: None }).await;
        let Response::Ok { data: Payload::IntakeBoard { board } } = response else { panic!("{response:?}") };
        assert_eq!(board.columns.received.len(), 1);
        assert_eq!(board.axes.len(), 7);
    }
}
