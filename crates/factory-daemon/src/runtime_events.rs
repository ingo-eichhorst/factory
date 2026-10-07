//! The runtime's pushed events and the run terminal: composition at the router.
//!
//! A runtime (an `AgentRuntime`, L3's seam) pushes status changes about sessions. A
//! session belongs to a standing agent (L3's rows) or to a task run (L4's), and the
//! occupancy chart is L4's record, so mapping a push onto its subject asks both levels
//! and is done here, beside the entry point, not inside either level's service.
use crate::engine::Engine;
use chrono::Utc;
use factory_core::adapter::runtime::{RuntimeEvent, RuntimeEventKind, RuntimeEventStream, RuntimeStatus};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::task::SessionRef;
use std::sync::Arc;

impl Engine {
    /// The same for a task run's session, so the modal's terminal can be typed
    /// into as well.
    pub async fn run_input(&self, run_id: &str, text: Option<&str>, keys: &[String]) -> Result<()> {
        let run = self
            .l4.store
            .get_run(run_id)
            .await?
            .ok_or_else(|| FactoryError::TaskNotFound(format!("run {run_id}")))?;
        let session = run.session.as_ref().ok_or_else(|| {
            FactoryError::BadRequest(format!("run {run_id} has no session to type into"))
        })?;
        let runtime = self.shared.registry.runtime(&session.runtime)?;
        if let Some(text) = text.filter(|t| !t.is_empty()) {
            runtime.send_text(session, text).await?;
        }
        if !keys.is_empty() {
            runtime.send_keys(session, keys).await?;
        }
        Ok(())
    }
    /// Ask every registered runtime whether it can push, and listen if so.
    /// One task per runtime that says yes; a runtime that answers `None`
    /// changes nothing here -- the poll in `scheduler.rs` already covers it,
    /// exactly as it did before this existed.
    pub async fn watch_runtimes(self: &Arc<Self>) {
        for runtime in self.shared.registry.runtimes() {
            let name = runtime.name().to_string();
            match runtime.watch().await {
                Ok(Some(stream)) => {
                    tracing::info!(runtime = %name, "listening for pushed status changes");
                    let engine = self.clone();
                    tokio::spawn(async move { engine.pump_runtime_events(stream).await });
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(runtime = %name, "could not start watching: {e}"),
            }
        }
    }
    async fn pump_runtime_events(self: Arc<Self>, mut stream: RuntimeEventStream) {
        while let Some(event) = stream.recv().await {
            self.on_runtime_event(event).await;
        }
        // The channel closed: the runtime stopped pushing. Nothing to do --
        // the poll never depended on this in the first place.
    }
    /// A push from a runtime, mapped onto whichever standing agent or run's
    /// session it was about, and set loose on the bus. This must never move
    /// a task or a run -- only the agent's own `factory task report` may do
    /// that -- so all this does is feed the occupancy record and publish an
    /// event; nothing here touches `self.l4.store.update` or a run's status.
    pub(crate) async fn on_runtime_event(&self, event: RuntimeEvent) {
        let Some((subject, scope, agent)) = self.subject_for_session(&event.session).await else {
            // A push about a session Factory has no record of -- already
            // stopped, or never one of ours. Nothing to attach it to.
            return;
        };
        let status = match event.kind {
            RuntimeEventKind::StatusChanged(status) => status,
            RuntimeEventKind::SessionGone => RuntimeStatus::Gone,
            // Not read yet: output is a second pass, coalesced, once there is
            // something to judge the volume against.
            RuntimeEventKind::OutputMoved => return,
        };

        // The occupancy chart's own record. This skips a sample when nothing
        // changed, which is right for a chart and wrong for what follows.
        self.l4_service().record_status(&subject, &scope, &agent, status).await;

        // Published separately, on the event itself: an agent that is
        // already `working` and keeps working is exactly the case this
        // exists for, and `record_status` above deliberately produces
        // nothing for it.
        self.shared.bus.publish(Event::AgentActivity {
            subject,
            scope,
            agent,
            status,
            at: Utc::now(),
        });
    }
    /// Which standing agent or run currently holds this session, if either does.
    /// `(subject, scope, agent name)` -- a standing agent's own id, or `run:<id>` for a
    /// task's session. L3 answers for its agents; L4's runs are asked here, at the
    /// router, so neither level reads the other's rows.
    pub(crate) async fn subject_for_session(&self, session: &SessionRef) -> Option<(String, String, String)> {
        if let Some(found) = self.l3_service().agent_subject_for_session(session).await {
            return Some(found);
        }
        if let Ok(runs) = self.l4.store.active_runs().await {
            if let Some(run) = runs.into_iter().find(|r| r.session.as_ref() == Some(session)) {
                if let Ok(Some(task)) = self.l4.store.get(&run.task_id).await {
                    return Some((format!("run:{}", run.id), task.scope, run.agent));
                }
            }
        }
        None
    }
}
