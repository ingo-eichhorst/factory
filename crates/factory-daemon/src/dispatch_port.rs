//! L3's command port, as `factory-daemon` serves it (#193 phase 6, S8a).
//!
//! `L3Port` is what a `Commands<L4, _>` wraps: the daemon's L3 service behind the traits in
//! `factory_agents::dispatch`. L4 reaches an adapter, a runtime or the harness health state
//! only through it.
use crate::l2_service::L2Service;
use crate::l3_service::L3Service;
use factory_agents::adapter::AgentContext;
use factory_agents::dispatch::{Assignments, Environments, HarnessCommands, SessionStart, Supervision};
use factory_environment::provision::{Note, Notes, PrepareRequest, Prepared, Provision, Reconcile, Resolved, RunLedger};
use factory_agents::harness::HeldTask;
use factory_agents::roster::ScopeAgent;
use factory_core::adapter::runtime::StartRequest;
use factory_kernel::{CommandPort, LaunchSpec, Result, SessionRef, L3};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct L3Port<'a>(pub(crate) L3Service<'a>);

impl CommandPort for L3Port<'_> {
    type Level = L3;
}

#[async_trait::async_trait]
impl Assignments for L3Port<'_> {
    async fn launch(
        &self,
        adapter: &str,
        ctx: &AgentContext,
        resume_args: &[String],
        declared: Option<&ScopeAgent>,
    ) -> Result<LaunchSpec> {
        let agent = self.0.wiring.registry().agent(adapter)?;
        let mut launch = agent.launch_spec(ctx).await?;
        // The resume args go first: `codex resume <id>` is a subcommand, which has to
        // lead, and a flag like claude's `--resume <id>` does not mind leading either.
        if !resume_args.is_empty() {
            let mut args = resume_args.to_vec();
            args.append(&mut launch.args);
            launch.args = args;
        }
        crate::engine::append_declared_args(&mut launch, declared);
        Ok(launch)
    }

    async fn prompt(&self, adapter: &str, ctx: &AgentContext) -> Result<String> {
        self.0.wiring.registry().agent(adapter)?.prompt(ctx).await
    }

    async fn start(&self, session: SessionStart) -> Result<SessionRef> {
        let runtime = self.0.wiring.registry().runtime(&session.runtime)?;
        // A run's own id fragment is its discriminator: `start()` adopts any agent already
        // carrying the name it asks for, and reconcile can dispatch this scope/agent pair
        // again while an earlier run is still live, so two concurrent runs must never
        // resolve to the same herdr agent.
        let run_id_fragment = &session.run_id[..8.min(session.run_id.len())];
        let meta = session.meta;
        let mut started = runtime
            .start(&StartRequest {
                id: session.run_id.clone(),
                scope: session.scope.clone(),
                name: crate::agents::herdr_name(
                    &format!("factory-{}-{}", session.scope, session.agent),
                    Some(run_id_fragment),
                ),
                label: crate::engine::truncate(&session.title, 40),
                cwd: session.cwd,
                launch: session.launch,
            })
            .await?;
        // A sandbox's teardown rides on the session so `close_session` can find it.
        started.meta.extend(meta);
        Ok(started)
    }

    async fn submit(&self, runtime: &str, session: &SessionRef, prompt: &str) -> Result<()> {
        self.0.wiring.registry().runtime(runtime)?.submit(session, prompt).await
    }
}

impl HarnessCommands for L3Port<'_> {
    fn hold(&self, binary: &str, task: HeldTask) {
        self.0.hold(binary, task);
    }
    fn set_held(&self, now_held: BTreeMap<String, Vec<HeldTask>>) {
        self.0.set_held(now_held);
    }
    fn doubt(&self, binary: &str) {
        self.0.doubt(binary);
    }
    fn claim_recheck(&self, every: Duration) -> bool {
        self.0.claim_recheck(every)
    }
    fn recheck_done(&self) {
        self.0.recheck_done();
    }
    fn auto_repair(&self, harness: &str, binary: &str, enabled: bool, script: Option<String>) {
        self.0.maybe_auto_repair(harness, binary, enabled, script);
    }
}

#[async_trait::async_trait]
impl Supervision for L3Port<'_> {
    async fn supervise(&self) {
        self.0.supervise_agents().await;
    }
}

/// L2's command port, as the daemon serves it: what a `Commands<L3, _>` wraps.
pub(crate) struct L2Port<'a>(pub(crate) L2Service<'a>);

impl CommandPort for L2Port<'_> {
    type Level = factory_kernel::L2;
}

#[async_trait::async_trait]
impl Provision for L2Port<'_> {
    async fn gate(
        &self,
        key: &(String, String),
        config: &factory_environment::openshell::OpenshellConfig,
    ) -> std::result::Result<Resolved, String> {
        self.0.sandbox_gate(key, config).await
    }
    async fn prepare(&self, request: PrepareRequest<'_>, notes: &dyn Notes) -> Result<Prepared> {
        factory_environment::provision::prepare_sandbox(request, notes).await
    }
    async fn discard(&self, plan: &factory_environment::openshell::Plan) {
        Box::pin(crate::openshell::discard(plan)).await;
    }
    fn release(&self, session_meta: &BTreeMap<String, String>, task: String, run: String, notes: Arc<dyn Notes>) {
        let Some(teardown) = crate::openshell::Teardown::from_meta(session_meta) else {
            return;
        };
        // After the pane is gone, off the caller's path: every caller of `close_session`
        // writes the run's terminal status only after it returns, and a download and a
        // delete taking seconds in between would leave a run that already reported `done`
        // looking active with its pane gone. Claimed per sandbox, so a run closed twice (a
        // report racing a cancel) is torn down once.
        tokio::spawn(async move {
            let Some(_claim) = crate::openshell::Claim::take(&teardown.sandbox) else {
                return;
            };
            for note in Box::pin(crate::openshell::finish(&teardown)).await {
                notes.note(Note::new(&task, &run, note)).await;
            }
        });
    }
    async fn reconcile(&self, request: Reconcile<'_>, ledger: &dyn RunLedger, notes: &dyn Notes) {
        factory_environment::provision::reconcile_sandboxes(request, ledger, notes).await;
    }
    fn forget_preserved(&self, sessions_root: &std::path::Path, task: &str) {
        crate::openshell::remove_preserved(sessions_root, task);
    }
}

/// L3 passes the environment commands to L2 (`Commands<L3, L2Port>`).
#[async_trait::async_trait]
impl Environments for L3Port<'_> {
    async fn gate_environment(
        &self,
        key: &(String, String),
        config: &factory_environment::openshell::OpenshellConfig,
    ) -> std::result::Result<Resolved, String> {
        self.0.wiring.provision().port().gate(key, config).await
    }
    async fn prepare_environment(&self, request: PrepareRequest<'_>, notes: &dyn Notes) -> Result<Prepared> {
        self.0.wiring.provision().port().prepare(request, notes).await
    }
    async fn discard_environment(&self, plan: &factory_environment::openshell::Plan) {
        self.0.wiring.provision().port().discard(plan).await;
    }
    fn release_environment(&self, session_meta: &BTreeMap<String, String>, task: String, run: String, notes: Arc<dyn Notes>) {
        self.0.wiring.provision().port().release(session_meta, task, run, notes);
    }
    async fn reconcile_environments(&self, ledger: &dyn RunLedger, notes: &dyn Notes) {
        let factory = self.0.wiring.snapshot();
        // Only asked for current declarations and persisted cleanup records, so removing or
        // changing a declaration cannot strand an older sandbox.
        let mut configs: Vec<factory_environment::openshell::OpenshellConfig> = Vec::new();
        for name in factory.scope_names() {
            if let Ok(scope) = factory.scope(&name) {
                for agent in scope.declared_agents() {
                    if let Some(config) = agent.openshell.filter(|_| agent.sandbox == factory_core::config::Sandbox::Openshell) {
                        configs.push(config);
                    }
                }
            }
        }
        let factory_dir = factory.factory_dir();
        self.0
            .wiring
            .provision()
            .port()
            .reconcile(
                Reconcile { instance_id: &factory.config.instance.id, factory_dir: &factory_dir, configs },
                ledger,
                notes,
            )
            .await;
    }
    fn forget_preserved(&self, task: &str) {
        let sessions_root = self.0.wiring.snapshot().factory_dir().join("openshell").join("sessions");
        self.0.wiring.provision().port().forget_preserved(&sessions_root, task);
    }
}
