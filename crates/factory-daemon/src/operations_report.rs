//! The Operations tab's report. A **page** (D5): it composes L4's tasks, runs and journal
//! with L2's sandbox state and L3's roster, owns no state, and reads through the entry point.
//! The task commands that share its vocabulary (`skip_next`, `close_task`, `reopen_task`,
//! `answer_run`, `Asked`) stay in `operations.rs`, which is L4's.
use super::*;

impl Engine {
    /// `Request::Operations`: the whole report for `scope` -- that scope and
    /// every scope nested under it, or every scope -- over `window`, with
    /// the charts' per-step and per-run detail only when `detail` asks for
    /// it. See the module doc for what is read.
    pub(crate) async fn operations_report(
        &self,
        scope: Option<&str>,
        window: HealthWindow,
        detail: bool,
    ) -> Result<OperationsReport> {
        let now = Utc::now();
        let snapshot = self.factory_snapshot();
        // Resolved, so a bare path still finds its scope and a typo is
        // refused rather than answered with an empty report; and widened to
        // the subtree, the same reading of a scope Policy and Scenarios use.
        let scope = match scope {
            None => None,
            Some(name) => {
                let (asked, subtree) = factory_core::config::subtree_scopes(&snapshot, Some(name))?;
                let asked = asked.expect("a named scope resolves or errors");
                let mut members: BTreeSet<String> = subtree.into_iter().map(|s| s.name).collect();
                members.insert(asked.name.clone());
                Some(ScopeFilter { name: asked.name, subtree: members })
            }
        };

        let history = Duration::days(HISTORY_DAYS.max(2 * window.days()));
        let mut runs = self.l4.store.runs_between(now - history, now).await?;
        let tasks = self.l4.store.list(&TaskFilter::default()).await?;
        let agents = self.l4.store.agents().await?;
        let scope_of: BTreeMap<&str, &str> = tasks.iter().map(|t| (t.id.as_str(), t.scope.as_str())).collect();
        let in_scope = |task_id: &str| match &scope {
            None => true,
            Some(s) => scope_of.get(task_id).is_some_and(|name| s.covers(name)),
        };

        // A task that failed with no retry left stays in the queue until
        // someone deals with it, however long ago that was -- not only while
        // its run is inside the history read above. Its newest run is the
        // one the task mirrors; if none of its runs overlapped the window,
        // that run is older than all of it, so fetch that one alone.
        let seen: BTreeSet<&str> = runs.iter().map(|r| r.task_id.as_str()).collect();
        let stale: Vec<String> = tasks
            .iter()
            .filter(|t| t.has_failed() && t.bench_origin.is_none() && !seen.contains(t.id.as_str()))
            .filter(|t| in_scope(&t.id))
            .map(|t| t.id.clone())
            .collect();
        for id in stale {
            runs.extend(self.l4.store.runs(&id, 1).await?);
        }

        let mut block_reasons = BTreeMap::new();
        let mut last_progress = BTreeMap::new();
        for run in runs.iter().filter(|r| !r.status.is_terminal() && in_scope(&r.task_id)) {
            let entries = self.l4.store.run_entries(&run.id, OPEN_RUN_ENTRIES).await?;
            if let Some(last) = entries.last() {
                last_progress.insert(run.id.clone(), last.at);
            }
            if run.status == RunStatus::Blocked {
                if let Some(said) = entries.iter().rev().find(|e| e.kind == "blocked") {
                    block_reasons.insert(run.id.clone(), said.message.clone());
                }
            }
        }

        // Answers and who asked for a run are read over both health
        // windows; slots only over the lookback. One query for all three,
        // from the earlier of the two. Only the owner's answers are
        // interventions, and only an agent's run requests are left out of
        // them -- see "intervention" in `factory_core::operations`.
        let asks_from = now - Duration::days(2 * window.days());
        let missed_from = now - Duration::hours(MISSED_LOOKBACK_HOURS);
        let journal = self
            .l4.store
            .entries_of_kinds(&["schedule_skipped", ANSWER_KIND, RUN_REQUESTED_KIND], asks_from.min(missed_from))
            .await?;
        let mut skipped = Vec::new();
        let mut answers = Vec::new();
        let mut agent_runs = BTreeSet::new();
        for (task_id, entry) in journal.into_iter().filter(|(t, _)| in_scope(t)) {
            match entry.kind.as_str() {
                ANSWER_KIND if entry.at > asks_from && entry.source == "owner" => answers.push(entry.at),
                RUN_REQUESTED_KIND if entry.at > asks_from && entry.source == "agent" => {
                    let queued = entry.data.as_ref().and_then(|d| d.get("queued_at")).cloned();
                    if let Some(at) = queued.and_then(|v| serde_json::from_value::<DateTime<Utc>>(v).ok()) {
                        agent_runs.insert((task_id, at));
                    }
                }
                "schedule_skipped" if entry.at > missed_from => {
                    if let Some(slots) = skipped_slots(task_id, &entry) {
                        skipped.push(slots);
                    }
                }
                _ => {}
            }
        }


        // Every scope's own `max_sessions` (`#179`), root included --
        // discovery already folds the root's own `scope:` block into
        // `config.scopes` (`discovery::apply`), so this alone is every
        // scope, the same list `reconcile_agents` walks. A scope that
        // declares none is simply absent, not zero.
        let capacity = crate::facts::Facts::<factory_kernel::L4>::new(self)
            .get::<factory_kernel::ScopeCapacityFact>(&()).await?;

        let input = OperationsInput {
            now,
            tasks: &tasks,
            runs: &runs,
            agents: &agents,
            scope,
            detail,
            window,
            capacity: capacity.max_sessions,
            block_reasons,
            last_progress,
            skipped,
            signposts: Vec::new(),
            answers,
            agent_runs,
            // Two ticks: a slot the next tick is about to fire is not late
            // -- what the model's own default means, at this instance's tick.
            late_after_seconds: Some(2 * capacity.tick_seconds as i64),
            harnesses: self.l3.harness.rows(&[], snapshot.config.daemon.harness_health.repair_script.as_deref()),
            sandboxes: self.sandbox_attention(&snapshot, &tasks),
        };
        Ok(operations::report(&input))
    }

    /// Every sandboxed agent's readiness (`#234`), with the scheduled tasks
    /// that would run it -- what raises `sandbox_not_ready` the moment the
    /// provisioner knows, instead of at the due time.
    fn sandbox_attention(&self, snapshot: &factory_core::config::Factory, tasks: &[factory_core::task::Task]) -> Vec<factory_core::operations::SandboxAttention> {
        self.l2.provision
            .all()
            .into_iter()
            .map(|((scope, agent), readiness)| {
                let scheduled = tasks
                    .iter()
                    .filter(|t| t.schedule.is_some() && !t.schedule_paused && t.next_run_at.is_some())
                    .filter(|t| t.agent == agent && snapshot.canonical_scope_name(&t.scope) == scope)
                    .map(|t| (t.id.clone(), t.title.clone()))
                    .collect();
                factory_core::operations::SandboxAttention { scope, agent, readiness, scheduled }
            })
            .collect()
    }
}
