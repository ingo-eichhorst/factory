//! `#351`: renaming an agent from the roster, as L4 carries it out.
//!
//! The rename is L4's because the two things that decide whether it is safe
//! and complete are: the runs on the agent, and the tasks still waiting for
//! it. The declaration and its standing sessions are L3's, reached through its
//! `Roster` port; the rules of which task moves are `factory_process::rename`.
//! Everything happens under `admission_lock`, the lock every dispatch creates
//! its run under, so no run can start on the old name halfway through.
use crate::l4_service::L4Service;
use factory_agents::dispatch::Roster;
use factory_core::task::{SlotWait, Task, TaskEntry, TaskFilter, TaskPatch};
use factory_kernel::{FactoryError, Result};
use factory_process::rename::{self, RenameReport};

impl L4Service<'_> {
    /// Rename `from` in `scope` to `to`, and move what is waiting for it.
    /// Refused, with nothing written, while a run on it is in flight, while
    /// one of its standing instances is, and while tasks waiting in the scope
    /// already name `to` -- they would be handed to this agent. Answers what
    /// was done and the instance numbers to bring up again under the new name.
    pub(crate) async fn rename_agent(
        &self,
        scope: &str,
        from: &str,
        to: &str,
        by: &str,
        source: &str,
    ) -> Result<(RenameReport, Vec<u32>)> {
        let _admission = self.state.admission_lock.lock().await;
        let l3 = self.wiring.l3();
        let check = l3.port().rename_check(scope, from, to).await?;
        let mut report = RenameReport {
            scope: check.scope.clone(),
            from: check.from.clone(),
            to: check.to.clone(),
            changed: check.changed,
            ..RenameReport::default()
        };
        if !check.changed {
            return Ok((report, Vec::new()));
        }
        let factory = self.wiring.snapshot();
        let is_here = |name: &str| factory.canonical_scope_name(name) == check.scope;

        // Chosen before the declaration changes: once it is renamed, the old
        // name resolves to a bare adapter, or to nothing.
        let tasks = self.state.store.list(&TaskFilter::default()).await?;
        let waiting: Vec<&Task> = tasks
            .iter()
            .filter(|task| rename::waits_for(task, from, is_here))
            .collect();
        let taken: Vec<&str> = tasks
            .iter()
            .filter(|task| rename::waits_for(task, &check.to, is_here))
            .map(|task| task.id.as_str())
            .collect();
        if !taken.is_empty() {
            return Err(FactoryError::BadRequest(format!(
                "{} waiting task(s) in {:?} already name {:?} ({}) and would go to this agent once it is called that; \
                 choose another name, or move them first. Nothing was changed",
                taken.len(),
                check.scope,
                check.to,
                taken.join(", ")
            )));
        }
        for run in self.state.store.active_runs().await? {
            if run.agent != from {
                continue;
            }
            let Some(task) = tasks.iter().find(|task| task.id == run.task_id) else {
                continue;
            };
            if is_here(&task.scope) {
                return Err(FactoryError::BadRequest(format!(
                    "{from:?} in {:?} has a run in flight: run {} of task {} ({}) is {}. Rename it once that run \
                     has finished; nothing was changed",
                    check.scope,
                    run.id,
                    task.id,
                    task.title,
                    run.status.as_str()
                )));
            }
        }

        let (sessions, to_start) = l3
            .port()
            .rename(&check.scope, from, &check.to, by, source)
            .await?;
        report.sessions = sessions;
        for task in waiting {
            // A task held for a free slot keeps its place in the queue: the
            // wait is the only record that its run is due, and it is the same
            // agent's, so it moves to the new name with the task. An edit to a
            // different agent drops the wait instead (`Engine::update`).
            let slot_wait = task
                .slot_wait
                .clone()
                .filter(|wait| wait.agent == from)
                .map(|wait| SlotWait {
                    agent: check.to.clone(),
                    ..wait
                });
            let patch = TaskPatch {
                agent: Some(check.to.clone()),
                slot_wait,
                ..TaskPatch::default()
            };
            if let Err(e) = self.state.store.update(&task.id, &patch).await {
                tracing::warn!(task = %task.id, "could not move a waiting task to the renamed agent: {e}");
                continue;
            }
            self.entry(
                &task.id,
                TaskEntry::new(
                    "daemon",
                    "agent_renamed",
                    format!(
                        "agent {from} renamed {} by {by}; this task now goes to {}",
                        check.to, check.to
                    ),
                ),
            )
            .await;
            self.publish_task(&task.id).await;
            report.tasks.push(task.id.clone());
        }
        report.files = self.workflow_files_naming(&check.scope, from, is_here);
        report.workflows = self.stored_workflows_naming(from, is_here).await?;
        if let Some(harness) = check.default_harness {
            report.notes.push(format!(
                "tasks created in {} without an agent name {harness}, the harness of its `agent:`, which no longer \
                 names this agent; give them --agent {}",
                check.scope, check.to
            ));
        }
        Ok((report, to_start))
    }

    /// Workflow files under the scope's own `workflows/` that still name the
    /// agent, relative to the scope's directory. Files are a person's: they
    /// are read and never rewritten, and one that is unreadable or not a
    /// workflow names nothing.
    fn workflow_files_naming(
        &self,
        scope: &str,
        name: &str,
        is_here: impl Fn(&str) -> bool + Copy,
    ) -> Vec<String> {
        let Ok(directory) = self.wiring.snapshot().scope_path(scope) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(directory.join("workflows")) else {
            return Vec::new();
        };
        let mut found: Vec<String> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .filter(|path| {
                std::fs::read_to_string(path)
                    .ok()
                    .and_then(|yaml| rename::file_nodes_naming(&yaml, name, is_here))
                    .is_some_and(|nodes| !nodes.is_empty())
            })
            .filter_map(|path| {
                path.strip_prefix(&directory)
                    .ok()
                    .map(|p| p.display().to_string())
            })
            .collect();
        found.sort();
        found
    }

    /// Stored workflow definitions, and the runs of one still going, whose
    /// nodes name the agent in this scope -- they spawn their tasks with the
    /// name they were written with.
    async fn stored_workflows_naming(
        &self,
        name: &str,
        is_here: impl Fn(&str) -> bool + Copy,
    ) -> Result<Vec<String>> {
        let mut found = Vec::new();
        for definition in self.state.workflows.definitions(None).await? {
            let nodes = rename::nodes_naming(&definition.nodes, &definition.scope, name, is_here);
            if !nodes.is_empty() {
                found.push(format!(
                    "workflow {} ({}: {})",
                    definition.name,
                    definition.id,
                    nodes.join(", ")
                ));
            }
        }
        for run in self.state.workflows.active_runs().await? {
            let nodes =
                rename::nodes_naming(&run.definition.nodes, &run.definition.scope, name, is_here);
            if !nodes.is_empty() {
                found.push(format!(
                    "workflow run {} of {} (still going: {})",
                    run.id,
                    run.definition.name,
                    nodes.join(", ")
                ));
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    //! A rename end to end through `Request::AgentRename`, over the stub runtime
    //! `agents.rs` tests standing sessions with and a config written on disk.
    use crate::agents::tests::{
        ask, config_file, instance, journal, rows, scaled_engine, session, started, task_and_run,
        TWO_SHELLS,
    };
    use crate::engine::Engine;
    use factory_core::adapter::runtime::RuntimeStatus;
    use factory_core::agent::{AgentState, MovedInstance};
    use factory_core::event::Event;
    use factory_core::protocol::{Payload, Request};
    use factory_core::role::Role;
    use factory_core::task::{NewTask, SlotWait, Task, TaskPatch, TaskStatus};
    use factory_core::workflow::{WorkflowDefinition, WorkflowDraft};
    use factory_process::rename::RenameReport;
    use std::sync::Arc;

    /// `pi` is a declared name and an adapter name at once, as in the
    /// `factory` scope itself; `helper` runs on `shell`, so a task naming
    /// `shell` names the bare adapter, not it.
    const PI_AND_HELPER: &str =
        "  - name: pi\n    harness: pi\n  - name: helper\n    harness: shell\n";

    async fn rename(
        engine: &Arc<Engine>,
        from: &str,
        to: &str,
    ) -> std::result::Result<RenameReport, String> {
        match ask(
            engine,
            Request::AgentRename {
                scope: "demo".into(),
                from: from.into(),
                to: to.into(),
            },
        )
        .await?
        {
            Payload::Renamed { rename } => Ok(rename),
            other => panic!("unexpected answer {other:?}"),
        }
    }

    async fn create(engine: &Arc<Engine>, scope: &str, agent: &str) -> Task {
        let new = NewTask {
            title: format!("{agent} in {scope}"),
            scope: Some(scope.into()),
            agent: Some(agent.into()),
            runtime: Some("stub".into()),
            ..NewTask::default()
        };
        match ask(engine, Request::TaskCreate(new)).await.unwrap() {
            Payload::Task { task } => task,
            other => panic!("unexpected answer {other:?}"),
        }
    }

    async fn task(engine: &Engine, id: &str) -> Task {
        engine.l4_service().require(id).await.unwrap()
    }

    async fn set_status(engine: &Engine, id: &str, status: TaskStatus) {
        engine
            .l4
            .store
            .update(
                id,
                &TaskPatch {
                    status: Some(status),
                    ..TaskPatch::default()
                },
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn waiting_tasks_move_with_the_agent_and_history_and_other_scopes_keep_their_name() {
        let (engine, _stub, root) = scaled_engine(PI_AND_HELPER);
        engine.shared.factory.write().unwrap().config.scopes.push(
            serde_yaml_ng::from_str(
                "name: other\npath: other\nruntime: stub\nagents:\n  - name: pi\n    harness: pi\n",
            )
            .unwrap(),
        );
        let pending = create(&engine, "demo", "pi").await;
        let scheduled = {
            let new = NewTask {
                title: "every hour".into(),
                scope: Some("demo".into()),
                agent: Some("pi".into()),
                runtime: Some("stub".into()),
                schedule: Some(factory_core::task::Schedule::Every { seconds: 3600 }),
                ..NewTask::default()
            };
            let Payload::Task { task } = ask(&engine, Request::TaskCreate(new)).await.unwrap()
            else {
                panic!()
            };
            task
        };
        let blocked = create(&engine, "demo", "pi").await;
        set_status(&engine, &blocked.id, TaskStatus::Blocked).await;
        let done = create(&engine, "demo", "pi").await;
        set_status(&engine, &done.id, TaskStatus::Done).await;
        let bare_shell = create(&engine, "demo", "shell").await;
        let elsewhere = create(&engine, "other", "pi").await;
        // Held for a free slot: its wait is the only record that its run is due.
        let held = create(&engine, "demo", "pi").await;
        let wait = SlotWait {
            agent: "pi".into(),
            scope: "demo".into(),
            trigger: factory_core::run::Trigger::Schedule,
            queued_at: chrono::Utc::now(),
            scheduled_for: None,
            since: chrono::Utc::now(),
        };
        engine
            .l4
            .store
            .update(
                &held.id,
                &TaskPatch {
                    slot_wait: Some(wait.clone()),
                    ..TaskPatch::default()
                },
            )
            .await
            .unwrap();
        let mut events = engine.shared.bus.subscribe();

        let report = rename(&engine, "pi", " scratch ").await.unwrap();

        assert!(report.changed, "{report:?}");
        assert_eq!(
            (report.from.as_str(), report.to.as_str()),
            ("pi", "scratch")
        );
        let mut moved = report.tasks.clone();
        moved.sort();
        let mut expected = vec![
            pending.id.clone(),
            scheduled.id.clone(),
            blocked.id.clone(),
            held.id.clone(),
        ];
        expected.sort();
        assert_eq!(
            moved, expected,
            "pending, scheduled and blocked move; nothing else"
        );
        for id in &expected {
            let moved = task(&engine, id).await;
            assert_eq!(moved.agent, "scratch", "{id}");
            // The point of moving them: `pi` alone would now be the bare
            // adapter, without the declaration's arguments, role or sandbox.
            let (_, adapter, declared) = engine.resolve_agent("demo", &moved.agent).unwrap();
            assert_eq!(adapter, "pi");
            assert!(
                declared.is_some(),
                "{id} reaches the declaration, not the bare adapter"
            );
            let entries = engine.l4.store.entries(id, 50).await.unwrap();
            assert!(
                entries.iter().any(|e| e.kind == "agent_renamed"
                    && e.message.contains("agent pi renamed scratch by the owner")),
                "{id}: {entries:#?}"
            );
        }
        assert_eq!(
            task(&engine, &scheduled.id).await.schedule,
            scheduled.schedule,
            "its schedule is untouched"
        );
        let still_held = task(&engine, &held.id)
            .await
            .slot_wait
            .expect("still queued for a slot");
        assert_eq!(
            (still_held.agent.as_str(), still_held.queued_at),
            ("scratch", wait.queued_at),
            "its place kept"
        );
        assert_eq!(
            task(&engine, &done.id).await.agent,
            "pi",
            "history is not rewritten"
        );
        assert_eq!(
            task(&engine, &bare_shell.id).await.agent,
            "shell",
            "a bare adapter is not this declaration"
        );
        assert_eq!(
            task(&engine, &elsewhere.id).await.agent,
            "pi",
            "another scope's pi is another agent"
        );
        assert!(
            engine.resolve_agent("demo", "pi").unwrap().2.is_none(),
            "pi is only the adapter now"
        );

        let text = std::fs::read_to_string(config_file(&root)).unwrap();
        assert!(
            text.contains("    - name: scratch\n      harness: pi\n"),
            "{text}"
        );
        loop {
            match events.recv().await.unwrap() {
                Event::AgentRenamed { scope, from, to } => {
                    assert_eq!(
                        (scope.as_str(), from.as_str(), to.as_str()),
                        ("demo", "pi", "scratch")
                    );
                    break;
                }
                _ => continue,
            }
        }
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_rename_is_refused_while_a_run_on_the_agent_is_in_flight() {
        let (engine, _stub, root) = scaled_engine(PI_AND_HELPER);
        let (running, run) = task_and_run(&engine, "demo", "pi", session("pane-1")).await;
        let waiting = create(&engine, "demo", "pi").await;
        let before = std::fs::read_to_string(config_file(&root)).unwrap();

        let e = rename(&engine, "pi", "scratch").await.unwrap_err();
        assert!(
            e.contains("run in flight") && e.contains(&run) && e.contains(&running.id),
            "{e}"
        );
        assert!(e.contains("Rename it once that run has finished"), "{e}");
        assert_eq!(
            std::fs::read_to_string(config_file(&root)).unwrap(),
            before,
            "nothing written"
        );
        assert_eq!(
            task(&engine, &waiting.id).await.agent,
            "pi",
            "nothing moved"
        );
        assert!(engine.resolve_agent("demo", "pi").unwrap().2.is_some());

        // A run on another agent in the scope holds nothing up.
        assert!(
            rename(&engine, "helper", "assistant")
                .await
                .unwrap()
                .changed
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// Renaming `helper` to `shell` would hand every task waiting on the bare
    /// `shell` adapter to it: refused, so nothing changes what it runs on.
    #[tokio::test]
    async fn a_name_waiting_tasks_already_use_is_refused() {
        let (engine, _stub, root) = scaled_engine(PI_AND_HELPER);
        let bare = create(&engine, "demo", "shell").await;
        let before = std::fs::read_to_string(config_file(&root)).unwrap();

        let e = rename(&engine, "helper", "shell").await.unwrap_err();
        assert!(
            e.contains("already name \"shell\"") && e.contains(&bare.id),
            "{e}"
        );
        assert_eq!(std::fs::read_to_string(config_file(&root)).unwrap(), before);
        assert_eq!(task(&engine, &bare.id).await.agent, "shell");

        // Once that task is done, nothing is waiting on the name any more.
        set_status(&engine, &bare.id, TaskStatus::Done).await;
        assert!(rename(&engine, "helper", "shell").await.unwrap().changed);
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn idle_standing_instances_come_back_under_the_new_name_with_their_count_role_and_stop() {
        let (engine, stub, root) = scaled_engine(TWO_SHELLS);
        engine.l3_service().reconcile_agents().await;
        assert_eq!(stub.started_ids(), ["demo/watcher", "demo/watcher#2"]);
        engine
            .l3_service()
            .set_agent_role("demo/watcher", Some(Role::foreman()))
            .await
            .unwrap();
        ask(
            &engine,
            Request::AgentStop {
                id: "demo/watcher#2".into(),
            },
        )
        .await
        .unwrap();

        let report = rename(&engine, "watcher", "lookout").await.unwrap();

        assert_eq!(
            report.sessions.moved,
            [
                MovedInstance {
                    from: "demo/watcher".into(),
                    to: "demo/lookout".into()
                },
                MovedInstance {
                    from: "demo/watcher#2".into(),
                    to: "demo/lookout#2".into()
                },
            ]
        );
        assert_eq!(
            report.sessions.started,
            ["demo/lookout"],
            "only the one that was up"
        );
        assert_eq!(
            started(&stub, 3).await,
            ["demo/watcher", "demo/watcher#2", "demo/lookout"]
        );
        assert!(
            stub.stopped_handles()
                .contains(&"stub-demo/watcher".to_string()),
            "the old session was closed"
        );
        assert_eq!(
            rows(&engine).await,
            ["demo/lookout", "demo/lookout#2"],
            "the same count, under the new name"
        );
        let mut one = instance(&engine, "demo/lookout").await;
        for _ in 0..300 {
            if one.state == AgentState::Ready {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            one = instance(&engine, "demo/lookout").await;
        }
        assert_eq!(one.state, AgentState::Ready);
        assert_eq!(
            (one.role.clone(), one.assigned_role.clone()),
            (Role::foreman(), Some(Role::foreman())),
            "its given role"
        );
        let two = instance(&engine, "demo/lookout#2").await;
        assert_eq!(
            two.state,
            AgentState::Stopped,
            "a stopped one stays stopped"
        );
        assert_eq!(two.stopped_by.as_deref(), Some("the owner"));

        let text = std::fs::read_to_string(config_file(&root)).unwrap();
        assert!(text.contains("    - name: lookout\n      harness: shell\n      lifetime: permanent\n      instances: 2\n"), "{text}");
        assert!(
            journal(&engine)
                .await
                .iter()
                .any(|(_, kind, message)| kind == "agent_renamed"
                    && message == "demo/watcher renamed demo/lookout by the owner"),
            "{:#?}",
            journal(&engine).await
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn a_rename_is_refused_while_a_standing_instance_is_working() {
        let (engine, stub, root) = scaled_engine(TWO_SHELLS);
        engine.l3_service().reconcile_agents().await;
        stub.set_status("demo/watcher#2", RuntimeStatus::Working);
        let before = std::fs::read_to_string(config_file(&root)).unwrap();

        let e = rename(&engine, "watcher", "lookout").await.unwrap_err();
        assert!(
            e.contains("standing instance in flight")
                && e.contains("demo/watcher#2: it is working"),
            "{e}"
        );
        assert_eq!(std::fs::read_to_string(config_file(&root)).unwrap(), before);
        assert_eq!(rows(&engine).await, ["demo/watcher", "demo/watcher#2"]);
        assert!(stub.stopped_handles().is_empty(), "nothing was interrupted");
        std::fs::remove_dir_all(root).ok();
    }

    /// Workflows are a person's: listed in the answer, never rewritten.
    #[tokio::test]
    async fn workflows_still_naming_the_old_agent_are_listed_and_left_alone() {
        let (engine, _stub, root) = scaled_engine(PI_AND_HELPER);
        let flow = "name: flow\nscope: demo\nnodes:\n  - id: work\n    task: { title: Work, agent: pi }\n  - id: check\n    task: { title: Check, agent: helper }\nedges: []\n";
        std::fs::create_dir_all(root.join("workflows")).unwrap();
        std::fs::write(root.join("workflows/flow.yaml"), flow).unwrap();
        std::fs::write(
            root.join("workflows/helper.yml"),
            flow.replace("agent: pi", "agent: helper"),
        )
        .unwrap();
        std::fs::write(root.join("workflows/notes.txt"), "agent: pi\n").unwrap();
        let draft: WorkflowDraft = serde_yaml_ng::from_str(flow).unwrap();
        let definition = WorkflowDefinition::from_draft(draft);
        engine
            .l4
            .workflows
            .put_definition(&definition)
            .await
            .unwrap();

        let report = rename(&engine, "pi", "scratch").await.unwrap();
        assert_eq!(report.files, ["workflows/flow.yaml"]);
        assert_eq!(
            report.workflows,
            [format!("workflow flow ({}: work)", definition.id)]
        );
        assert!(
            report
                .summary()
                .contains("still naming pi: workflows/flow.yaml, workflow flow"),
            "{}",
            report.summary()
        );
        assert_eq!(
            std::fs::read_to_string(root.join("workflows/flow.yaml")).unwrap(),
            flow,
            "not rewritten"
        );
        let stored = engine
            .l4
            .workflows
            .get_definition(&definition.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.nodes[0].task.agent.as_deref(),
            Some("pi"),
            "not rewritten"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[tokio::test]
    async fn an_unchanged_name_answers_without_writing_anything() {
        let (engine, _stub, root) = scaled_engine(PI_AND_HELPER);
        let waiting = create(&engine, "demo", "pi").await;
        let before = std::fs::read_to_string(config_file(&root)).unwrap();
        let report = rename(&engine, "pi", " pi ").await.unwrap();
        assert!(!report.changed && report.tasks.is_empty(), "{report:?}");
        assert_eq!(std::fs::read_to_string(config_file(&root)).unwrap(), before);
        assert!(task(&engine, &waiting.id).await.agent == "pi");
        let e = rename(&engine, "nobody", "x").await.unwrap_err();
        assert!(
            e.contains("no local agent declaration named \"nobody\""),
            "{e}"
        );
        std::fs::remove_dir_all(root).ok();
    }
}
