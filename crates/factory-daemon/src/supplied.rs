//! What the levels above supply to L4, and the page that serves workflow lint (#193 phase 6, S10 part 4).
//!
//! **Page (D5).** Both are compositions across levels that own no state: judging a scope's quality block needs L5's
//! evidence services over L1 to L4 facts, binding a workflow's reviewers reads L5's workflow preview over L3's roster
//! and L4's blueprints, and `workflow lint` composes the same preview with L6's authored plan inputs. L4 does not call
//! any of it up; `L4Service` is handed `SuppliedFromAbove` when it is built (`Engine::l4_service()`), and this is the
//! implementation.
use crate::engine::Engine;
use crate::facts::Port;
use factory_agents::adapter::QualityAttributeContext;
use factory_core::error::Result;
use factory_core::workflow::{WorkflowDefinition, WorkflowLint};

#[async_trait::async_trait]
impl factory_process::supplied::SuppliedFromAbove for Engine {
    async fn quality_block(&self, scope: &str) -> Vec<QualityAttributeContext> {
        self.quality_context(scope).await
    }

    /// Freeze review and approval functionaries into an immutable workflow
    /// snapshot. Scope declaration order is stable and deliberate; the first
    /// concrete agent different from the subject executor is the checker.
    async fn bind_functionaries(&self, definition: &mut WorkflowDefinition) -> Result<()> {
        factory_kernel::WorkflowTargetsFact::provider(self).bind_functionaries(definition).await
    }
}

impl Engine {
    /// `factory workflow lint`: the plan, the injection and the ordering
    /// violations for a stored workflow, a task's implicit workflow, or --
    /// with neither -- just a scope's plan for one category.
    pub(crate) async fn workflow_lint(
        &self,
        workflow: Option<String>,
        task: Option<String>,
        scope: Option<String>,
        category: Option<String>,
    ) -> Result<WorkflowLint> {
        let blueprints = factory_kernel::WorkflowBlueprintFact::provider(self);
        let preview = factory_kernel::WorkflowTargetsFact::provider(self);
        let prepared = preview.prepare(
            factory_assurance::workflow_preview::Subject { workflow, task, scope, category },
            &blueprints,
        ).await?;
        let snapshot = self.factory_snapshot();
        let intent = crate::intent::Intent::of(&snapshot);
        let mut requirements = Vec::new();
        for category in prepared.categories() {
            requirements.push(intent.plan_input(prepared.scope(), category).await);
        }
        preview.finish(prepared, requirements).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L4's handle on what is supplied from above is the page's own implementation: the block dispatch asks for is
    /// the block `quality_context` judges, and an unknown scope is the empty block, never an error.
    #[tokio::test]
    async fn l4s_quality_block_is_what_the_page_judges_and_an_unknown_scope_is_empty() {
        let (engine, _root) = crate::environments::tests::engine_with("  - name: prod\n");
        let l4 = engine.l4_service();
        for scope in ["no-such-scope", "company"] {
            assert_eq!(l4.above.quality_block(scope).await, engine.quality_context(scope).await, "{scope}");
        }
        assert!(l4.above.quality_block("no-such-scope").await.is_empty());
    }
}
