//! Process projects a dispatch snapshot for the directly lower L3 seam.
use factory_agents::assignment::AssignedTask;
use factory_kernel::{FactoryError, Result};

impl TryFrom<&crate::task::Task> for AssignedTask {
    type Error = FactoryError;

    fn try_from(task: &crate::task::Task) -> Result<Self> {
        let json = serde_json::to_value(task).map_err(|error| {
            FactoryError::Other(anyhow::anyhow!(
                "encoding the task dispatch snapshot: {error}"
            ))
        })?;
        serde_json::from_value(json).map_err(|error| {
            FactoryError::Other(anyhow::anyhow!(
                "projecting the task dispatch snapshot: {error}"
            ))
        })
    }
}
