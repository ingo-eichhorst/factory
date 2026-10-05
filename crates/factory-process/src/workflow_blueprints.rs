//! Live immutable authored blueprint projection, owned by L4. Domain
//! validation/injection stay here; no task lifecycle is exposed in this fact.
use crate::{store::TaskStore, workflow::WorkflowDefinition, workflow_store::WorkflowStore};
use factory_kernel::{
    FactProvider, FactoryError, Provide, Result, WorkflowBlueprintFact, WorkflowBlueprintQuery, L4,
};

pub struct Provider<'a> {
    pub tasks: &'a dyn TaskStore,
    pub workflows: &'a WorkflowStore,
}
impl FactProvider for Provider<'_> {
    type Level = L4;
}

pub fn project(definition: &WorkflowDefinition) -> Result<WorkflowBlueprintFact> {
    Ok(WorkflowBlueprintFact {
        blueprint: serde_json::from_value(
            serde_json::to_value(definition).map_err(anyhow::Error::from)?,
        )
        .map_err(anyhow::Error::from)?,
        categories: definition.categories().into_iter().collect(),
    })
}
pub fn definition(fact: &WorkflowBlueprintFact) -> Result<WorkflowDefinition> {
    Ok(
        serde_json::from_value(serde_json::to_value(&fact.blueprint).map_err(anyhow::Error::from)?)
            .map_err(anyhow::Error::from)?,
    )
}
#[async_trait::async_trait]
impl Provide<WorkflowBlueprintFact> for Provider<'_> {
    type Query = WorkflowBlueprintQuery;
    type Value = Vec<WorkflowBlueprintFact>;
    type Error = FactoryError;
    async fn get(&self, query: &Self::Query) -> Result<Self::Value> {
        let definitions = match query {
            WorkflowBlueprintQuery::All => self.workflows.definitions(None).await?,
            WorkflowBlueprintQuery::Workflow(id) => vec![self
                .workflows
                .get_definition(id)
                .await?
                .ok_or_else(|| FactoryError::BadRequest(format!("no such workflow: {id}")))?],
            WorkflowBlueprintQuery::Task(id) => {
                let task = self
                    .tasks
                    .get(id)
                    .await?
                    .ok_or_else(|| FactoryError::TaskNotFound(id.clone()))?;
                vec![WorkflowDefinition::implicit(&task)]
            }
        };
        definitions.iter().map(project).collect()
    }
}
