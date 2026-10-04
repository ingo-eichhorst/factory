//! L3's immutable dispatch input, not an L4 task record or a task-state fact.
use factory_kernel::WorkflowOrigin;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Only the task fields needed to phrase a prompt and reporting instructions.
/// The producer supplies these values at dispatch; L3 cannot query or mutate
/// a process task, interpret its lifecycle, or use its scheduling methods.
///
/// Opaque legacy fields are not a typed lifecycle or scheduling API:
/// ```compile_fail
/// use factory_agents::assignment::AssignedTask;
/// let task = AssignedTask::new("t1", "title", "instructions");
/// let _ = task.status;
/// ```
/// ```compile_fail
/// use factory_agents::assignment::AssignedTask;
/// let task = AssignedTask::new("t1", "title", "instructions");
/// task.fires();
/// ```
/// ```compile_fail
/// use factory_agents::assignment::AssignedTask;
/// let task = AssignedTask::new("t1", "title", "instructions");
/// let _ = task.compatibility;
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignedTask {
    pub id: String,
    pub title: String,
    pub instructions: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow_origin: Option<WorkflowOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    /// Preserve existing plugin JSON and future producer fields without
    /// turning them into runtime APIs. There is deliberately no accessor:
    /// only serde forwards this opaque compatibility snapshot.
    #[serde(flatten)]
    compatibility: BTreeMap<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_minimal_assignment_has_no_process_defaults_or_null_optional_keys() {
        let task = AssignedTask::new("t1", "title", "instructions");
        assert_eq!(
            serde_json::to_value(task).unwrap(),
            json!({
                "id": "t1", "title": "title", "instructions": "instructions"
            })
        );
    }

    #[test]
    fn unknown_producer_fields_round_trip_without_becoming_l3_fields() {
        let wire = json!({"id":"t1", "title":"title", "instructions":"instructions",
            "status":"a-future-status", "schedule":{"new":true},
            "unknown":{"values":[null, true, 123, "text"]},
            "workflow_origin":{"workflow_id":"w", "workflow_run_id":"wr", "node_id":"n",
                "workspace":{"base_ref":"integration"}}, "parent_task_id":"parent"});
        let task: AssignedTask = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(task.parent_task_id.as_deref(), Some("parent"));
        assert_eq!(
            task.workflow_origin
                .as_ref()
                .unwrap()
                .workspace
                .as_ref()
                .unwrap()
                .base_ref,
            "integration"
        );
        assert_eq!(serde_json::to_value(task.clone()).unwrap(), wire);
        assert_eq!(serde_json::to_value(task).unwrap(), wire);
    }
}

impl AssignedTask {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        instructions: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            instructions: instructions.into(),
            workflow_origin: None,
            parent_task_id: None,
            compatibility: BTreeMap::new(),
        }
    }
}
