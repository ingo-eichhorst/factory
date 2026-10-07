//! What L4 needs from the levels above it, declared by L4 and supplied at wiring time (#193 phase 6, S10 part 4).
//!
//! Dispatch puts a scope's quality block into the agent's guide, and creating or updating a workflow binds its
//! reviewers to real agents. Both are answers only the levels above can compute (judging quality needs L5's evidence
//! services over every level; binding functionaries reads L5's preview). L4 does not call up for them: it holds this
//! capability, given to it when the service is built. The list is the whole of what L4 may ask above, so growing it is
//! a visible change; the daemon counts the call sites.
use crate::workflow::WorkflowDefinition;
use factory_agents::adapter::QualityAttributeContext;
use factory_kernel::Result;

#[async_trait::async_trait]
pub trait SuppliedFromAbove: Send + Sync {
    /// The scope's H-importance quality attributes, judged and reused for a short time. Empty, never an error, when
    /// nothing applies or something cannot be read: a quality profile must never stop a task from dispatching.
    async fn quality_block(&self, scope: &str) -> Vec<QualityAttributeContext>;

    /// Bind a workflow definition's reviewing nodes to the agents that will check them (the first concrete agent
    /// different from the subject's executor).
    async fn bind_functionaries(&self, definition: &mut WorkflowDefinition) -> Result<()>;
}
