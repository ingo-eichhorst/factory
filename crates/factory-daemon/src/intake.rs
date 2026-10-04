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
//!
//! A split is the other way out: the item is replaced by smaller intake
//! items in the same scope, each carrying `intake-parent`, and closes as
//! `Done` with their ids -- the precedent a workflow release set.

use crate::access::Caller;
use crate::engine::{Due, Engine};
use crate::operations::Asked;
use chrono::{DateTime, Utc};
use factory_core::config::Factory;
use factory_core::control_plan;
use factory_core::intake::{
    self, AgentOption, Decision, DecisionRecord, Intake, IntakeBoard, IntakeSource, IntakeStage, NewIntake,
    RouteOptions, SourceKind, Verdict, WorkflowOption, TRIAGE_LABEL,
};
use factory_core::ready::{self, ReadyDefinition};
use factory_core::role::Grant;
use factory_core::intake::IntakeSourceIdentity;
use factory_core::{Event, FactoryError, NewTask, Result, Task, TaskEntry, TaskFilter, TaskPatch, TaskStatus, Trigger};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The journal kind of a triage verdict -- the first attestation in a
/// released task's evidence trail, until `#118` gives attestations a store.
pub(crate) const TRIAGE_VERDICT_KIND: &str = "triage_verdict";

impl Engine {
    /// `Request::IntakeAdd`. A relayed `email` or `chat` item (`#167`) is
    /// its own path, open to Owner and Agent alike; every other kind keeps
    /// exactly the per-caller provenance it always had.
    pub(crate) async fn intake_add(&self, caller: &Caller, new: NewIntake) -> Result<Task> {
        if new.title.trim().is_empty() {
            return Err(FactoryError::BadRequest("an intake item needs a title".into()));
        }
        if matches!(new.source, Some(SourceKind::Email) | Some(SourceKind::Chat)) {
            // Boxed: this is one more nested `.await` on top of an already
            // tight chain (`Engine::handle_request`'s own dispatch already
            // boxes `intake_add`'s call), so it inlining here would size
            // every request's future -- the same reason `intake_decide` and
            // friends box their own calls above.
            return Box::pin(self.relay_intake(caller, new)).await;
        }
        // `provider`/`received_at` only mean anything for a relayed item:
        // carrying either one here is a malformed relay, whatever kind it
        // claims (including a bare `github`, which never becomes trusted
        // provenance this way regardless -- see the arm below).
        if new.provider.is_some() || new.received_at.is_some() {
            return Err(FactoryError::BadRequest(
                "provider and received_at only apply to a relayed item -- pass --source email or --source chat".into(),
            ));
        }
        let requested_by = caller.describe();
        let flagged_by = requested_by.clone();
        let now = Utc::now();
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
                // A person cannot pass something off as an agent's request,
                // and never off as GitHub's either -- that stays the
                // poller's alone, whatever a caller claims.
                let kind = match new.source {
                    Some(SourceKind::Ui) => SourceKind::Ui,
                    _ => SourceKind::Cli,
                };
                (kind, clean(new.requester).unwrap_or(requested_by), clean(new.reference))
            }
        };
        // `--security`/the UI checkbox: flag it before it is even stored
        // (`#170`). A caller can only add scrutiny this way, so it needs
        // nothing beyond `intake.add` itself.
        let security = if new.security {
            Some(Box::new(intake::flag_security(None, "flagged at intake", &flagged_by, now).expect(
                "a record that does not exist yet never already carries a flag",
            )))
        } else {
            None
        };
        let record = Intake {
            stage: IntakeStage::Received,
            source: Box::new(IntakeSource { kind, reference, provider: None, relayed_by: None, repository: None, number: None, external_id: None }),
            requester,
            received_at: now,
            triage: None,
            triage_task: None,
            questions: Vec::new(),
            decision: None,
            candidates: Vec::new(),
            security,
            outbound: None,
        };
        Box::pin(self.receive_intake(
            NewTask {
                title: new.title,
                instructions: new.instructions,
                scope: new.scope,
                labels: new.labels,
                ..Default::default()
            },
            record,
        ))
        .await
    }

    /// The relayed-provenance path (`#167`) for `source: email` or `source:
    /// chat`: open to Owner and Agent alike, since neither is the source
    /// itself, only the one handing it in. Nothing the caller sends is
    /// trusted beyond that it is relaying: `relayed_by` is always
    /// `caller.describe()`, never a claim. What *is* required, because
    /// there is no other way to identify or attribute the item: the
    /// provider's own message id (`reference`) and who it is from
    /// (`requester`, the sender's own address or handle -- unlike the
    /// ordinary Agent path above, never folded into a "via" suffix, since
    /// `relayed_by` already carries that). `received_at` is the provider's
    /// own receipt time, refused if it is in the future, defaulted to now
    /// when absent.
    async fn relay_intake(&self, caller: &Caller, new: NewIntake) -> Result<Task> {
        let now = Utc::now();
        let kind = match new.source {
            Some(kind @ (SourceKind::Email | SourceKind::Chat)) => kind,
            // Unreachable from `intake_add`'s own gate above; kept as the
            // relay's own refusal in case another caller ever reaches this
            // directly -- github and every other kind arrive their own way,
            // never relayed.
            _ => return Err(FactoryError::BadRequest("only email and chat are relayed this way".into())),
        };
        let reference = clean(new.reference).ok_or_else(|| {
            FactoryError::BadRequest(format!(
                "a relayed {} item needs the provider's own message id as --reference",
                kind.as_str()
            ))
        })?;
        let requester = clean(new.requester)
            .ok_or_else(|| FactoryError::BadRequest("a relayed item needs --requester -- who it is from".into()))?;
        let received_at = match new.received_at {
            Some(at) if at > now => {
                return Err(FactoryError::BadRequest("received_at cannot be in the future".into()));
            }
            Some(at) => at,
            None => now,
        };
        let relayed_by = caller.describe();
        let flagged_by = relayed_by.clone();
        let security = if new.security {
            Some(Box::new(intake::flag_security(None, "flagged at intake", &flagged_by, now).expect(
                "a record that does not exist yet never already carries a flag",
            )))
        } else {
            None
        };
        let record = Intake {
            stage: IntakeStage::Received,
            source: Box::new(IntakeSource { kind, reference: Some(reference), provider: clean(new.provider), relayed_by: Some(relayed_by), repository: None, number: None, external_id: None }),
            requester,
            received_at,
            triage: None,
            triage_task: None,
            questions: Vec::new(),
            decision: None,
            candidates: Vec::new(),
            security,
            outbound: None,
        };
        Box::pin(self.receive_intake(
            NewTask {
                title: new.title,
                instructions: new.instructions,
                scope: new.scope,
                labels: new.labels,
                ..Default::default()
            },
            record,
        ))
        .await
    }

    /// Receive an item whose provenance was established inside the daemon.
    /// Keeping this separate from `NewIntake` means public callers cannot
    /// choose a trusted source kind or its timestamp. Idempotent by
    /// identity (`#167`, `IntakeSource::identity`): for GitHub, a relayed
    /// email or a relayed chat, a second receipt with the same `(kind,
    /// provider, reference)` returns the existing item unchanged -- no
    /// second task, no second `intake_received` entry -- so the GitHub
    /// poller's own retries and a relay's own retries are both safe. The
    /// lookup and the create happen under one lock
    /// (`Engine::intake_receipt_lock`), so two identical requests racing
    /// each other still produce exactly one item. Every other kind (`cli`,
    /// `ui`, `agent`) always creates, exactly as it always has; `#166`'s
    /// duplicate candidate search, below, is its only signal that a repeat
    /// came in.
    pub(crate) async fn receive_intake(&self, new: NewTask, record: Intake) -> Result<Task> {
        let _guard = self.intake_receipt_lock.lock().await;
        let all = self.store.list(&TaskFilter::default()).await?;
        if let Some(identity) = record.source.identity() {
            if let Some(existing) =
                all.iter().find(|t| t.intake.as_ref().and_then(|i| i.source.identity()) == Some(identity.clone()))
            {
                return Ok(existing.clone());
            }
        }
        let task = self.create_intake_task(new, record.clone()).await?;
        // `all` was read before `task` was created, so it never contains
        // `task` itself -- the same set `duplicate_candidates` would search
        // after excluding the item by id, one list call doing both jobs.
        let candidates = intake::duplicate_candidates(&task, &all);
        let task = if candidates.is_empty() {
            task
        } else {
            let mut next = record.clone();
            next.candidates = candidates.clone();
            self.write_intake(&task.id, next, TaskPatch::default()).await?
        };
        self.entry(
            &task.id,
            TaskEntry::new(
                "daemon",
                "intake_received",
                format!("received into intake from {} ({})", record.requester, record.source.kind.as_str()),
            )
            .with_data(serde_json::json!({
                "source": record.source,
                "requester": record.requester,
                "candidates": candidates,
            })),
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
                let (asked, subtree) = factory_core::config::subtree_scopes(&snapshot, Some(name))?;
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
        let ready = self.ready_definitions(&snapshot);
        let definitions: BTreeMap<String, ReadyDefinition> =
            ready.iter().map(|(scope, (def, _))| (scope.clone(), def.clone())).collect();
        let mut board = intake::board(&tasks, Utc::now(), &definitions);
        board.routes = self.intake_routes_with(&snapshot, &ready).await?;
        Ok(board)
    }

    /// Every scope's effective definition of ready (`#169`), keyed by scope
    /// name: the authored files under `factory.intake_dir()` re-read fresh
    /// (no watcher, no cache -- the same rule quality and policies follow),
    /// folded down each scope's own chain
    /// (`Config::intake_chain_for_scope`). `factory-core::ready` does the
    /// actual add-or-tighten fold and the fail-closed handling; this is only
    /// where the daemon resolves which chain applies to which live scope.
    fn ready_definitions(&self, factory: &Factory) -> BTreeMap<String, (ReadyDefinition, Vec<ready::Finding>)> {
        let catalogue = ready::load(&factory.intake_dir());
        factory
            .config
            .scopes
            .iter()
            .map(|scope| {
                let chain = factory.config.intake_chain_for_scope(scope);
                let (def, findings) = ready::effective(&catalogue, &scope.name, &chain);
                (scope.name.clone(), (def, findings))
            })
            .collect()
    }

    /// This scope's effective definition of ready, resolved fresh -- the one
    /// `intake_assess` enforces and `intake_triage` shows in the run's
    /// instructions. `scope` must already be the canonical name
    /// (`Factory::scope` having resolved it), the same as every other
    /// per-scope read here.
    fn ready_definition_for(&self, factory: &Factory, scope: &factory_core::config::Scope) -> ReadyDefinition {
        let catalogue = ready::load(&factory.intake_dir());
        let chain = factory.config.intake_chain_for_scope(scope);
        ready::effective(&catalogue, &scope.name, &chain).0
    }

    /// Every scope an item can be routed to, with the agents a task there
    /// can run on (and the model each one's args choose) and the workflows
    /// it has -- what a triager, a run or a person, chooses the route from.
    /// Standing agents are left out: they are not given tasks.
    pub(crate) async fn intake_routes(&self) -> Result<Vec<RouteOptions>> {
        let factory = self.factory_snapshot();
        let ready = self.ready_definitions(&factory);
        self.intake_routes_with(&factory, &ready).await
    }

    async fn intake_routes_with(
        &self,
        factory: &Factory,
        ready: &BTreeMap<String, (ReadyDefinition, Vec<ready::Finding>)>,
    ) -> Result<Vec<RouteOptions>> {
        let mut out = Vec::new();
        for scope in &factory.config.scopes {
            let default_agent = scope
                .agent_adapter()
                .map(str::to_string)
                .unwrap_or_else(|| factory.config.daemon.default_agent.clone());
            let mut agents: Vec<AgentOption> = scope
                .agents_with(&factory.config.daemon.foreman)
                .into_iter()
                .filter(|a| !a.lifetime.is_standing())
                .map(|a| AgentOption { name: a.name(), harness: a.harness.clone(), model: intake::model_of(&a.args) })
                .collect();
            if !agents.iter().any(|a| a.name == default_agent) {
                agents.insert(0, AgentOption { name: default_agent.clone(), harness: default_agent.clone(), model: None });
            }
            let workflows = self
                .workflows
                .definitions(Some(&scope.name))
                .await?
                .iter()
                .filter(|d| d.scope == scope.name)
                .map(WorkflowOption::from_definition)
                .collect();
            let (definition, findings) = ready.get(&scope.name).cloned().unwrap_or_default();
            out.push(RouteOptions {
                scope: scope.name.clone(),
                default_agent: Some(default_agent),
                agents,
                workflows,
                definition,
                findings,
            });
        }
        Ok(out)
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
        let record = open_record(&item)?.clone();
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
        // Whoever the run lands on has to be able to report its own result
        // -- otherwise it starts and can never close the loop (`#172`).
        // Resolve exactly the agent `create` below would land it on, named
        // or not, and check the role it would actually run under.
        let factory = self.factory_snapshot();
        let declared = factory.scope(&item.scope)?.clone();
        let requested = agent
            .clone()
            .or_else(|| declared.agent_adapter().map(str::to_string))
            .unwrap_or_else(|| factory.config.daemon.default_agent.clone());
        let (resolved, _, _) = self.resolve_agent(&item.scope, &requested)?;
        let role = self.effective_role(&item.scope, &resolved).await;
        let may_report = self
            .roles_for(&item.scope)
            .get(&role)
            .is_some_and(|def| def.allows(Grant::TaskReport));
        if !may_report {
            return Err(FactoryError::BadRequest(format!(
                "{resolved} holds {role}, which may not {} ({} needed); a triage run must be able \
                 to report its own result",
                Grant::TaskReport.describe(),
                Grant::TaskReport.as_str(),
            )));
        }
        // Search again: fresh candidates for the run's instructions and the
        // record, in case something new has appeared since receipt.
        let all = self.store.list(&TaskFilter::default()).await?;
        let candidates = intake::duplicate_candidates(&item, &all);
        let mut record_now = record.clone();
        record_now.candidates = candidates;
        let routes = self.intake_routes().await?;
        // Already computed for `item.scope` as part of `routes` above --
        // reuse it rather than reading the definition files a second time.
        let definition = routes
            .iter()
            .find(|r| r.scope == item.scope)
            .map(|r| r.definition.clone())
            .unwrap_or_default();
        let triage = self
            .create(NewTask {
                title: format!("Triage: {}", item.title),
                instructions: intake::triage_instructions(
                    &item,
                    &record_now,
                    &definition,
                    &routes,
                    &self.factory_bin.display().to_string(),
                ),
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
        let mut next = record_now;
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
        // A blank route is the assessment's own problem, refused by
        // `validate` itself with its own wording -- checked here, before
        // resolving a scope out of it, so that message survives rather than
        // `Factory::scope`'s "no such scope ''".
        if assessment.routing.scope.trim().is_empty() {
            return Err(FactoryError::BadRequest("an assessment has to route the item to a scope".into()));
        }
        // The route has to be somewhere real now, not at release, when
        // whoever assessed it has stopped watching -- and its effective
        // definition of ready (`#169`) is what `validate` and `evaluate`
        // hold the assessment to, resolved fresh (no cache, same as
        // quality and policies).
        let factory = self.factory_snapshot();
        let routed_scope = factory.scope(&assessment.routing.scope)?.clone();
        let definition = self.ready_definition_for(&factory, &routed_scope);
        intake::validate(&assessment, &definition).map_err(FactoryError::BadRequest)?;
        // `owns`/`estimate_seconds` are the executable-plan marker. Old
        // assessments with only the legacy split shape remain proposals and
        // keep the explicit owner-approved `intake decide split` fallback.
        let plan_shaped = plan_shaped(&assessment);
        let executable_plan = decide && plan_shaped;
        if executable_plan {
            intake::validate_plan(&assessment.split)
                .map_err(|error| FactoryError::BadRequest(format!("the executable plan: {error}")))?;
        }
        intake::validate_duplicates(&record.candidates, &assessment.duplicates).map_err(FactoryError::BadRequest)?;
        // Normalized once, here, the same way `routing.scope`/`agent` are
        // resolved below: `validate` itself only checks the trimmed form, so
        // an untrimmed category would otherwise reach the reference-class
        // sample match, the release patch's `Task.category`, and the
        // `category` label still carrying whitespace `effective_category`'s
        // own trim on the *task* side would never line up with.
        assessment.category = assessment.category.trim().to_string();
        let scope = routed_scope.name.clone();
        assessment.routing.scope = scope.clone();
        if let Some(agent) = &assessment.routing.agent {
            let (name, _, _) = self.resolve_agent(&scope, agent)?;
            assessment.routing.agent = Some(name);
        }
        if let Some(workflow) = &assessment.routing.workflow {
            let found = self.find_workflow(&scope, workflow).await?;
            if plan_shaped {
                // `#235`: on an executable plan the workflow is the part
                // workflow every part runs through, filled with each part's
                // own values -- held to its contract now, and again when the
                // plan expands.
                if !assessment.routing.inputs.is_empty() {
                    return Err(FactoryError::BadRequest(format!(
                        "part workflow {} is filled with each part's own values; leave routing.inputs out",
                        found.name
                    )));
                }
                found
                    .validate()
                    .and_then(|_| found.part_shape())
                    .map_err(|e| FactoryError::BadRequest(format!("part workflow {}: {e}", found.name)))?;
            } else {
                // Refused now rather than at release: an input the run needs,
                // or a step that is not there, is the assessor's to fix --
                // and so is a part workflow, which only an executable plan
                // can run.
                if found.part.is_some() {
                    return Err(FactoryError::BadRequest(format!(
                        "workflow {} is a part workflow: it runs only for the parts of an executable plan (a split with owns and estimate_seconds); route this item to an ordinary workflow",
                        found.name
                    )));
                }
                found
                    .with_inputs(&assessment.routing.inputs)
                    .map_err(|e| FactoryError::BadRequest(format!("workflow {}: {e}", found.name)))?;
            }
            let mut agents = std::collections::BTreeMap::new();
            for (step, agent) in &assessment.routing.agents {
                let node = found
                    .nodes
                    .iter()
                    .find(|n| &n.id == step && n.kind == factory_core::WorkflowNodeKind::Task)
                    .ok_or_else(|| {
                        let steps: Vec<&str> = found
                            .nodes
                            .iter()
                            .filter(|n| n.kind == factory_core::WorkflowNodeKind::Task)
                            .map(|n| n.id.as_str())
                            .collect();
                        FactoryError::BadRequest(format!(
                            "workflow {} has no step {step:?}; its steps are {}",
                            found.name,
                            steps.join(", ")
                        ))
                    })?;
                let node_scope = node.task.scope.clone().unwrap_or_else(|| found.scope.clone());
                let (name, _, _) = self.resolve_agent(&node_scope, agent)?;
                agents.insert(step.clone(), name);
            }
            assessment.routing.agents = agents;
            assessment.routing.workflow = Some(found.id);
        }
        let reference = self
            .reference_estimate_for(&scope, &assessment.category, assessment.routing.agent.as_deref(), Utc::now())
            .await?;
        let triage = intake::evaluate(&assessment, &definition, &reference, caller.describe(), Utc::now());
        let mut next = record.clone();
        next.triage = Some(triage.clone());
        next.stage = IntakeStage::Triaging;
        // A `security-report` assessment flags the item automatically
        // (`#170`) -- deterministic, no classifier, and only once: a manual
        // flag, or an earlier person's decision, is never overwritten by a
        // later re-triage.
        let auto_flagged = next.security.is_none() && assessment.category == "security-report";
        if auto_flagged {
            next.security = Some(Box::new(
                intake::flag_security(None, "assessed as a security report", &caller.describe(), Utc::now())
                    .expect("a never-flagged item cannot refuse its first flag"),
            ));
        }
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
                    estimate_words(&triage),
                ),
                serde_json::json!({ "triage": triage }),
            ),
        )
        .await;
        if auto_flagged {
            self.entry(
                &item.id,
                asked.entry(
                    "intake_security_flagged",
                    format!("flagged as a possible security report {}: category security-report", asked.words()),
                    serde_json::json!({ "reason": "assessed as a security report" }),
                ),
            )
            .await;
        }
        if !decide {
            return Ok(item);
        }
        let decision = match triage.verdict {
            Verdict::Ready => Decision::Ready { run: false },
            Verdict::NeedsInfo { .. } => Decision::NeedsInfo { questions: Vec::new() },
        };
        // A possible security report waits for a person (`#170`): a ready
        // verdict is held here, not refused -- the assessment is already
        // saved, and a triage run that did what its instructions say should
        // not see an error for it. Needs-info still goes through.
        let possible = item
            .intake
            .as_ref()
            .and_then(|i| i.security.as_ref())
            .is_some_and(|f| f.state == intake::SecurityState::Possible);
        if possible && (matches!(decision, Decision::Ready { .. }) || executable_plan) {
            self.entry(
                &item.id,
                asked.entry(
                    "intake_security_held",
                    "not released: a possible security report waits for a person to confirm or dismiss it".into(),
                    serde_json::json!({}),
                ),
            )
            .await;
            return Ok(item);
        }
        if executable_plan && intake::plan_is_ready(&assessment, &definition) {
            return self.intake_expand_plan(caller, &item).await;
        }
        self.intake_decide(caller, id, decision).await
    }

    /// `#168`'s reference class for `scope`+`category` (narrowed to `agent`
    /// when that alone clears the minimum), gathered from the store the same
    /// way `#117`'s own first-turn cohort is (`record_re_estimate` in
    /// `costs.rs`): a cheap pre-filter over the task list -- `Done`, not a
    /// triage bookkeeping task, the same effective category, the same exact
    /// canonical scope -- then each survivor's own runs.
    /// `factory_core::intake::reference_estimate` does the actual sample
    /// selection, exclusions and percentiles; everything here only fetches
    /// what it needs and re-checks nothing.
    async fn reference_estimate_for(
        &self,
        scope: &str,
        category: &str,
        agent: Option<&str>,
        at: DateTime<Utc>,
    ) -> Result<intake::ReferenceEstimate> {
        let snapshot = self.factory_snapshot();
        let target = snapshot.canonical_scope_name(scope);
        let tasks = self.store.list(&TaskFilter::default()).await?;
        let mut sample_tasks = Vec::new();
        let mut sample_runs = Vec::new();
        for task in tasks {
            if task.status != TaskStatus::Done {
                continue;
            }
            if task.labels.contains_key(TRIAGE_LABEL) {
                continue;
            }
            if snapshot.canonical_scope_name(&task.scope) != target {
                continue;
            }
            if control_plan::effective_category(task.category.as_deref()) != category {
                continue;
            }
            let Ok(runs) = self.store.runs(&task.id, u32::MAX).await else { continue };
            sample_runs.extend(runs);
            sample_tasks.push(task);
        }
        Ok(intake::reference_estimate(scope, category, agent, at, &sample_tasks, &sample_runs, |s| {
            snapshot.canonical_scope_name(s)
        }))
    }

    /// `Request::IntakeDecide`.
    pub(crate) async fn intake_decide(self: &Arc<Self>, caller: &Caller, id: &str, decision: Decision) -> Result<Task> {
        let item = self.require(id).await?;
        let record = open_record(&item)?.clone();
        let questions = intake::check_decision(&record, &decision).map_err(FactoryError::BadRequest)?;
        let asked = Asked::new(caller, None);
        let now = Utc::now();
        let mut next = record.clone();
        let mut decided = DecisionRecord {
            decision: decision.clone(),
            by: caller.describe(),
            at: now,
            workflow_run: None,
            parts: Vec::new(),
            part_workflow: None,
        };

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
                // The full range, not just its midpoint (`estimate_seconds`
                // is advisory only) -- and the category itself, not only its
                // label copy: `#117`'s reference classes and `#118`'s
                // control plan both key on `Task.category`, and until this
                // (`#168`) a released item was invisible to either (the
                // triage's own finding). Setting it here means a released
                // item now also picks up any control plan declared for its
                // category, which it did not before.
                let task_estimate = triage.estimate.map(|e| factory_core::task::Estimate {
                    time: factory_core::task::TimeEstimateRange {
                        low: e.min_seconds,
                        expected: e.midpoint(),
                        high: e.max_seconds,
                    },
                    cost: e.cost,
                });
                let mut patch = TaskPatch {
                    scope: Some(declared.name.clone()),
                    agent: Some(agent.clone()),
                    labels: Some(labels),
                    estimate_seconds: task_estimate.as_ref().map(|e| e.time.expected),
                    estimate: task_estimate,
                    category: Some(triage.assessment.category.clone()),
                    ..Default::default()
                };
                if moved {
                    patch.runtime = Some(
                        declared.runtime.clone().unwrap_or_else(|| factory.config.daemon.default_runtime.clone()),
                    );
                }
                let released_into = match &routing.workflow {
                    // `#235`: on a plan the route is the part workflow every
                    // part runs through. Released as one item it would run
                    // once, for no part, and the plan would be dropped.
                    Some(workflow) if plan_shaped(&triage.assessment) => {
                        return Err(FactoryError::BadRequest(format!(
                            "this assessment is a plan whose parts run through part workflow {workflow}; releasing it as one item would drop the plan. Expand it instead: `factory intake assess {} --file <assessment> --decide`",
                            item.id
                        )));
                    }
                    // Routed to a workflow: the workflow run is the work, so
                    // the item is released by starting it and has nothing
                    // left to do itself. `start_workflow` checks the caller
                    // could create and run every node by hand.
                    Some(workflow) => {
                        let run = self
                            .start_workflow_with_agents(workflow, routing.inputs.clone(), &routing.agents, caller)
                            .await?;
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
                next.outbound = crate::github_outbound::awaiting_approval_outbound(&record, &decided);
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
                next.outbound = crate::github_outbound::awaiting_approval_outbound(&record, &decided);
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
            Decision::Split { parts } => self.intake_split(&item, &record, parts, next, decided, &asked, now).await,
            Decision::Wontfix { reason, evidence, duplicate_of } => {
                let why = match duplicate_of {
                    Some(of) => format!("wontfix: duplicate of {of} -- {}", evidence.trim()),
                    None => format!("wontfix: {} -- {}", reason.as_str().replace('_', " "), evidence.trim()),
                };
                next.stage = IntakeStage::Wontfix;
                next.questions.clear();
                next.decision = Some(decided.clone());
                next.outbound = crate::github_outbound::awaiting_approval_outbound(&record, &decided);
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

    /// Execute a complete plan as one generated workflow.  Its expand node
    /// materialises every internal child, roots start at once, dependants
    /// remain scheduled, and the workflow owns integration through one PR.
    async fn intake_expand_plan(self: &Arc<Self>, caller: &Caller, item: &Task) -> Result<Task> {
        if item.parent_task_id.is_some() {
            return Err(FactoryError::BadRequest(
                "automatic decomposition is one level deep; finish this child as a bounded task".into(),
            ));
        }
        let record = open_record(item)?.clone();
        let triage = record.triage.clone().ok_or_else(|| {
            FactoryError::BadRequest("an executable plan needs an assessment".into())
        })?;
        intake::validate_plan(&triage.assessment.split)
            .map_err(|error| FactoryError::BadRequest(format!("the executable plan: {error}")))?;
        let parts = intake::split_parts(&record, &triage.assessment.split)
            .map_err(FactoryError::BadRequest)?;
        let workflow = self
            .start_decomposition_workflow(item, &parts, &triage.assessment.routing, caller)
            .await?;
        let children = self
            .store
            .list(&TaskFilter { parent_task_id: Some(item.id.clone()), ..Default::default() })
            .await?;
        // `#235`: which part workflow every part ran through, by name.
        let through = match &triage.assessment.routing.workflow {
            Some(id) => match self.workflow_definition(id).await {
                Ok(template) => format!(", every part through part workflow {}", template.name),
                Err(_) => format!(", every part through part workflow {id}"),
            },
            None => String::new(),
        };
        let result = format!(
            "expanded into {} tasks in workflow {}{through}: {}",
            children.len(),
            workflow.id,
            children.iter().map(|task| format!("{} ({})", task.title, task.id)).collect::<Vec<_>>().join("; ")
        );
        let now = Utc::now();
        let decided = DecisionRecord {
            decision: Decision::Split { parts: parts.clone() },
            by: caller.describe(),
            at: now,
            workflow_run: Some(workflow.id.clone()),
            parts: children.iter().map(|task| task.id.clone()).collect(),
            part_workflow: triage.assessment.routing.workflow.clone(),
        };
        let mut next = record;
        next.stage = IntakeStage::Split;
        next.questions.clear();
        next.decision = Some(decided.clone());
        next.outbound = crate::github_outbound::awaiting_approval_outbound(&next, &decided);
        let task = self
            .write_intake(
                &item.id,
                next,
                TaskPatch {
                    status: Some(TaskStatus::Done),
                    result: Some(result.clone()),
                    ..Default::default()
                },
            )
            .await?;
        let asked = Asked::new(caller, None);
        self.entry(
            &task.id,
            asked.entry(
                "intake_plan_expanded",
                format!("{result} ({})", asked.words()),
                serde_json::json!({ "decision": decided, "parts": parts }),
            ),
        )
        .await;

        Ok(task)
    }

    /// A `split` decision: each part an intake item of its own in the item's
    /// scope, made dependencies first, and the item closed as `Done` naming
    /// them.
    #[allow(clippy::too_many_arguments)]
    async fn intake_split(
        &self,
        item: &Task,
        record: &Intake,
        parts: &[intake::SplitPart],
        mut next: Intake,
        mut decided: DecisionRecord,
        asked: &Asked,
        now: chrono::DateTime<Utc>,
    ) -> Result<Task> {
        let parts = intake::split_parts(record, parts).map_err(FactoryError::BadRequest)?;
        let ordered = intake::split_order(&parts).expect("split_parts refuses a cycle");
        let total = ordered.len();
        let mut made: std::collections::BTreeMap<String, Task> = std::collections::BTreeMap::new();
        for (k, part) in ordered.into_iter().enumerate() {
            let mut text = part.instructions.clone();
            if let Some(acceptance) = &part.acceptance {
                text.push_str(&format!("\n\nDone when: {acceptance}"));
            }
            if !part.depends_on.is_empty() {
                // Dependencies are made first, so each one is there.
                let after: Vec<String> = part
                    .depends_on
                    .iter()
                    .filter_map(|d| made.get(d))
                    .map(|t| format!("{} (intake item {})", t.title, t.id))
                    .collect();
                text.push_str(&format!("\n\nComes after: {}.", after.join("; ")));
            }
            text.push_str(&format!(
                "\n\n---\nPart {} of {total} split from intake item {} ({}). The original request, for context:\n\n{}",
                k + 1,
                item.id,
                item.title,
                item.instructions.trim(),
            ));
            let mut labels = item.labels.clone();
            labels.insert(intake::PARENT_LABEL.into(), item.id.clone());
            labels.insert(intake::PART_LABEL.into(), part.id.clone());
            let child = self
                .create_intake_task(
                    NewTask {
                        title: part.title.clone(),
                        instructions: text,
                        scope: Some(item.scope.clone()),
                        labels,
                        ..Default::default()
                    },
                    Intake {
                        stage: IntakeStage::Received,
                        source: record.source.clone(),
                        requester: record.requester.clone(),
                        received_at: now,
                        triage: None,
                        triage_task: None,
                        questions: Vec::new(),
                        decision: None,
                        candidates: Vec::new(),
                        // A confirmed report's evidence trail, and the fast
                        // lane, carry over to every part (`#170`) -- a split
                        // never happens while it is still `possible`
                        // (`check_decision`), so this is only ever `None`,
                        // `confirmed` or `dismissed`.
                        security: record.security.clone(),
                        // A part is a fresh intake item: nothing has been
                        // decided or published for it yet, whatever the
                        // parent's own outbound state was.
                        outbound: None,
                    },
                )
                .await?;
            self.entry(
                &child.id,
                asked.entry(
                    "intake_received",
                    format!("split from intake item {} as part {} of {total} {}", item.id, k + 1, asked.words()),
                    serde_json::json!({ "parent": item.id, "part": part.id, "depends_on": part.depends_on }),
                ),
            )
            .await;
            made.insert(part.id.clone(), child);
        }
        let children: Vec<&Task> = parts.iter().filter_map(|p| made.get(&p.id)).collect();
        decided.parts = children.iter().map(|t| t.id.clone()).collect();
        let result = format!(
            "split into {} intake items: {}",
            children.len(),
            children.iter().map(|t| format!("{} ({})", t.title, t.id)).collect::<Vec<_>>().join("; ")
        );
        next.stage = IntakeStage::Split;
        next.questions.clear();
        next.decision = Some(decided.clone());
        let patch =
            TaskPatch { status: Some(TaskStatus::Done), result: Some(result.clone()), ..Default::default() };
        let task = self.write_intake(&item.id, next, patch).await?;
        self.entry(
            &task.id,
            asked.entry(
                "intake_split",
                format!("{result} ({})", asked.words()),
                serde_json::json!({ "decision": decided, "parts": parts }),
            ),
        )
        .await;
        Ok(task)
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

    /// `Request::IntakeFlagSecurity`: flag an item still in the gate as a
    /// possible security report. `intake.assess` reach, the same as
    /// `IntakeAssess` (`access.rs`) -- flagging only adds scrutiny, so it is
    /// open to whoever could assess the item at all.
    pub(crate) async fn intake_flag_security(&self, caller: &Caller, id: &str, reason: &str) -> Result<Task> {
        let item = self.require(id).await?;
        let record = open_record(&item)?.clone();
        let by = caller.describe();
        let flag = intake::flag_security(record.security.as_deref(), reason, &by, Utc::now())
            .map_err(FactoryError::BadRequest)?;
        let mut next = record;
        next.security = Some(Box::new(flag));
        let task = self.write_intake(&item.id, next, TaskPatch::default()).await?;
        let asked = Asked::new(caller, None);
        self.entry(
            &task.id,
            asked.entry(
                "intake_security_flagged",
                format!("flagged as a possible security report {}: {}", asked.words(), reason.trim()),
                serde_json::json!({ "reason": reason }),
            ),
        )
        .await;
        Ok(task)
    }

    /// `Request::IntakeSecurity`: a person confirms or dismisses a possible
    /// security report. `Needs::Owner` (`access.rs`) already refused anyone
    /// else before this runs.
    pub(crate) async fn intake_security_decision(
        &self,
        caller: &Caller,
        id: &str,
        verdict: intake::SecurityVerdict,
        evidence: &str,
    ) -> Result<Task> {
        let item = self.require(id).await?;
        let record = open_record(&item)?.clone();
        let by = caller.describe();
        let flag = intake::decide_security(&record, verdict, evidence, &by, Utc::now())
            .map_err(FactoryError::BadRequest)?;
        let mut next = record;
        next.security = Some(Box::new(flag.clone()));
        let task = self.write_intake(&item.id, next, TaskPatch::default()).await?;
        let asked = Asked::new(caller, None);
        let (kind, verb) = match verdict {
            intake::SecurityVerdict::Confirm => ("intake_security_confirmed", "confirmed"),
            intake::SecurityVerdict::Dismiss => ("intake_security_dismissed", "dismissed"),
        };
        let evidence_note = flag.evidence.as_deref().map(|e| format!(": {e}")).unwrap_or_default();
        self.entry(
            &task.id,
            asked.entry(
                kind,
                format!("security report {verb} {}{evidence_note}", asked.words()),
                serde_json::json!({ "verdict": verdict, "evidence": flag.evidence }),
            ),
        )
        .await;
        Ok(task)
    }

    /// `Request::IntakeSecurityReports`: every confirmed security report
    /// over `scope`'s subtree (every scope, when `scope` is `None`), read
    /// live off the store -- including an item that has since left intake.
    /// The L4 fact `#157`'s reporting clock will read; the same subtree
    /// resolution `intake_board` uses.
    pub(crate) async fn confirmed_security_reports(
        &self,
        scope: Option<&str>,
    ) -> Result<Vec<intake::ConfirmedSecurityReport>> {
        let snapshot = self.factory_snapshot();
        let members: Option<BTreeSet<String>> = match scope {
            None => None,
            Some(name) => {
                let (asked, subtree) = factory_core::config::subtree_scopes(&snapshot, Some(name))?;
                let mut members: BTreeSet<String> = subtree.into_iter().map(|s| s.name).collect();
                if let Some(asked) = asked {
                    members.insert(asked.name);
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
        reports.sort_by(|a, b| a.awareness_at.cmp(&b.awareness_at).then(a.item.cmp(&b.item)));
        Ok(reports)
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

    pub(crate) async fn find_workflow(&self, scope: &str, wanted: &str) -> Result<factory_core::WorkflowDefinition> {
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
    /// `pub(crate)`: `github_outbound.rs` (`#171`) writes through it too,
    /// rather than duplicate the store-write and event-publish it does.
    pub(crate) async fn write_intake(&self, id: &str, record: Intake, mut patch: TaskPatch) -> Result<Task> {
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
                estimate_words(triage),
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
/// Whether an assessment's split is an executable plan rather than a
/// proposal: `owns`/`estimate_seconds` are the marker. Its
/// `routing.workflow`, if any, is then the part workflow (`#235`).
fn plan_shaped(assessment: &intake::Assessment) -> bool {
    assessment
        .split
        .iter()
        .any(|part| !part.owns.is_empty() || part.estimate_seconds.is_some())
}

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

/// `45m-2h (p10-p90 of 12 completed bugfix tasks in factory, last 90 days)`
/// -- a triage's estimate with its basis (`#168`), for the two places a
/// verdict is journaled in plain words.
fn estimate_words(triage: &intake::Triage) -> String {
    match (&triage.estimate, &triage.estimate_basis) {
        (Some(e), Some(basis)) => format!("{} ({})", e.describe(), basis.describe()),
        (Some(e), None) => e.describe(),
        (None, _) => "no estimate".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use factory_core::adapter::{AgentRuntime, StartRequest};
    use factory_core::config::{Config, DaemonConfig, Factory, Instance, Scope};
    use factory_core::intake::{
        Assessment, Axis, AxisCheck, DuplicateCandidate, DuplicateKind, DuplicateMatch, DuplicateVerdict, Level,
        NextActionKind, Routing, SecurityState, SecurityVerdict, WontfixReason,
    };
    use factory_core::protocol::{Payload, Request, Response};
    use factory_core::role::Role;
    use factory_core::run::RunStatus;
    use factory_core::task::{SessionRef, TaskReport};
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
            max_sessions: None,
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            intake: Default::default(),
            dependencies: Default::default(),
            environments: Vec::new(),
            renewals: Vec::new(),
        }
    }

    fn engine() -> Arc<Engine> {
        engine_with_scopes(vec![scope("demo"), scope("web")])
    }

    /// The same, with the caller's own scopes rather than the two plain ones
    /// -- for a test that needs a declared agent of its own.
    fn engine_with_scopes(scopes: Vec<Scope>) -> Arc<Engine> {
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
            scopes,
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
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

    /// The same, but backed by a real file rather than `:memory:` -- so a
    /// second call against the same `path` sees what the first one wrote,
    /// the shape a daemon restart takes (`#170`'s awareness-time test).
    fn engine_at(path: &std::path::Path, scopes: Vec<Scope>) -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-intake-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig { power_assertion: false, default_agent: "shell".into(), ..DaemonConfig::default() },
            roles: Default::default(),
            dashboard: None,
            policies: Default::default(),
            quality: Default::default(),
            scope: None,
            scopes,
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let mut registry = Registry::with_builtins();
        registry.add_runtime(Arc::new(QuietRuntime), "test");
        Arc::new(Engine::new(
            Factory { root, config },
            registry,
            Arc::new(SqliteStore::open(path).unwrap()),
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
            routing: Routing { scope: scope.into(), agent: Some("shell".into()), ..Default::default() },
            summary: "bounded".into(),
            questions: vec![],
            split: vec![],
            duplicates: vec![],
            checks: vec![],
            areas: vec![],
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
        let why = refused(
            engine
                .handle_request(Request::TaskRun { override_wait: false, id: item.id.clone(), reason: None, continue_run: false })
                .await,
        );
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
    async fn a_public_receipt_cannot_claim_trusted_github_provenance() {
        let engine = engine();
        let item = engine
            .intake_add(
                &Caller::Owner,
                NewIntake {
                    title: "Pretend issue".into(),
                    source: Some(SourceKind::Github),
                    reference: Some("https://github.com/example/repo/issues/1".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let record = item.intake.unwrap();
        assert_eq!(record.source.kind, SourceKind::Cli);
        assert_eq!(record.source.reference.as_deref(), Some("https://github.com/example/repo/issues/1"));
    }

    #[tokio::test]
    async fn provider_or_received_at_without_a_relay_source_is_refused_including_a_bare_github_claim() {
        let engine = engine();
        for new in [
            NewIntake { title: "x".into(), provider: Some("apple-mail".into()), ..Default::default() },
            NewIntake { title: "x".into(), received_at: Some(Utc::now()), ..Default::default() },
            NewIntake {
                title: "x".into(),
                source: Some(SourceKind::Github),
                reference: Some("https://github.com/example/repo/issues/1".into()),
                provider: Some("apple-mail".into()),
                ..Default::default()
            },
        ] {
            let why = engine.intake_add(&Caller::Owner, new).await.unwrap_err().to_string();
            assert!(why.contains("only apply to a relayed"), "{why}");
        }
        // The daemon keeps serving after every refusal.
        add(&engine, "still fine").await;
    }

    fn email_relay(title: &str, reference: &str, requester: &str, received_at: DateTime<Utc>) -> NewIntake {
        NewIntake {
            title: title.into(),
            instructions: "please look at this".into(),
            scope: Some("demo".into()),
            source: Some(SourceKind::Email),
            provider: Some("apple-mail".into()),
            reference: Some(reference.into()),
            requester: Some(requester.into()),
            received_at: Some(received_at),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn an_owner_relayed_email_item_carries_its_own_provenance_and_who_relayed_it() {
        let engine = engine();
        let at = Utc::now() - chrono::Duration::hours(2);
        let item = engine.intake_add(&Caller::Owner, email_relay("Invoice question", "<abc@x>", "a@b.c", at)).await.unwrap();
        let record = item.intake.unwrap();
        assert_eq!(record.source.kind, SourceKind::Email);
        assert_eq!(record.source.provider.as_deref(), Some("apple-mail"));
        assert_eq!(record.source.reference.as_deref(), Some("<abc@x>"));
        assert_eq!(record.source.relayed_by.as_deref(), Some("the owner"));
        assert_eq!(record.requester, "a@b.c", "the sender, never folded into a 'via' suffix");
        assert_eq!(record.received_at, at);
        assert!(kinds(&engine, &item.id).await.contains(&"intake_received".to_string()));
    }

    #[tokio::test]
    async fn an_agent_relayed_chat_item_carries_its_own_provenance_and_who_relayed_it() {
        let engine = engine();
        let at = Utc::now() - chrono::Duration::minutes(5);
        let new = NewIntake {
            title: "Can Factory look at this?".into(),
            source: Some(SourceKind::Chat),
            provider: Some("imessage".into()),
            reference: Some("imsg-42".into()),
            requester: Some("+15551234567".into()),
            received_at: Some(at),
            ..Default::default()
        };
        let item = engine.intake_add(&worker(None), new).await.unwrap();
        let record = item.intake.unwrap();
        assert_eq!(record.source.kind, SourceKind::Chat);
        assert_eq!(record.source.provider.as_deref(), Some("imessage"));
        assert_eq!(record.source.reference.as_deref(), Some("imsg-42"));
        assert_eq!(record.source.relayed_by.as_deref(), Some("w (worker) in demo"));
        assert_eq!(record.requester, "+15551234567");
        assert_eq!(record.received_at, at);
    }

    #[tokio::test]
    async fn a_relayed_items_received_at_defaults_to_now_when_absent() {
        let engine = engine();
        let before = Utc::now();
        let new = NewIntake {
            title: "x".into(),
            source: Some(SourceKind::Email),
            reference: Some("<no-time@x>".into()),
            requester: Some("a@b.c".into()),
            ..Default::default()
        };
        let item = engine.intake_add(&Caller::Owner, new).await.unwrap();
        let record = item.intake.unwrap();
        assert!(record.received_at >= before, "defaulted to now");
    }

    #[tokio::test]
    async fn a_relay_missing_its_message_id_is_refused() {
        let engine = engine();
        let new = NewIntake {
            title: "x".into(),
            source: Some(SourceKind::Email),
            requester: Some("a@b.c".into()),
            ..Default::default()
        };
        let why = engine.intake_add(&Caller::Owner, new).await.unwrap_err().to_string();
        assert!(why.contains("message id"), "{why}");
        add(&engine, "still fine").await;
    }

    #[tokio::test]
    async fn a_relay_missing_its_requester_is_refused() {
        let engine = engine();
        let new =
            NewIntake { title: "x".into(), source: Some(SourceKind::Chat), reference: Some("m1".into()), ..Default::default() };
        let why = engine.intake_add(&Caller::Owner, new).await.unwrap_err().to_string();
        assert!(why.contains("--requester"), "{why}");
        add(&engine, "still fine").await;
    }

    #[tokio::test]
    async fn a_relay_with_a_future_received_at_is_refused() {
        let engine = engine();
        let future = Utc::now() + chrono::Duration::days(1);
        let why = engine
            .intake_add(&Caller::Owner, email_relay("x", "<future@x>", "a@b.c", future))
            .await
            .unwrap_err()
            .to_string();
        assert!(why.contains("future"), "{why}");
        add(&engine, "still fine").await;
    }

    /// `#167`: a replay with the same `(kind, provider, reference)` returns
    /// the existing item, never a second one, and writes no second
    /// `intake_received` entry.
    #[tokio::test]
    async fn replaying_a_relayed_receipt_returns_the_same_item_with_no_second_receipt() {
        let engine = engine();
        let at = Utc::now() - chrono::Duration::hours(1);
        let first = engine.intake_add(&Caller::Owner, email_relay("Invoice question", "<abc@x>", "a@b.c", at)).await.unwrap();
        let second = engine.intake_add(&Caller::Owner, email_relay("Invoice question", "<abc@x>", "a@b.c", at)).await.unwrap();
        assert_eq!(first.id, second.id);
        let receipts = kinds(&engine, &first.id).await.into_iter().filter(|k| k == "intake_received").count();
        assert_eq!(receipts, 1, "the replay wrote nothing new");
        let all = engine.store.list(&TaskFilter::default()).await.unwrap();
        let emails = all.iter().filter(|t| t.intake.as_ref().is_some_and(|i| i.source.kind == SourceKind::Email)).count();
        assert_eq!(emails, 1, "exactly one task exists for the identity");
    }

    /// `#171`'s outbound record is GitHub-only (`awaiting_approval_outbound`
    /// gates on `source.kind == Github`) -- a relayed email or chat item
    /// must never get one, whatever it is decided. `#167` never sends
    /// anywhere but Factory itself, so this holds by construction; asserted
    /// here so a later change to that gate cannot silently widen it.
    #[tokio::test]
    async fn a_relayed_item_is_decided_ready_but_never_gets_outbound_state() {
        let engine = engine();
        let at = Utc::now() - chrono::Duration::hours(1);
        let item = engine.intake_add(&Caller::Owner, email_relay("Invoice question", "<outbound@x>", "a@b.c", at)).await.unwrap();
        let released =
            engine.intake_assess(&Caller::Owner, &item.id, assessment("demo"), true).await.unwrap();
        let record = released.intake.unwrap();
        assert_eq!(record.decision.as_ref().map(|d| &d.decision), Some(&Decision::Ready { run: false }));
        assert!(record.outbound.is_none(), "an email item is never published to GitHub");
    }

    /// The atomicity guarantee itself (`#167`, the same shape as
    /// `capacity_ten_concurrent_dispatches_against_a_limit_of_one_never_open_more_than_one_run`):
    /// two identical relays racing each other, however they interleave,
    /// never mint more than one item.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_concurrent_identical_relays_create_exactly_one_item() {
        let engine = engine();
        let at = Utc::now() - chrono::Duration::minutes(1);
        let mut handles = Vec::new();
        for _ in 0..2 {
            let engine = engine.clone();
            let new = email_relay("Race", "<race@x>", "a@b.c", at);
            handles.push(tokio::spawn(async move { engine.intake_add(&Caller::Owner, new).await.unwrap() }));
        }
        let mut ids = Vec::new();
        for h in handles {
            ids.push(h.await.unwrap().id);
        }
        assert_eq!(ids[0], ids[1], "both callers land on the same item");
        let all = engine.store.list(&TaskFilter::default()).await.unwrap();
        let matching = all
            .iter()
            .filter(|t| {
                t.intake.as_ref().is_some_and(|i| i.source.reference.as_deref() == Some("<race@x>"))
            })
            .count();
        assert_eq!(matching, 1, "never more than one item for the identity, however the calls interleaved");
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
        // `#168`: the category is a real field now, not only a label -- so
        // `#117`'s reference classes and `#118`'s control plan can both read
        // it -- and `Task.estimate` carries the full range the complexity
        // table gave, not only its midpoint.
        assert_eq!(released.category.as_deref(), Some("bugfix"));
        let estimate = released.estimate.as_ref().expect("the full range, not only estimate_seconds");
        assert_eq!((estimate.time.low, estimate.time.expected, estimate.time.high), (45 * 60, (45 * 60 + 2 * 3600) / 2, 2 * 3600));
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
        // A scope with nothing behind it yet falls back to the complexity
        // table, and says so.
        assert_eq!(data["triage"]["estimate_basis"]["source"], "complexity_table");
        assert_eq!(data["triage"]["estimate_basis"]["fallback_reason"], "insufficient evidence (0 of 5)");

        // And now it is ordinary work: dispatch goes ahead rather than
        // holding it. (It then fails on the test scope not being a git
        // repository -- a released item keeps a task's default worktree.)
        engine.start_run(&item.id, Trigger::Manual).await;
        assert!(!kinds(&engine, &item.id).await.contains(&"intake_held".to_string()));
        let t = engine.require(&item.id).await.unwrap();
        assert!(t.error.as_deref().unwrap_or("").contains("git"), "{:?} {:?}", t.status, t.error);
    }

    #[tokio::test]
    async fn an_assessors_padded_category_is_trimmed_everywhere_it_lands() {
        // `validate` only checks the trimmed form (`intake::validate`'s own
        // `category.trim()`), so a padded category must be normalized before
        // it reaches the reference-class sample match, `Task.category` and
        // the `category` label -- `control_plan::effective_category` trims
        // the *task* side, and an untrimmed release would never match it.
        let engine = engine();
        let item = add(&engine, "Broken link").await;
        let mut a = assessment("web");
        a.category = "  bugfix  ".into();
        let released = engine.intake_assess(&Caller::Owner, &item.id, a, true).await.unwrap();
        assert_eq!(released.category.as_deref(), Some("bugfix"));
        assert_eq!(released.labels["category"], "bugfix");
    }

    /// A completed `bugfix` task in `scope`, dispatched and reported done at
    /// once -- `QuietRuntime` has no usage to answer, so cost stays unknown;
    /// only wall time (`#168`'s time dimension) is exercised here.
    async fn done_bugfix_task(engine: &Arc<Engine>, scope: &str, title: &str) {
        let task = engine
            .create(NewTask {
                title: title.into(),
                instructions: "true".into(),
                scope: Some(scope.into()),
                agent: Some("shell".into()),
                category: Some("bugfix".into()),
                worktree: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        engine.start_run(&task.id, Trigger::Manual).await;
        let run = engine.store.active_run(&task.id).await.unwrap().expect("dispatched");
        engine
            .report(
                &task.id,
                TaskReport {
                    artifacts: Vec::new(),
                    status: Some(RunStatus::Done),
                    message: None,
                    result: Some("fixed".into()),
                    send_to: None,
                    error: None,
                    token: run.token.clone(),
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_scope_with_five_completed_bugfix_tasks_estimates_from_them_and_sets_the_category() {
        let engine = engine();
        for n in 0..5 {
            done_bugfix_task(&engine, "web", &format!("fixed thing {n}")).await;
        }
        let item = add(&engine, "Another broken link").await;
        let released = engine.intake_assess(&Caller::Owner, &item.id, assessment("web"), true).await.unwrap();
        assert_eq!(released.category.as_deref(), Some("bugfix"));
        let estimate = released.estimate.as_ref().expect("five samples clear the minimum");
        assert_eq!(released.estimate_seconds, Some(estimate.time.expected));

        let entries = engine.store.entries(&item.id, 200).await.unwrap();
        let verdict = entries.iter().find(|e| e.kind == TRIAGE_VERDICT_KIND).expect("the verdict is journaled");
        let basis = &verdict.data.as_ref().unwrap()["triage"]["estimate_basis"];
        assert_eq!(basis["source"], "reference_class");
        assert_eq!(basis["scope"], "web");
        assert_eq!(basis["category"], "bugfix");
        assert_eq!(basis["time_samples"], 5);
        assert!(basis.get("fallback_reason").is_some(), "cost is unmeasured here, so the fallback names it");
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
                    session: Default::default(),
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
                    expand: None,
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
    async fn a_triage_run_must_be_able_to_report_its_own_result() {
        // A declared agent holding a role with no `task.report` -- `triager`
        // is exactly that role -- must be refused before any run starts.
        let mut demo = scope("demo");
        demo.agents = vec![serde_yaml_ng::from_str("name: gatekeeper\nharness: shell\nrole: triager\n").unwrap()];
        let engine = engine_with_scopes(vec![demo, scope("web")]);
        let item = add(&engine, "Broken link").await;

        let why = engine.intake_triage(&Caller::Owner, &item.id, Some("gatekeeper".into())).await.unwrap_err();
        assert!(why.to_string().contains("task.report"), "{why}");
        assert!(engine.require(&item.id).await.unwrap().intake.unwrap().triage_task.is_none(), "nothing was started");

        // A bare adapter name with no declared role defaults to worker, which
        // may report -- unaffected by the scope's other declared agent.
        assert!(engine.intake_triage(&Caller::Owner, &item.id, Some("shell".into())).await.is_ok());
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

    async fn add_referencing(engine: &Arc<Engine>, title: &str, reference: &str) -> Task {
        engine
            .intake_add(
                &Caller::Owner,
                NewIntake {
                    title: title.into(),
                    instructions: "see the linked issue".into(),
                    scope: Some("demo".into()),
                    reference: Some(reference.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn receipt_and_triage_both_search_and_a_confirmed_candidate_blocks_release() {
        let engine = engine();
        let first = add_referencing(&engine, "Checkout crashes on coupon", "https://github.com/acme/shop/issues/42").await;
        let second =
            add_referencing(&engine, "Coupon code crash at checkout", "https://github.com/acme/shop/issues/42").await;

        // Receipt already searched: the second item found the first by its
        // shared GitHub reference.
        let record = engine.require(&second.id).await.unwrap().intake.unwrap();
        assert_eq!(record.candidates.len(), 1, "{:?}", record.candidates);
        assert_eq!(record.candidates[0].reference, first.id);
        assert_eq!(record.candidates[0].kind, DuplicateKind::IntakeItem);
        assert_eq!(record.candidates[0].matched, DuplicateMatch::Source);

        // Triage searches again and lists what it found in the run's own
        // instructions.
        let triage = engine.intake_triage(&Caller::Owner, &second.id, Some("shell".into())).await.unwrap();
        assert!(triage.instructions.contains("Possible duplicates"));
        assert!(triage.instructions.contains(&first.id), "{}", triage.instructions);
        let record = engine.require(&second.id).await.unwrap().intake.unwrap();
        assert_eq!(record.candidates.len(), 1);

        // Confirming it blocks the verdict -- `--decide` never releases it.
        let mut a = assessment("demo");
        a.duplicates = vec![DuplicateCandidate {
            kind: record.candidates[0].kind,
            reference: first.id.clone(),
            title: first.title.clone(),
            evidence: "same GitHub issue, reported twice".into(),
            matched: DuplicateMatch::Source,
            score: None,
            verdict: DuplicateVerdict::Confirmed,
        }];
        let assessed = engine.intake_assess(&Caller::Owner, &second.id, a, true).await.unwrap();
        assert_eq!(assessed.status, TaskStatus::Intake, "a confirmed duplicate is never released");
        let record = assessed.intake.clone().unwrap();
        assert_eq!(record.stage, IntakeStage::NeedsInfo);
        let factory_core::intake::Verdict::NeedsInfo { blockers } = &record.triage.as_ref().unwrap().verdict else {
            panic!("expected needs-info")
        };
        assert!(blockers[0].contains("confirmed duplicate"), "{blockers:?}");

        let board = engine.intake_board(None).await.unwrap();
        let card = board.columns.needs_info.iter().find(|c| c.id == second.id).expect("still needs-info");
        assert!(
            card.next_actions.iter().any(|n| n.action == NextActionKind::CloseDuplicate
                && n.reference.as_deref() == Some(first.id.as_str())),
            "{:?}",
            card.next_actions
        );
        assert_eq!(card.candidates[0].verdict, DuplicateVerdict::Confirmed, "the board shows the answer, not raw");
    }

    #[tokio::test]
    async fn assess_refuses_to_leave_a_stored_candidate_unanswered() {
        let engine = engine();
        let first = add_referencing(&engine, "Checkout crashes on coupon", "https://github.com/acme/shop/issues/42").await;
        let second =
            add_referencing(&engine, "Coupon code crash at checkout", "https://github.com/acme/shop/issues/42").await;
        assert!(!engine.require(&second.id).await.unwrap().intake.unwrap().candidates.is_empty());

        let why = engine.intake_assess(&Caller::Owner, &second.id, assessment("demo"), false).await.unwrap_err();
        assert!(why.to_string().contains("needs a verdict"), "{why}");

        // Answering it, with evidence, goes through.
        let mut a = assessment("demo");
        a.duplicates = vec![DuplicateCandidate {
            kind: DuplicateKind::IntakeItem,
            reference: first.id.clone(),
            title: first.title.clone(),
            evidence: "not the same bug on closer reading".into(),
            matched: DuplicateMatch::Source,
            score: None,
            verdict: DuplicateVerdict::Rejected,
        }];
        assert!(engine.intake_assess(&Caller::Owner, &second.id, a, false).await.is_ok());
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

    fn too_big() -> Assessment {
        let mut a = assessment("demo");
        let scope = a.axes.iter_mut().find(|c| c.axis == Axis::Scope).unwrap();
        scope.pass = false;
        scope.evidence = "four subsystems in one item".into();
        a.complexity = 10;
        a.split = vec![
            factory_core::intake::SplitPart {
                id: "rework".into(),
                title: "Rework as a run".into(),
                instructions: "rounds on runs".into(),
                depends_on: vec!["resume".into()],
                acceptance: None,
                ..Default::default()
            },
            factory_core::intake::SplitPart {
                id: "resume".into(),
                title: "Resume mechanism".into(),
                instructions: "capture and resume sessions".into(),
                depends_on: vec![],
                acceptance: Some("cargo test resume_".into()),
                ..Default::default()
            },
        ];
        a
    }

    #[tokio::test]
    async fn a_red_item_says_what_moves_it_and_splitting_hands_its_parts_back_into_intake() {
        let engine = engine();
        let item = engine
            .intake_add(
                &Caller::Owner,
                NewIntake {
                    title: "Everything at once".into(),
                    instructions: "the whole issue".into(),
                    scope: Some("demo".into()),
                    reference: Some("https://example.test/issues/178".into()),
                    labels: [("issue".to_string(), "178".to_string())].into_iter().collect(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let back = engine.intake_assess(&Caller::Owner, &item.id, too_big(), true).await.unwrap();
        assert_eq!(back.intake.as_ref().unwrap().stage, IntakeStage::NeedsInfo, "a proposal does not split by itself");
        let board = engine.intake_board(None).await.unwrap();
        let card = &board.columns.needs_info[0];
        assert_eq!(card.next_actions[0].action, factory_core::intake::NextActionKind::Split);
        assert!(card.next_actions[0].hint.contains("Resume mechanism"), "{:?}", card.next_actions);

        let split = engine.intake_decide(&Caller::Owner, &item.id, Decision::Split { parts: vec![] }).await.unwrap();
        assert_eq!(split.status, TaskStatus::Done);
        let record = split.intake.as_ref().unwrap();
        assert_eq!(record.stage, IntakeStage::Split);
        let parts = record.decision.as_ref().unwrap().parts.clone();
        assert_eq!(parts.len(), 2);
        assert!(split.result.as_deref().unwrap().starts_with("split into 2 intake items"));
        assert!(kinds(&engine, &item.id).await.contains(&"intake_split".to_string()));

        // Written order is kept in the record; dependencies are made first.
        let rework = engine.require(&parts[0]).await.unwrap();
        let resume = engine.require(&parts[1]).await.unwrap();
        assert_eq!(rework.title, "Rework as a run");
        for part in [&rework, &resume] {
            assert_eq!(part.status, TaskStatus::Intake);
            let r = part.intake.as_ref().unwrap();
            assert_eq!(r.stage, IntakeStage::Received);
            assert_eq!(r.source.reference.as_deref(), Some("https://example.test/issues/178"));
            assert_eq!(part.labels[factory_core::intake::PARENT_LABEL], item.id);
            assert_eq!(part.labels["issue"], "178", "the parent's labels travel");
            assert!(part.instructions.contains("the whole issue"), "the original request, for context");
        }
        assert_eq!(rework.labels[factory_core::intake::PART_LABEL], "rework");
        assert!(rework.instructions.contains(&format!("Comes after: Resume mechanism (intake item {})", resume.id)));
        assert!(resume.instructions.contains("Done when: cargo test resume_"));
        assert!(resume.instructions.contains("Part 1 of 2"), "dependencies first");

        let board = engine.intake_board(None).await.unwrap();
        assert_eq!(board.split, 1);
        assert_eq!(board.columns.received.len(), 2);
        assert!(board.columns.received.iter().all(|c| c.parent.as_deref() == Some(item.id.as_str())));
        let again = engine.intake_decide(&Caller::Owner, &item.id, Decision::Split { parts: vec![] }).await;
        assert!(again.unwrap_err().to_string().contains("already left intake"));
    }

    #[tokio::test]
    async fn a_complete_plan_expands_into_related_tasks_without_child_intake() {
        let engine = engine();
        let item = add(&engine, "Build the whole subsystem").await;
        let mut plan = assessment("demo");
        plan.complexity = 10;
        plan.split = vec![
            factory_core::intake::SplitPart {
                id: "foundation".into(),
                title: "Build foundation".into(),
                instructions: "true".into(),
                acceptance: Some("foundation tests pass".into()),
                owns: vec!["core".into()],
                interface: Some("core API is available".into()),
                estimate_seconds: Some(600),
                ..Default::default()
            },
            factory_core::intake::SplitPart {
                id: "surface".into(),
                title: "Build surface".into(),
                instructions: "true".into(),
                depends_on: vec!["foundation".into()],
                acceptance: Some("surface tests pass".into()),
                owns: vec!["ui".into()],
                interface: Some("UI consumes the core API".into()),
                estimate_seconds: Some(600),
            },
        ];

        let expanded = engine.intake_assess(&Caller::Owner, &item.id, plan, true).await.unwrap();
        assert_eq!(expanded.status, TaskStatus::Done);
        assert_eq!(expanded.intake.as_ref().unwrap().stage, IntakeStage::Split);
        assert!(expanded.result.as_deref().unwrap().starts_with("expanded into 2 tasks"));

        let tasks = engine.store.list(&TaskFilter::default()).await.unwrap();
        let children: Vec<&Task> = tasks.iter().filter(|task| task.parent_task_id.as_deref() == Some(&item.id)).collect();
        assert_eq!(children.len(), 2);
        assert!(children.iter().all(|task| task.intake.is_none()), "children are executable tasks, not GitHub/intake children");
        let foundation = children.iter().find(|task| task.decomposition_part.as_deref() == Some("foundation")).unwrap();
        let surface = children.iter().find(|task| task.decomposition_part.as_deref() == Some("surface")).unwrap();
        assert_eq!(surface.depends_on, vec![foundation.id.clone()]);
        assert_eq!(surface.status, TaskStatus::Pending);
        assert_eq!(surface.runs, 0);
        engine.start_run_due(&surface.id, Trigger::Manual, Due::now()).await;
        let still_waiting = engine.require(&surface.id).await.unwrap();
        assert_eq!(still_waiting.status, TaskStatus::Pending);
        assert_eq!(still_waiting.runs, 0, "a direct run request cannot jump its dependency");
        assert!(kinds(&engine, &surface.id).await.contains(&"dependency_held".to_string()));

        // Intake dispatches the foundation in the background. Wait for its
        // admission/launch writes before changing the fixture's task mirror.
        for _ in 0..400 {
            if kinds(&engine, &foundation.id).await.contains(&"dispatched".to_string()) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(kinds(&engine, &foundation.id).await.contains(&"dispatched".to_string()));
        engine
            .store
            .update(
                &foundation.id,
                &TaskPatch { status: Some(TaskStatus::Done), ..Default::default() },
            )
            .await
            .unwrap();
        let ready = engine.dependency_ready_tasks().await.unwrap();
        assert!(ready.is_empty(), "workflow release is graph-owned, not the generic scheduler's");
        engine.sync_workflow_for_task(&foundation.id).await;
        assert!(engine.require(&surface.id).await.unwrap().after.is_none(), "the graph consumes the upstream wait");
    }

    /// A two-part plan (`surface` after `foundation`) routed to `workflow`.
    fn plan_through(workflow: &str) -> Assessment {
        let mut plan = assessment("demo");
        plan.complexity = 10;
        plan.routing.workflow = Some(workflow.into());
        plan.split = ["foundation", "surface"]
            .into_iter()
            .map(|id| factory_core::intake::SplitPart {
                id: id.into(),
                title: format!("Build {id}"),
                instructions: "true".into(),
                depends_on: if id == "surface" { vec!["foundation".into()] } else { Vec::new() },
                acceptance: Some(format!("{id} tests pass")),
                owns: vec![id.into()],
                interface: Some(format!("{id} API")),
                estimate_seconds: Some(600),
            })
            .collect();
        plan
    }

    async fn part_flow(engine: &Arc<Engine>) {
        let step = |id: &str, title: &str| WorkflowNode {
            session: Default::default(),
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title: title.into(),
                instructions: "{{part_instructions}}".into(),
                scope: Some("demo".into()),
                agent: Some("shell".into()),
                worktree: Some(id == "implement"),
                ..Default::default()
            },
            gate: None,
            exits: Vec::new(),
            expand: None,
        };
        engine
            .create_workflow(WorkflowDraft {
                name: "part-flow".into(),
                scope: "demo".into(),
                part: Some(factory_core::workflow::PartSpec::default()),
                nodes: vec![step("implement", "Implement {{part_title}}"), step("review", "Review {{part_title}}")],
                edges: vec![factory_core::workflow::WorkflowEdge {
                    id: "implement-review".into(),
                    from: "implement".into(),
                    to: "review".into(),
                }],
                ..Default::default()
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_complete_plan_can_run_every_part_through_a_part_workflow() {
        let engine = engine();
        part_flow(&engine).await;
        issue_flow(&engine).await;
        let item = add(&engine, "Build the whole subsystem").await;

        // Not a part workflow: it takes an input of its own.
        let why = engine.intake_assess(&Caller::Owner, &item.id, plan_through("issue-flow"), true).await.unwrap_err();
        assert!(why.to_string().contains("part workflow issue-flow"), "{why}");
        assert!(why.to_string().contains("takes no other input; it declares \"issue\""), "{why}");
        // Factory fills in the part's values; the plan gives none.
        let mut with_inputs = plan_through("part-flow");
        with_inputs.routing.inputs.insert("part_id".into(), "x".into());
        let why = engine.intake_assess(&Caller::Owner, &item.id, with_inputs, true).await.unwrap_err();
        assert!(why.to_string().contains("leave routing.inputs out"), "{why}");
        // A step the template does not have.
        let mut unknown_step = plan_through("part-flow");
        unknown_step.routing.agents.insert("ship".into(), "shell".into());
        let why = engine.intake_assess(&Caller::Owner, &item.id, unknown_step, true).await.unwrap_err();
        assert!(why.to_string().contains("no step \"ship\""), "{why}");
        assert!(engine.require(&item.id).await.unwrap().intake.unwrap().triage.is_none(), "nothing was written");

        let mut plan = plan_through("part-flow");
        plan.routing.agents.insert("review".into(), "shell".into());
        let expanded = engine.intake_assess(&Caller::Owner, &item.id, plan, true).await.unwrap();
        assert_eq!(expanded.intake.as_ref().unwrap().stage, IntakeStage::Split);
        let result = expanded.result.as_deref().unwrap();
        assert!(result.starts_with("expanded into 4 tasks"), "{result}");
        assert!(result.contains("every part through part workflow part-flow"), "{result}");
        let template = engine.find_workflow("demo", "part-flow").await.unwrap();
        let decision = expanded.intake.as_ref().unwrap().decision.clone().unwrap();
        assert_eq!(decision.part_workflow.as_deref(), Some(template.id.as_str()), "the decision says which template");
        assert_eq!(decision.parts.len(), 4);

        let children: Vec<Task> = engine
            .store
            .list(&TaskFilter { parent_task_id: Some(item.id.clone()), ..Default::default() })
            .await
            .unwrap();
        let mut titles: Vec<&str> = children.iter().map(|task| task.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, ["Implement Build foundation", "Implement Build surface", "Review Build foundation", "Review Build surface"]);
        assert!(children.iter().all(|task| task.intake.is_none()));
    }

    #[tokio::test]
    async fn a_part_workflow_is_never_released_as_one_item() {
        let engine = engine();
        part_flow(&engine).await;
        let item = add(&engine, "Build the whole subsystem").await;

        // An item that is not a plan cannot be routed to one.
        let mut single = assessment("demo");
        single.routing.workflow = Some("part-flow".into());
        let why = engine.intake_assess(&Caller::Owner, &item.id, single, false).await.unwrap_err();
        assert!(why.to_string().contains("part-flow is a part workflow"), "{why}");

        // A plan stored without --decide, and so with a ready verdict and
        // no expansion yet: `decide ready` would drop the plan and run the
        // template once, for no part.
        let mut plan = plan_through("part-flow");
        plan.complexity = 7;
        let stored = engine.intake_assess(&Caller::Owner, &item.id, plan, false).await.unwrap();
        assert_eq!(stored.intake.as_ref().unwrap().triage.as_ref().unwrap().verdict, Verdict::Ready);
        let why = engine.intake_decide(&Caller::Owner, &item.id, Decision::Ready { run: false }).await.unwrap_err();
        assert!(why.to_string().contains("Expand it instead"), "{why}");
        let after = engine.require(&item.id).await.unwrap();
        assert_eq!(after.status, TaskStatus::Intake, "nothing was released");
        assert!(after.intake.as_ref().unwrap().decision.is_none());
        assert!(engine.workflows.runs(None, None, 10).await.unwrap().is_empty(), "and no workflow started");
    }

    #[tokio::test]
    async fn the_triage_run_may_propose_a_split_but_not_make_one() {
        let engine = engine();
        let item = add(&engine, "Everything at once").await;
        let triage = engine.intake_triage(&Caller::Owner, &item.id, Some("shell".into())).await.unwrap();
        let run = loop {
            if let Some(run) = engine.store.active_run(&triage.id).await.unwrap() {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        let triager = worker(Some(run.id.clone()));
        let assess = Request::IntakeAssess { id: item.id.clone(), assessment: too_big(), decide: true };
        engine.authorize(&triager, &assess).await.unwrap();
        let split = Request::IntakeDecide { id: item.id.clone(), decision: Decision::Split { parts: vec![] } };
        let why = engine.authorize(&triager, &split).await.unwrap_err();
        assert!(why.to_string().contains("propose the split"), "{why}");
        engine.authorize(&Caller::Owner, &split).await.unwrap();
    }

    async fn issue_flow(engine: &Arc<Engine>) {
        engine
            .create_workflow(WorkflowDraft {
                name: "issue-flow".into(),
                scope: "demo".into(),
                inputs: vec![factory_core::workflow::WorkflowInput { name: "issue".into(), description: "number".into() }],
                nodes: vec![WorkflowNode {
                    session: Default::default(),
                    id: "fix".into(),
                    position: CanvasPoint::default(),
                    kind: WorkflowNodeKind::Task,
                    task: NewTask {
                        title: "Fix #{{issue}}".into(),
                        instructions: "true".into(),
                        scope: Some("demo".into()),
                        agent: Some("shell".into()),
                        worktree: Some(false),
                        ..Default::default()
                    },
                    gate: None,
                    exits: Vec::new(),
                    expand: None,
                }],
                ..Default::default()
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_workflow_route_carries_its_inputs_and_each_steps_agent_into_the_run() {
        let engine = engine();
        issue_flow(&engine).await;
        let item = add(&engine, "Issue 178").await;
        let routed = |inputs: &[(&str, &str)], agents: &[(&str, &str)]| {
            let mut a = assessment("demo");
            a.routing.agent = None;
            a.routing.workflow = Some("issue-flow".into());
            a.routing.inputs = inputs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            a.routing.agents = agents.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            a
        };
        // Refused at assessment, before anything is written.
        let why = engine.intake_assess(&Caller::Owner, &item.id, routed(&[], &[]), false).await.unwrap_err();
        assert!(why.to_string().contains("needs issue"), "{why}");
        let why = engine
            .intake_assess(&Caller::Owner, &item.id, routed(&[("issue", "178")], &[("review", "shell")]), false)
            .await
            .unwrap_err();
        assert!(why.to_string().contains("no step \"review\"; its steps are fix"), "{why}");
        let why = engine
            .intake_assess(&Caller::Owner, &item.id, routed(&[("issue", "178")], &[("fix", "no-such-agent")]), false)
            .await
            .unwrap_err();
        assert!(why.to_string().contains("no-such-agent"), "{why}");
        assert!(engine.require(&item.id).await.unwrap().intake.unwrap().triage.is_none());

        let released = engine
            .intake_assess(&Caller::Owner, &item.id, routed(&[("issue", "178")], &[("fix", "codex")]), true)
            .await
            .unwrap();
        assert_eq!(released.status, TaskStatus::Done, "released by starting the workflow");
        let run_id = released.intake.as_ref().unwrap().decision.as_ref().unwrap().workflow_run.clone().unwrap();
        let run = engine.workflow_run(&run_id).await.unwrap();
        assert_eq!(run.inputs["issue"], "178");
        let fix = run.definition.nodes.iter().find(|n| n.id == "fix").unwrap();
        assert_eq!(fix.task.title, "Fix #178");
        assert_eq!(fix.task.agent.as_deref(), Some("codex"), "the route's agent, not the definition's");
        let stored = engine.find_workflow("demo", "issue-flow").await.unwrap();
        assert_eq!(stored.nodes[0].task.agent.as_deref(), Some("shell"), "the definition is untouched");
    }

    #[tokio::test]
    async fn the_board_lists_every_route_with_agents_and_workflows() {
        let engine = engine();
        issue_flow(&engine).await;
        let board = engine.intake_board(None).await.unwrap();
        let names: Vec<&str> = board.routes.iter().map(|r| r.scope.as_str()).collect();
        assert_eq!(names, vec!["demo", "web"]);
        let demo = &board.routes[0];
        assert_eq!(demo.default_agent.as_deref(), Some("shell"));
        assert!(demo.agents.iter().any(|a| a.name == "shell"));
        assert_eq!(demo.workflows.len(), 1);
        let flow = &demo.workflows[0];
        assert_eq!(flow.inputs[0].name, "issue");
        assert_eq!(flow.steps[0].id, "fix");
        assert_eq!(flow.steps[0].agent.as_deref(), Some("shell"));
        assert!(board.routes[1].workflows.is_empty());
    }

    // ------------------------------------------------------ definitions of ready (#169)

    /// Write `<root>/.factory/intake/<name>.yaml`, creating the directory
    /// the first time. The daemon re-reads this on every board, triage and
    /// assess request -- no watcher, no cache -- so a test can rewrite it
    /// between two calls and see the difference right away.
    fn write_ready(engine: &Engine, name: &str, body: &str) {
        let dir = engine.factory_snapshot().root.join(".factory/intake");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.yaml")), body).unwrap();
    }

    #[tokio::test]
    async fn intake_assess_refuses_an_assessment_missing_a_required_check() {
        let engine = engine();
        write_ready(
            &engine,
            "ready",
            "checks:\n  - id: threat-model\n    pass_condition: names a threat model\n",
        );
        let item = add(&engine, "Ship the new endpoint").await;
        let why = engine.intake_assess(&Caller::Owner, &item.id, assessment("demo"), false).await.unwrap_err();
        assert!(why.to_string().contains("threat-model"), "{why}");
        assert!(why.to_string().contains("not assessed"), "{why}");

        let mut a = assessment("demo");
        a.checks = vec![factory_core::intake::CheckResult {
            id: "threat-model".into(),
            pass: true,
            evidence: "documented in the PR".into(),
        }];
        let released = engine.intake_assess(&Caller::Owner, &item.id, a, true).await.unwrap();
        assert_eq!(released.status, TaskStatus::Pending, "the extra check satisfied, released as usual");
    }

    #[tokio::test]
    async fn the_triage_instructions_list_the_scopes_effective_checks() {
        let engine = engine();
        write_ready(
            &engine,
            "ready",
            "checks:\n  - id: threat-model\n    pass_condition: names a threat model\n",
        );
        let item = add(&engine, "Ship the new endpoint").await;
        let triage = engine.intake_triage(&Caller::Owner, &item.id, Some("shell".into())).await.unwrap();
        assert!(triage.instructions.contains("threat-model"), "{}", triage.instructions);
        assert!(triage.instructions.contains("names a threat model"), "{}", triage.instructions);
    }

    #[tokio::test]
    async fn rewriting_the_definition_file_changes_the_next_evaluation_without_a_restart() {
        let engine = engine();
        let first = add(&engine, "First item").await;
        let released = engine.intake_assess(&Caller::Owner, &first.id, assessment("demo"), true).await.unwrap();
        assert_eq!(released.status, TaskStatus::Pending, "no definition file yet: today's seven axes only");

        write_ready(&engine, "ready", "max_complexity: 2\n");
        let second = add(&engine, "Second item").await;
        // `--decide` only refuses releasing a needs-info verdict as ready;
        // recording the assessment itself still succeeds, and the tighter
        // cap shows up in the verdict it computed, no restart needed.
        let recorded = engine.intake_assess(&Caller::Owner, &second.id, assessment("demo"), false).await.unwrap();
        let Verdict::NeedsInfo { blockers } = recorded.intake.unwrap().triage.unwrap().verdict else {
            panic!("complexity 3 over a cap of 2 should need info")
        };
        assert!(blockers.iter().any(|b| b.contains("over demo's own limit of 2")), "{blockers:?}");
    }

    #[tokio::test]
    async fn a_malformed_definition_fails_closed_with_a_blocker_naming_the_scope() {
        let engine = engine();
        write_ready(&engine, "ready", "checks: [unclosed");
        let item = add(&engine, "Anything at all").await;
        let why = engine.intake_assess(&Caller::Owner, &item.id, assessment("demo"), false).await;
        // `validate` refuses nothing extra here (no checks were declared,
        // since the file never parsed), so the assessment itself validates;
        // `evaluate` is where the fail-closed blocker actually shows, via
        // `--decide`.
        assert!(why.is_ok(), "{why:?}");
        let decided =
            engine.intake_assess(&Caller::Owner, &item.id, assessment("demo"), true).await.unwrap();
        let record = decided.intake.unwrap();
        let blockers = match record.triage.unwrap().verdict {
            Verdict::NeedsInfo { blockers } => blockers,
            Verdict::Ready => panic!("an unreadable definition must never be silently ready"),
        };
        assert!(
            blockers.iter().any(|b| b.contains("definition of ready for demo could not be read")),
            "{blockers:?}"
        );
    }

    // -- the security fast lane (`#170`) -------------------------------

    #[tokio::test]
    async fn flag_confirm_gates_release_and_both_are_journaled() {
        let engine = engine();
        let item = add(&engine, "Possible RCE").await;
        let flagged = engine.intake_flag_security(&Caller::Owner, &item.id, "looks like an injection").await.unwrap();
        let flag = flagged.intake.as_ref().unwrap().security.as_ref().expect("flagged");
        assert_eq!(flag.state, SecurityState::Possible);
        assert_eq!(flag.flagged_by, "the owner");
        assert!(kinds(&engine, &item.id).await.contains(&"intake_security_flagged".to_string()));

        // Assessed ready, but still refused while the report is only
        // `possible` -- a person has to look first.
        engine.intake_assess(&Caller::Owner, &item.id, assessment("demo"), false).await.unwrap();
        let why = engine
            .intake_decide(&Caller::Owner, &item.id, Decision::Ready { run: false })
            .await
            .unwrap_err()
            .to_string();
        assert!(why.contains("confirm or dismiss"), "{why}");

        let confirmed = engine
            .intake_security_decision(&Caller::Owner, &item.id, SecurityVerdict::Confirm, "")
            .await
            .unwrap();
        let flag = confirmed.intake.as_ref().unwrap().security.as_ref().unwrap();
        assert_eq!(flag.state, SecurityState::Confirmed);
        assert!(kinds(&engine, &item.id).await.contains(&"intake_security_confirmed".to_string()));

        // Now it releases.
        let released = engine.intake_decide(&Caller::Owner, &item.id, Decision::Ready { run: false }).await.unwrap();
        assert_eq!(released.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn dismissing_without_evidence_is_refused_and_with_it_is_journaled() {
        let engine = engine();
        let item = add(&engine, "False alarm").await;
        engine.intake_flag_security(&Caller::Owner, &item.id, "maybe").await.unwrap();
        let why = engine
            .intake_security_decision(&Caller::Owner, &item.id, SecurityVerdict::Dismiss, "  ")
            .await
            .unwrap_err()
            .to_string();
        assert!(why.contains("no silent dismissal"), "{why}");
        assert!(!kinds(&engine, &item.id).await.contains(&"intake_security_dismissed".to_string()));

        let dismissed = engine
            .intake_security_decision(&Caller::Owner, &item.id, SecurityVerdict::Dismiss, "false positive")
            .await
            .unwrap();
        let flag = dismissed.intake.as_ref().unwrap().security.as_ref().unwrap();
        assert_eq!(flag.state, SecurityState::Dismissed);
        assert_eq!(flag.evidence.as_deref(), Some("false positive"));
        assert!(kinds(&engine, &item.id).await.contains(&"intake_security_dismissed".to_string()));

        // Dismissed is an ordinary item again: wontfix is accepted.
        let wontfix = Decision::Wontfix { reason: WontfixReason::Invalid, evidence: "confirmed false alarm".into(), duplicate_of: None };
        assert!(engine.intake_decide(&Caller::Owner, &item.id, wontfix).await.is_ok());
    }

    #[tokio::test]
    async fn an_assessment_categorised_security_report_auto_flags_and_blocks_auto_release() {
        let engine = engine();
        let item = add(&engine, "Possible RCE").await;
        let mut a = assessment("demo");
        a.category = "security-report".into();
        let assessed = engine.intake_assess(&Caller::Owner, &item.id, a.clone(), false).await.unwrap();
        let flag = assessed.intake.as_ref().unwrap().security.as_ref().expect("auto-flagged");
        assert_eq!(flag.state, SecurityState::Possible);
        assert_eq!(flag.flagged_by, "the owner");
        assert!(kinds(&engine, &item.id).await.contains(&"intake_security_flagged".to_string()));

        // `--decide` records the (re-)assessment but holds a ready verdict
        // for a person rather than releasing -- or erroring at -- a security
        // report that is only `possible`.
        let held = engine.intake_assess(&Caller::Owner, &item.id, a, true).await.unwrap();
        assert_eq!(held.status, TaskStatus::Intake, "not released");
        assert!(kinds(&engine, &item.id).await.contains(&"intake_security_held".to_string()));
        // A person deciding ready directly is still refused.
        let why = engine.intake_decide(&Caller::Owner, &item.id, Decision::Ready { run: false }).await.unwrap_err().to_string();
        assert!(why.contains("confirm or dismiss"), "{why}");

        // A second assessment never overwrites a flag already there.
        let still = engine.require(&item.id).await.unwrap();
        assert_eq!(still.intake.unwrap().security.unwrap().state, SecurityState::Possible);
    }

    #[tokio::test]
    async fn task_delete_refuses_a_task_carrying_a_confirmed_security_report() {
        let engine = engine();
        let item = add(&engine, "Possible RCE").await;
        engine.intake_flag_security(&Caller::Owner, &item.id, "looks bad").await.unwrap();
        engine.intake_security_decision(&Caller::Owner, &item.id, SecurityVerdict::Confirm, "").await.unwrap();
        let why = refused(engine.handle_request(Request::TaskDelete { id: item.id.clone() }).await);
        assert!(why.contains("CRA evidence") && why.contains(&item.id), "{why}");
        assert!(engine.require(&item.id).await.is_ok(), "never deleted");

        // An ordinary item, or one only possible or dismissed, deletes fine.
        let plain = add(&engine, "Ordinary").await;
        assert!(matches!(
            engine.handle_request(Request::TaskDelete { id: plain.id.clone() }).await,
            Response::Ok { .. }
        ));
    }

    #[tokio::test]
    async fn confirmed_security_reports_awareness_time_is_received_at_and_survives_a_restart() {
        let dir = std::env::temp_dir().join(format!("factory-intake-restart-{}", uuid::Uuid::new_v4()));
        let db = dir.join("f.sqlite");
        let scopes = || vec![scope("demo"), scope("web")];
        let received_at = Utc::now() - chrono::Duration::hours(30);
        let confirmed_at;
        let item_id;
        {
            let engine = engine_at(&db, scopes());
            let record = Intake {
                stage: IntakeStage::Received,
                source: Box::new(IntakeSource {
                    kind: SourceKind::Github,
                    reference: Some("https://github.com/o/r/issues/9".into()),
                    provider: None,
                    relayed_by: None,
                    repository: None,
                    number: None,
                    external_id: None,
                }),
                requester: "octocat".into(),
                received_at,
                triage: None,
                triage_task: None,
                questions: Vec::new(),
                decision: None,
                candidates: Vec::new(),
                security: None,
                outbound: None,
            };
            let item = engine
                .receive_intake(
                    NewTask { title: "Unauthenticated RCE".into(), scope: Some("demo".into()), ..Default::default() },
                    record,
                )
                .await
                .unwrap();
            item_id = item.id.clone();
            engine.intake_flag_security(&Caller::Owner, &item.id, "reported upstream").await.unwrap();
            let confirmed = engine
                .intake_security_decision(&Caller::Owner, &item.id, SecurityVerdict::Confirm, "")
                .await
                .unwrap();
            confirmed_at = confirmed.intake.as_ref().unwrap().security.as_ref().unwrap().decided_at.unwrap();
            assert_ne!(confirmed_at, received_at, "the test would prove nothing if these ever collided");
        }
        // A fresh engine over the same store -- what a restart looks like.
        let restarted = engine_at(&db, scopes());
        let reports = restarted.confirmed_security_reports(None).await.unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].item, item_id);
        assert_eq!(reports[0].awareness_at, received_at, "never the confirmation time");
        assert_eq!(reports[0].confirmed_at, confirmed_at);
        assert_eq!(reports[0].source.kind, SourceKind::Github);

        // Scoped the same way `intake_board` is: a sibling scope sees none.
        let none = restarted.confirmed_security_reports(Some("web")).await.unwrap();
        assert!(none.is_empty());
    }

    /// The CRA reporting clock's own reason for `ConfirmedSecurityReport.parent`
    /// (`#157`, phase 1): a confirmed report split through the real path
    /// (`intake_split`) carries `parent`, and folding every report in the
    /// chain through `reporting_clock::compute` shows exactly one item, at
    /// the *root's* `received_at` -- never a part's own (later) split time.
    #[tokio::test]
    async fn a_split_confirmed_report_carries_parent_and_the_clock_counts_it_once() {
        let engine = engine();
        let item = add(&engine, "Everything at once, and it's exploited").await;
        engine.intake_flag_security(&Caller::Owner, &item.id, "reported upstream").await.unwrap();
        let confirmed = engine
            .intake_security_decision(&Caller::Owner, &item.id, SecurityVerdict::Confirm, "verified")
            .await
            .unwrap();
        let root_awareness = confirmed.intake.as_ref().unwrap().received_at;

        engine.intake_assess(&Caller::Owner, &item.id, too_big(), true).await.unwrap();
        let split = engine.intake_decide(&Caller::Owner, &item.id, Decision::Split { parts: vec![] }).await.unwrap();
        let parts = split.intake.as_ref().unwrap().decision.as_ref().unwrap().parts.clone();
        assert_eq!(parts.len(), 2);

        let reports = engine.confirmed_security_reports(None).await.unwrap();
        assert_eq!(reports.len(), 3, "the root and both parts are each their own confirmed report");
        let root_report = reports.iter().find(|r| r.item == item.id).unwrap();
        assert_eq!(root_report.parent, None);
        for part_id in &parts {
            let part_report = reports.iter().find(|r| &r.item == part_id).unwrap();
            assert_eq!(part_report.parent.as_deref(), Some(item.id.as_str()));
            assert_ne!(
                part_report.awareness_at, root_awareness,
                "a part's own received_at is the split's time, not proof the test is meaningless"
            );
        }

        let clock = factory_core::reporting_clock::compute(&[], &reports, &[], Utc::now());
        assert_eq!(clock.items.len(), 1, "the whole chain counts once");
        assert_eq!(clock.items[0].item, factory_core::reporting_clock::ClockItemRef::Report { item: item.id.clone() });
        assert_eq!(clock.items[0].awareness_at, root_awareness, "the root's own awareness, never a part's");
    }
}
