//! Durable process definitions and their execution attempts.

use crate::task::NewTask;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CanvasPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNode {
    pub id: String,
    #[serde(default)]
    pub position: CanvasPoint,
    #[serde(default)]
    pub kind: WorkflowNodeKind,
    pub task: NewTask,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    #[default]
    Task,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEdge {
    pub id: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowDraft {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub scope: String,
    #[serde(default)]
    pub nodes: Vec<WorkflowNode>,
    #[serde(default)]
    pub edges: Vec<WorkflowEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub scope: String,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub revision: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkflowDefinition {
    pub fn from_draft(draft: WorkflowDraft) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: draft.name,
            description: draft.description,
            scope: draft.scope,
            nodes: draft.nodes,
            edges: draft.edges,
            revision: 1,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn apply(&mut self, draft: WorkflowDraft) {
        self.name = draft.name;
        self.description = draft.description;
        self.scope = draft.scope;
        self.nodes = draft.nodes;
        self.edges = draft.edges;
        self.revision += 1;
        self.updated_at = Utc::now();
    }

    /// Validate the definition and return a topological order. The order is
    /// also the accessible textual representation used by the UI.
    pub fn validate(&self) -> Result<Vec<String>, String> {
        if self.name.trim().is_empty() {
            return Err("a workflow needs a name".into());
        }
        if self.scope.trim().is_empty() {
            return Err("a workflow needs an owning scope".into());
        }
        if self.nodes.is_empty() {
            return Err("a workflow needs at least one task node".into());
        }

        let mut ids = BTreeSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                return Err("every workflow node needs an id".into());
            }
            if !ids.insert(node.id.as_str()) {
                return Err(format!("duplicate workflow node id {:?}", node.id));
            }
            if node.task.title.trim().is_empty() {
                return Err(format!("node {:?} needs a task title", node.id));
            }
            for (value, label) in [
                (node.task.estimate_seconds, "estimate"),
                (node.task.ack_timeout_seconds, "acknowledgement timeout"),
                (node.task.timeout_seconds, "run timeout"),
                (node.task.blocked_timeout_seconds, "blocked timeout"),
            ] {
                if value == Some(0) {
                    return Err(format!(
                        "node {:?} has a zero {label}; use at least one second",
                        node.id
                    ));
                }
            }
            if let Some(scope) = &node.task.scope {
                if scope != &self.scope {
                    return Err(format!(
                        "node {:?} targets scope {:?}, but this workflow belongs to {:?}",
                        node.id, scope, self.scope
                    ));
                }
            }
            if node.task.schedule.is_some() {
                return Err(format!(
                    "node {:?} has a schedule; workflow task nodes are started by dependencies",
                    node.id
                ));
            }
        }

        let mut edge_ids = BTreeSet::new();
        let mut indegree: BTreeMap<&str, usize> = ids.iter().copied().map(|id| (id, 0)).collect();
        let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for edge in &self.edges {
            if edge.id.trim().is_empty() || !edge_ids.insert(edge.id.as_str()) {
                return Err(format!("duplicate or empty workflow edge id {:?}", edge.id));
            }
            if !ids.contains(edge.from.as_str()) {
                return Err(format!(
                    "edge {:?} starts at missing node {:?}",
                    edge.id, edge.from
                ));
            }
            if !ids.contains(edge.to.as_str()) {
                return Err(format!(
                    "edge {:?} ends at missing node {:?}",
                    edge.id, edge.to
                ));
            }
            if edge.from == edge.to {
                return Err(format!(
                    "edge {:?} makes node {:?} depend on itself",
                    edge.id, edge.from
                ));
            }
            *indegree.get_mut(edge.to.as_str()).expect("validated node") += 1;
            outgoing
                .entry(edge.from.as_str())
                .or_default()
                .push(edge.to.as_str());
        }

        let mut ready: VecDeque<&str> = indegree
            .iter()
            .filter_map(|(id, degree)| (*degree == 0).then_some(*id))
            .collect();
        let mut ordered = Vec::with_capacity(ids.len());
        while let Some(id) = ready.pop_front() {
            ordered.push(id.to_string());
            for child in outgoing.get(id).into_iter().flatten() {
                let degree = indegree.get_mut(child).expect("validated node");
                *degree -= 1;
                if *degree == 0 {
                    ready.push_back(child);
                }
            }
        }
        if ordered.len() != ids.len() {
            // Named by title as well as id: "node-80e3b710-..." means nothing
            // to a person looking at the canvas, but the title on the card
            // does.
            let cyclic = indegree
                .into_iter()
                .filter_map(|(id, degree)| (degree > 0).then_some(id))
                .map(|id| {
                    let title = self
                        .nodes
                        .iter()
                        .find(|node| node.id == id)
                        .map(|node| node.task.title.as_str())
                        .unwrap_or("");
                    format!("{title:?} ({id})")
                })
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!("workflow contains a cycle involving {cyclic}"));
        }
        Ok(ordered)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowRunStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

impl WorkflowRunStatus {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeStatus {
    Unstarted,
    Pending,
    Dispatching,
    Running,
    Blocked,
    Done,
    Failed,
    Cancelled,
    Skipped,
}

impl WorkflowNodeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::Skipped
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowNodeRun {
    pub node_id: String,
    pub status: WorkflowNodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Who started a workflow run, recorded well enough to re-derive their
/// current authority later rather than trusting a snapshot of what they
/// could do the moment they clicked Run. A role can change between a run's
/// start and a node it spawns hours afterward -- `factory-daemon` re-resolves
/// this through `effective_role` at every spawn, including one recovery
/// repairs after a restart, so a role a caller has since lost stops it there
/// too. `#[serde(default)]` reads a run written before this field existed as
/// `Owner`, which is what every one of them in fact was.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowActor {
    #[default]
    Owner,
    Agent { scope: String, name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: String,
    pub scope: String,
    pub revision: u64,
    /// The immutable definition used by this attempt.
    pub definition: WorkflowDefinition,
    pub status: WorkflowRunStatus,
    pub nodes: Vec<WorkflowNodeRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_node_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Whoever asked for this attempt. Every node it spawns must never carry
    /// more authority than a fresh request from this same actor would have,
    /// even long after the click that started it.
    #[serde(default)]
    pub started_by: WorkflowActor,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkflowRun {
    pub fn new(definition: WorkflowDefinition, started_by: WorkflowActor) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            workflow_id: definition.id.clone(),
            scope: definition.scope.clone(),
            revision: definition.revision,
            nodes: definition
                .nodes
                .iter()
                .map(|node| WorkflowNodeRun {
                    node_id: node.id.clone(),
                    status: WorkflowNodeStatus::Unstarted,
                    task_id: None,
                    error: None,
                })
                .collect(),
            definition,
            status: WorkflowRunStatus::Running,
            failure_node_id: None,
            error: None,
            started_by,
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title: id.into(),
                ..Default::default()
            },
        }
    }

    fn definition(nodes: Vec<WorkflowNode>, edges: Vec<WorkflowEdge>) -> WorkflowDefinition {
        WorkflowDefinition::from_draft(WorkflowDraft {
            name: "release".into(),
            scope: "demo".into(),
            nodes,
            edges,
            ..Default::default()
        })
    }

    #[test]
    fn validates_fan_out_and_fan_in() {
        let def = definition(
            vec![node("a"), node("b"), node("c"), node("d")],
            vec![
                WorkflowEdge {
                    id: "ab".into(),
                    from: "a".into(),
                    to: "b".into(),
                },
                WorkflowEdge {
                    id: "ac".into(),
                    from: "a".into(),
                    to: "c".into(),
                },
                WorkflowEdge {
                    id: "bd".into(),
                    from: "b".into(),
                    to: "d".into(),
                },
                WorkflowEdge {
                    id: "cd".into(),
                    from: "c".into(),
                    to: "d".into(),
                },
            ],
        );
        let order = def.validate().unwrap();
        assert_eq!(order.first().map(String::as_str), Some("a"));
        assert_eq!(order.last().map(String::as_str), Some("d"));
    }

    #[test]
    fn reports_the_nodes_in_a_cycle() {
        let def = definition(
            vec![node("a"), node("b")],
            vec![
                WorkflowEdge {
                    id: "ab".into(),
                    from: "a".into(),
                    to: "b".into(),
                },
                WorkflowEdge {
                    id: "ba".into(),
                    from: "b".into(),
                    to: "a".into(),
                },
            ],
        );
        let error = def.validate().unwrap_err();
        assert!(error.contains("cycle"));
        assert!(error.contains("a"));
        assert!(error.contains("b"));
    }

    /// The cycle message names a node by its title as well as its opaque id
    /// -- "node-80e3b710-..." means nothing on its own; the card's title does.
    #[test]
    fn the_cycle_message_names_titles_not_just_ids() {
        let mut left = node("left");
        left.task.title = "Left review".into();
        let mut right = node("right");
        right.task.title = "Right review".into();
        let def = definition(
            vec![left, right],
            vec![
                WorkflowEdge {
                    id: "lr".into(),
                    from: "left".into(),
                    to: "right".into(),
                },
                WorkflowEdge {
                    id: "rl".into(),
                    from: "right".into(),
                    to: "left".into(),
                },
            ],
        );
        let error = def.validate().unwrap_err();
        assert!(error.contains("\"Left review\" (left)"), "{error}");
        assert!(error.contains("\"Right review\" (right)"), "{error}");
    }

    #[test]
    fn duplicate_node_ids_are_refused_with_the_id_named() {
        let error = definition(vec![node("a"), node("a")], vec![]).validate().unwrap_err();
        assert!(error.contains("duplicate"), "{error}");
        assert!(error.contains("\"a\""), "{error}");
    }

    #[test]
    fn an_edge_to_a_missing_node_names_the_edge_and_the_missing_endpoint() {
        let error = definition(
            vec![node("a")],
            vec![WorkflowEdge {
                id: "a-ghost".into(),
                from: "a".into(),
                to: "ghost".into(),
            }],
        )
        .validate()
        .unwrap_err();
        assert!(error.contains("a-ghost"), "{error}");
        assert!(error.contains("ghost"), "{error}");
    }

    #[test]
    fn a_self_edge_is_refused_as_depending_on_itself() {
        let error = definition(
            vec![node("a")],
            vec![WorkflowEdge {
                id: "a-a".into(),
                from: "a".into(),
                to: "a".into(),
            }],
        )
        .validate()
        .unwrap_err();
        assert!(error.contains("depend on itself"), "{error}");
        assert!(error.contains("a-a"), "{error}");
    }

    #[test]
    fn an_empty_workflow_needs_at_least_one_node() {
        let error = definition(vec![], vec![]).validate().unwrap_err();
        assert!(error.contains("at least one"), "{error}");
    }

    #[test]
    fn a_zero_timeout_is_refused_and_names_which_one() {
        let mut a = node("a");
        a.task.timeout_seconds = Some(0);
        let error = definition(vec![a], vec![]).validate().unwrap_err();
        assert!(error.contains("run timeout"), "{error}");
        assert!(error.contains("zero"), "{error}");
    }

    #[test]
    fn a_node_targeting_another_scope_is_refused() {
        let mut a = node("a");
        a.task.scope = Some("other-scope".into());
        let error = definition(vec![a], vec![]).validate().unwrap_err();
        assert!(error.contains("other-scope"), "{error}");
        assert!(error.contains("demo"), "{error}");
    }

    #[test]
    fn a_schedule_on_a_node_is_refused() {
        let mut a = node("a");
        a.task.schedule = Some(crate::task::Schedule::Cron("* * * * *".into()));
        let error = definition(vec![a], vec![]).validate().unwrap_err();
        assert!(error.contains("schedule"), "{error}");
    }
}
