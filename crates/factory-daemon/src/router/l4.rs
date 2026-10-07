//! Requests served by L4 Process. The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use super::misrouted;

impl Engine {
    pub(super) async fn route_l4(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            Request::Occupancy { minutes, from, to } => Ok(Payload::Occupancy {
                occupancy: self.occupancy(minutes, from, to).await?,
            }),
            Request::Production { minutes, bin, scope } => Ok(Payload::Production {
                production: self.production(minutes, bin, scope).await?,
            }),
            Request::SiteFootprint => Ok(Payload::SiteFootprint {
                footprint: self.site_footprint().await?,
            }),
            // No event of its own: every fact it reads changes through
            // `TaskUpdated`, `RunUpdated`, `TaskEntry` or `AgentUpdated`
            // already, and a viewer re-reads on those.
            Request::Operations { scope, window, detail } => Ok(Payload::Operations {
                report: Box::new(self.operations_report(scope.as_deref(), window, detail).await?),
            }),
            Request::RunScreen { id } => {
                let run = self.require_run(&id).await?;
                match self.run_screen(&run).await? {
                    Some(screen) => Ok(Payload::Screen { screen }),
                    None => Err(FactoryError::BadRequest(
                        "this run has no session to show".into(),
                    )),
                }
            }
            Request::RunInput { id, text, keys } => {
                self.run_input(&id, text.as_deref(), &keys).await?;
                Ok(Payload::Ok)
            }
            Request::RunAnswer { id, text, reason } => {
                let asked = crate::operations::Asked::new(caller, Some(reason));
                self.answer_run(&id, &text, &asked).await?;
                Ok(Payload::Ok)
            }

            Request::TaskCreate(new) => Ok(Payload::Task {
                task: self.create(new).await?,
            }),
            Request::TaskGet { id } => Ok(Payload::Task {
                task: self.require(&id).await?,
            }),
            Request::TaskList(filter) => Ok(Payload::Tasks {
                tasks: self.l4.store.list(&filter).await?,
            }),
            Request::TaskUpdate { id, patch, reason } => {
                let asked = crate::operations::Asked::new(caller, reason);
                Ok(Payload::Task {
                    task: self.update(&id, patch, Some(&asked)).await?,
                })
            }
            Request::TaskSkipNext { id, reason, slot } => {
                let asked = crate::operations::Asked::new(caller, reason);
                Ok(Payload::Task {
                    task: self.skip_next(&id, slot, &asked).await?,
                })
            }
            Request::TaskDelete { id } => {
                // A confirmed security report is CRA evidence (`#170`):
                // checked before anything else touches the run or the row,
                // whatever the task's current status or stage.
                if let Some(task) = self.l4.store.get(&id).await? {
                    if let Some(reason) = factory_core::intake::confirmed_security_delete_guard(&task) {
                        return Err(FactoryError::BadRequest(reason));
                    }
                }
                if let Some(run) = self.l4.store.active_run(&id).await? {
                    self.close_session(&run).await;
                    // Deleting the task ends the run without ever reaching
                    // `finish_run` (there is no row left to mirror), so the
                    // slot it held is freed here instead (`#179`).
                    if let Ok(Some(task)) = self.l4.store.get(&id).await {
                        self.enqueue_capacity_release(&task.scope, &run.agent);
                    }
                }
                // `#274`: released with the task, not by a periodic sweep.
                self.remove_preserved_session(&id);
                let deleted = self.l4.store.delete(&id).await?;
                if deleted {
                    self.shared.bus.publish(Event::TaskDeleted { id });
                }
                Ok(Payload::Deleted { deleted })
            }
            Request::TaskRun { id, reason, continue_run, override_wait } => {
                let task = self.require(&id).await?;
                // The gate (`#119`): an item still in intake has not been
                // released, and nothing but a decision releases it.
                if task.status == TaskStatus::Intake {
                    return Err(FactoryError::BadRequest(
                        "this task is still in intake: triage it and release it (factory intake decide) before it can run".into(),
                    ));
                }
                let blockers = self.dependency_blockers(&task).await?;
                if (task.after.is_some() || !blockers.is_empty()) && !override_wait {
                    let waiting = if task.after.is_some() {
                        self.waiting_description(&task).await?
                    } else {
                        blockers.join(", ")
                    };
                    return Err(FactoryError::BadRequest(format!(
                        "this task is waiting for: {}{}",
                        waiting,
                        if task.after.is_some() { "; use --override-wait --reason to run early" } else { "" }
                    )));
                }
                // A task is a standing intent; a run is one attempt at it. Two
                // attempts at once would race for the same working directory.
                if let Some(run) = self.l4.store.active_run(&id).await? {
                    return Err(FactoryError::BadRequest(format!(
                        "attempt {} of this task is still {}; cancel it before starting another",
                        run.attempt,
                        run.status.as_str()
                    )));
                }
                // Already waiting for a slot (`#179`): a second request would
                // only spawn a second hold attempt on the same wait, and the
                // journal would say so twice for no reason.
                if let Some(wait) = &task.slot_wait {
                    return Err(FactoryError::BadRequest(format!(
                        "this task is already waiting for a {} slot, since {}; cancel or close it, or wait for a slot to open",
                        wait.agent,
                        wait.since.to_rfc3339(),
                    )));
                }
                // `#178`: `--continue` is refused outright -- not merely
                // fallen back from -- unless the task's newest run is
                // terminal and ended on an infrastructure failure.
                // `resolve_continue` decides, per session, whether that run
                // can actually be resumed; this only decides whether asking
                // is sensible at all.
                let continue_from = if continue_run {
                    let Some(prev) = self.l4.store.runs(&id, 1).await?.into_iter().next() else {
                        return Err(FactoryError::BadRequest("this task has no previous run to continue".into()));
                    };
                    if !prev.status.is_terminal() {
                        return Err(FactoryError::BadRequest(format!(
                            "attempt {} of this task is still {}; cancel it before continuing",
                            prev.attempt,
                            prev.status.as_str()
                        )));
                    }
                    // `#274`: a blocked-timeout reclaim is not one of
                    // `FailKind::is_infrastructure`'s members -- that set
                    // also excludes a run from L5 conformance's held-failure
                    // category, which a blocked timeout should not join --
                    // so it is accepted here by name instead of widening
                    // that shared membership. Resuming after reclaim is the
                    // whole point of preserving a blocked sandbox's
                    // conversation: a person who was waiting to answer
                    // should be able to pick the same session back up.
                    let infra = match prev.fail_kind {
                        Some(kind) if kind.is_infrastructure() => {
                            kind != FailKind::DispatchFailed || prev.last_session.is_some()
                        }
                        Some(FailKind::BlockedTimeout) => true,
                        _ => false,
                    };
                    if !infra {
                        return Err(FactoryError::BadRequest(format!(
                            "attempt {} of this task ended {}, which is not an infrastructure failure; \
                             --continue is only for an ack timeout, a run timeout, a session gone, a \
                             blocked timeout, or a dispatch failure after a session had already come up",
                            prev.attempt,
                            prev.fail_kind.map_or(prev.status.as_str(), FailKind::as_str),
                        )));
                    }
                    Some(prev)
                } else {
                    None
                };
                // Who asked, written down before the run exists: the record
                // otherwise says only that a run was `manual`, which a
                // person and an agent both are (`#106`).
                // Stamped here, not inside the spawned dispatch: the queue
                // wait of a manual run starts when it was asked for.
                let due = Due::now();
                if override_wait {
                    let mut released = task.clone();
                    released.after = None;
                    if !self.dependency_blockers(&released).await?.is_empty() {
                        return Err(FactoryError::BadRequest("--override-wait overrides after, not static decomposition dependencies".into()));
                    }
                    if reason.as_deref().is_none_or(|reason| reason.trim().is_empty()) {
                        return Err(FactoryError::BadRequest("--override-wait requires a non-empty --reason".into()));
                    }
                    self.override_waiting(&task, caller, reason.as_deref().expect("checked reason")).await?;
                }
                // `queued_at` is what ties this entry to the run it starts,
                // which does not exist yet: the Operations report leaves a
                // run an agent asked for out of the interventions by it.
                let asked = crate::operations::Asked::new(caller, reason);
                let words = if let Some(prev) = &continue_from {
                    format!("{} (continuing attempt {})", asked.words(), prev.attempt)
                } else {
                    asked.words()
                };
                self.entry(
                    &id,
                    asked.entry(
                        crate::operations::RUN_REQUESTED_KIND,
                        format!("run requested {words}"),
                        serde_json::json!({ "queued_at": due.queued_at }),
                    ),
                )
                .await;
                let engine = self.clone();
                // Dispatch can take a minute: opening a pane, waiting for an
                // agent to be ready. The caller gets its answer now.
                tokio::spawn(async move {
                    match continue_from {
                        Some(prev) => engine.start_run_due_continue(&id, due, prev).await,
                        None => engine.start_run_due(&id, Trigger::Manual, due).await,
                    }
                });
                Ok(Payload::Ok)
            }
            Request::TaskCancel { id, reason, run } => {
                // Who asked is the one thing that tells a person's
                // intervention from an agent tidying up -- see `FailKind`
                // for the limit of that.
                let kind = match caller {
                    crate::access::Caller::Owner => FailKind::CancelledByPerson,
                    crate::access::Caller::Agent { .. } => FailKind::CancelledByAgent,
                };
                let run = self.cancel_task_run(&id, run.as_deref(), kind).await?;
                let asked = crate::operations::Asked::new(caller, reason);
                self.entry(
                    &id,
                    asked
                        .entry("cancel_requested", format!("cancelled {}", asked.words()), serde_json::json!({}))
                        .in_run(&run.id),
                )
                .await;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Run { run: run.redacted() })
            }
            Request::TaskClose { id, reason, duplicate_of, note } => {
                let asked = crate::operations::Asked::new(caller, note);
                let task = self.close_task(&id, reason, duplicate_of, &asked).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Task { task })
            }
            Request::TaskReopen { id, reason } => {
                let asked = crate::operations::Asked::new(caller, reason);
                let task = self.reopen_task(&id, &asked).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Task { task })
            }
            Request::TaskReport { id, report } => {
                // Feedback now carries a richer run snapshot. Keep the
                // report/verification chain off every request's future,
                // including small reads on axum's default worker stack.
                let run = Box::pin(self.report(&id, report)).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Run { run: run.redacted() })
            }
            Request::TaskTurnEnded { id, turn } => {
                self.turn_ended(&id, turn).await?;
                self.sync_workflow_for_task(&id).await;
                self.sync_bench_for_task(&id).await;
                Ok(Payload::Ok)
            }
            Request::TaskEntries { id, limit, task_only } => Ok(Payload::Entries {
                entries: if task_only {
                    self.l4.store.task_own_entries(&id, limit.unwrap_or(200)).await?
                } else {
                    self.l4.store.entries(&id, limit.unwrap_or(200)).await?
                },
            }),
            Request::TaskOutput { id, lines } => {
                let latest = self.l4.store.runs(&id, 1).await?.into_iter().next();
                Ok(Payload::Text {
                    text: match latest {
                        Some(run) => self.output(&run, lines.unwrap_or(200)).await,
                        None => String::new(),
                    },
                })
            }

            // Intake (`#119`). Every write publishes `TaskCreated` or
            // `TaskUpdated` itself; the board is a read over tasks. The
            // big ones are boxed: releasing starts a workflow and splitting
            // makes tasks, and inline they would size every request's
            // future -- enough to overflow a test thread's stack. `add`
            // joined them in `#167`: relaying grew `NewIntake` and
            // `IntakeSource` enough that the HTTP interface's own deeper
            // extractor-and-routing chain, on top of this dispatch, on top
            // of `intake_add`'s own chain, overflowed the default stack a
            // socket caller's shallower path did not.
            Request::IntakeAdd(new) => Ok(Payload::Task { task: Box::pin(self.intake_add(caller, new)).await? }),
            Request::IntakeBoard { scope } => Ok(Payload::IntakeBoard {
                board: Box::pin(self.intake_board(scope.as_deref())).await?,
            }),
            Request::IntakeTriage { id, agent } => Ok(Payload::Task {
                task: Box::pin(self.intake_triage(caller, &id, agent)).await?,
            }),
            Request::IntakeAssess { id, assessment, decide } => Ok(Payload::Task {
                task: Box::pin(self.intake_assess(caller, &id, assessment, decide)).await?,
            }),
            Request::IntakeDecide { id, decision } => Ok(Payload::Task {
                task: Box::pin(self.intake_decide(caller, &id, decision)).await?,
            }),
            Request::IntakeInfo { id, text } => Ok(Payload::Task {
                task: self.intake_info(caller, &id, &text).await?,
            }),
            Request::IntakeFlagSecurity { id, reason } => Ok(Payload::Task {
                task: self.intake_flag_security(caller, &id, &reason).await?,
            }),
            Request::IntakeSecurity { id, verdict, evidence } => Ok(Payload::Task {
                task: self.intake_security_decision(caller, &id, verdict, &evidence).await?,
            }),
            Request::IntakeSecurityReports { scope } => Ok(Payload::IntakeSecurityReports {
                reports: self.confirmed_security_reports(scope.as_deref()).await?,
            }),
            Request::IntakePublish { id } => Ok(Payload::Task {
                task: Box::pin(self.intake_publish(caller, &id)).await?,
            }),

            Request::WorkflowCreate(draft) => Ok(Payload::Workflow {
                workflow: self.create_workflow(draft).await?,
            }),
            Request::WorkflowGet { id } => Ok(Payload::Workflow {
                workflow: self.workflow_definition(&id).await?,
            }),
            Request::WorkflowList { scope } => Ok(Payload::Workflows {
                workflows: self.l4.workflows.definitions(scope.as_deref()).await?,
            }),
            Request::WorkflowUpdate { id, workflow } => Ok(Payload::Workflow {
                workflow: self.update_workflow(&id, workflow).await?,
            }),
            Request::WorkflowDelete { id } => Ok(Payload::Deleted {
                deleted: self.delete_workflow(&id).await?,
            }),
            Request::WorkflowStart { id, inputs } => Ok(Payload::WorkflowRun {
                run: Box::pin(self.start_workflow(&id, inputs, caller)).await?,
            }),
            Request::WorkflowRunGet { id } => Ok(Payload::WorkflowRun {
                run: self.workflow_run(&id).await?,
            }),
            Request::WorkflowRunList { workflow_id, scope, limit } => Ok(Payload::WorkflowRuns {
                runs: self.l4.workflows.runs(workflow_id.as_deref(), scope.as_deref(), limit.unwrap_or(50)).await?,
            }),
            Request::WorkflowRunCancel { id } => Ok(Payload::WorkflowRun {
                run: Box::pin(self.cancel_workflow(&id)).await?,
            }),
            Request::WorkflowLint { workflow, task, scope, category } => Ok(Payload::WorkflowLint {
                lint: self.workflow_lint(workflow, task, scope, category).await?,
            }),
            Request::RunAttestations { id } => Ok(Payload::Attestations {
                attestations: self.run_attestations(&id).await?,
            }),
            Request::RunProvenance { id } => Ok(Payload::RunProvenance {
                records: crate::facts::Facts::<factory_kernel::People>::new(self).get::<factory_kernel::ArtifactProvenance>(&id).await?,
            }),
            Request::RunApprove { id, reason } => Ok(Payload::Run {
                run: Box::pin(self.decide_approval(
                        caller,
                        &id,
                        factory_core::control_plan::AttestationVerdict::Pass,
                        &reason,
                    ))
                    .await?
                    .redacted(),
            }),
            Request::RunReject { id, reason } => Ok(Payload::Run {
                run: Box::pin(self.decide_approval(
                        caller,
                        &id,
                        factory_core::control_plan::AttestationVerdict::Fail,
                        &reason,
                    ))
                    .await?
                    .redacted(),
            }),
            Request::RunRework { id } => Ok(Payload::Run {
                run: Box::pin(self.accept_rework(caller, &id)).await?.redacted(),
            }),

            Request::RunList { task_id, limit } => Ok(Payload::Runs {
                runs: self
                    .l4.store
                    .runs(&task_id, limit.unwrap_or(50))
                    .await?
                    .into_iter()
                    .map(|r| r.redacted())
                    .collect(),
            }),
            Request::RunGet { id } => Ok(Payload::Run {
                run: self.require_run(&id).await?.redacted(),
            }),
            Request::RunUsage { id } => {
                let run = self.require_run(&id).await?;
                Ok(Payload::UsageSnapshots {
                    snapshots: self.l4.store.usage_snapshots(&run.id).await?,
                })
            }
            Request::TaskUsage { id } => Ok(Payload::TaskUsage {
                usage: self.l4_service().task_usage(&id).await?,
            }),
            Request::Costs { group_by, from, to, scope } => Ok(Payload::Costs {
                report: crate::facts::Facts::<factory_kernel::People>::new(self)
                    .get::<factory_kernel::CostReport>(&factory_core::usage::SpendQuery { scope, from, to, group_by, ..Default::default() })
                    .await?,
            }),
            Request::RunEntries { id, limit } => Ok(Payload::Entries {
                entries: self.l4.store.run_entries(&id, limit.unwrap_or(200)).await?,
            }),
            Request::RunOutput { id, lines } => {
                let run = self.require_run(&id).await?;
                Ok(Payload::Text {
                    text: self.output(&run, lines.unwrap_or(200)).await,
                })
            }
            other => Err(misrouted(other.level())),
        }
    }
}
