//! L3's command port, as `factory-daemon` serves it (#193 phase 6, S8a).
//!
//! `L3Port` is what a `Commands<L4, _>` wraps: the daemon's L3 service behind the traits in
//! `factory_agents::dispatch`. L4 reaches an adapter, a runtime or the harness health state
//! only through it.
use crate::l3_service::L3Service;
use factory_agents::adapter::AgentContext;
use factory_agents::dispatch::{Assignments, HarnessCommands, SessionStart, Supervision};
use factory_agents::harness::HeldTask;
use factory_agents::roster::ScopeAgent;
use factory_core::adapter::runtime::StartRequest;
use factory_kernel::{CommandPort, LaunchSpec, Result, SessionRef, L3};
use std::collections::BTreeMap;
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
        runtime
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
            .await
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
