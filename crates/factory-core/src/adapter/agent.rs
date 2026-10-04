//! Canonical compatibility paths for L3's unchanged agent adapter seam.
//! Process-to-dispatch projection is composed outside the ladder.
pub use factory_agents::adapter::*;
pub use factory_agents::assignment::AssignedTask;

impl TryFrom<&crate::task::Task> for AssignedTask {
    type Error = crate::error::FactoryError;

    fn try_from(task: &crate::task::Task) -> crate::error::Result<Self> {
        let json = serde_json::to_value(task).map_err(|error| {
            crate::error::FactoryError::Other(anyhow::anyhow!(
                "encoding the task dispatch snapshot: {error}"
            ))
        })?;
        serde_json::from_value(json).map_err(|error| {
            crate::error::FactoryError::Other(anyhow::anyhow!(
                "projecting the task dispatch snapshot: {error}"
            ))
        })
    }
}
