//! Durable process definitions and their execution attempts.

use crate::control_plan::{self, ControlPlan, PlanStep, RequiredStep, StepKind};
use crate::task::NewTask;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct CanvasPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkflowNode {
    pub id: String,
    #[serde(default)]
    pub position: CanvasPoint,
    #[serde(default)]
    pub kind: WorkflowNodeKind,
    /// For a `Task` node, the task it spawns. For a `Gate` node only its
    /// `title` (the card's label) and `scope` mean anything -- a gate never
    /// spawns a task.
    pub task: NewTask,
    /// What a `Gate` node checks. `None` on every `Task` node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<GateSpec>,
    /// Ordered conditional exits, checked after this task reports `done`.
    /// The first condition that holds selects its target exclusively; when
    /// none holds the node's ordinary outgoing edges remain the default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exits: Vec<WorkflowExit>,
}

/// One ordered conditional route out of a task node (`#149`). Exactly one of
/// `check` and `agent` is present. A backwards exit also carries its bounded
/// number of rounds; a forwards exit is backed by an ordinary graph edge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowExit {
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<u32>,
}

/// The old on-disk shape. Kept only at the serde boundary: definitions and
/// workflow-run snapshots written before #149 load as one `agent:` exit and
/// are serialized in the new shape the next time they are stored.
#[derive(Debug, Clone, Deserialize)]
struct WorkflowNodeWire {
    pub id: String,
    #[serde(default)]
    pub position: CanvasPoint,
    #[serde(default)]
    pub kind: WorkflowNodeKind,
    pub task: NewTask,
    #[serde(default)]
    pub gate: Option<GateSpec>,
    #[serde(default)]
    pub exits: Vec<WorkflowExit>,
    #[serde(default)]
    pub rework: Option<LegacyReworkSpec>,
}

#[derive(Debug, Clone, Deserialize)]
struct LegacyReworkSpec {
    to: String,
    max_rounds: u32,
}

impl<'de> Deserialize<'de> for WorkflowNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut wire = WorkflowNodeWire::deserialize(deserializer)?;
        if wire.exits.is_empty() {
            if let Some(rework) = wire.rework {
                wire.exits.push(WorkflowExit {
                    to: rework.to,
                    check: None,
                    agent: Some(
                        "work needs changes this node can describe for the target agent".into(),
                    ),
                    max_rounds: Some(rework.max_rounds),
                });
            }
        }
        Ok(Self {
            id: wire.id,
            position: wire.position,
            kind: wire.kind,
            task: wire.task,
            gate: wire.gate,
            exits: wire.exits,
        })
    }
}

/// A value a run is started with (`#140`), written `{{name}}` in a task
/// node's title, instructions or label values and in a gate's command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowInput {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    #[default]
    Task,
    /// A required step (`#118`): a shell command the daemon runs in the
    /// worktree of the task node it verifies, exit 0 = pass, leaving an
    /// attestation behind. Never spawns a task and never runs an agent --
    /// its status mirrors the attestations its subject's run collected.
    Gate,
    /// An independent agent judges the subject's result after deterministic
    /// gates. The verifier spawns its task; the node mirrors that evidence.
    Review,
    /// A person decides before the subject is dispatched.
    Approval,
}

/// A `Gate` node's check. Injected ones are `locked` -- the control plan put
/// them there, and an author cannot take them out -- and name the controls
/// that required them; an author may also place one by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateSpec {
    /// The step it is -- `tests`, `sbom`, `security_scan` -- and so which
    /// required step of the same name it satisfies.
    pub step: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u64>,
    /// The task node whose run this gate judges. Injected gates always name
    /// it; an authored one may leave it to [`WorkflowDefinition::gate_subject`],
    /// which follows its parents back to the one task node above it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// `<framework>/<control>` or `quality/<attribute>` for each control
    /// whose plan this gate carries out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    /// Injected by the control plan at run start.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub locked: bool,
    /// `independent` for review, `person` for approval. Absent on v1 gates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Concrete functionary frozen into a run snapshot. A missing review
    /// actor is a visible lint/execution gap, never permission to self-review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
}

/// What [`WorkflowDefinition::inject`] did for one required step of one
/// task node -- the preview `factory workflow lint` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Injection {
    /// The task node the step verifies.
    pub node_id: String,
    pub category: String,
    pub step: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
    /// The gate node that carries it out: the injected one, or the
    /// authored one that already did.
    pub gate_node_id: String,
    /// `true` when an authored gate node already satisfied the step and
    /// nothing was injected for it.
    pub satisfied_by_authored: bool,
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
    /// The category every task node is planned as unless it names its own
    /// (`#118`). Absent is the default category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// What a run of it must be started with (`#140`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<WorkflowInput>,
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
    /// See `WorkflowDraft::category`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// See `WorkflowDraft::inputs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<WorkflowInput>,
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
            category: draft.category,
            inputs: draft.inputs,
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
        self.category = draft.category;
        self.inputs = draft.inputs;
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
            if let Some(category) = &node.task.category {
                control_plan::check_category(category).map_err(|e| format!("node {:?}: {e}", node.id))?;
            }
            match (node.kind, &node.gate) {
                (WorkflowNodeKind::Task, Some(_)) => {
                    return Err(format!("node {:?} is a task node but carries a gate; make it a gate node", node.id));
                }
                (WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval, None) => {
                    return Err(format!("control node {:?} needs a gate spec naming its step", node.id));
                }
                (WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval, Some(gate)) => {
                    if !control_plan::is_name(&gate.step) {
                        return Err(format!("gate node {:?} names step {:?}, which is not a step name", node.id, gate.step));
                    }
                    // A locked gate is the plan's, and a plan may require a
                    // gate nobody gave a command -- it blocks, which is the
                    // point. An author placing one by hand says what it runs.
                    if node.kind == WorkflowNodeKind::Gate
                        && !gate.locked
                        && gate.command.as_deref().map(str::trim).unwrap_or("").is_empty()
                    {
                        return Err(format!("gate node {:?} needs a command to run", node.id));
                    }
                    if gate.timeout_seconds == Some(0) {
                        return Err(format!("gate node {:?} has a zero timeout; use at least one second", node.id));
                    }
                }
                (WorkflowNodeKind::Task, None) => {}
            }
        }
        if let Some(category) = &self.category {
            control_plan::check_category(category)?;
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
        self.validate_inputs()?;
        self.validate_exits()?;
        for node in self
            .nodes
            .iter()
            .filter(|n| n.kind == WorkflowNodeKind::Gate)
        {
            if self.gate_subject(&node.id).is_none() {
                return Err(format!(
                    "gate node {:?} ({}) must verify exactly one task node: name it as the gate's subject, \
                     or give the gate one task node (or a chain of gates from one) as its parent",
                    node.task.title, node.id
                ));
            }
        }
        Ok(ordered)
    }

    fn node(&self, id: &str) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Declared inputs have names and are declared once, and -- once a
    /// definition declares any -- every `{{name}}` in a task node names one
    /// of them, so a typo is refused here rather than reaching an agent as
    /// literal braces. A definition with no inputs leaves braces alone: it
    /// may well carry `{{.Names}}`-style text meant for some other tool.
    fn validate_inputs(&self) -> Result<(), String> {
        let mut declared = BTreeSet::new();
        for input in &self.inputs {
            if !is_input_name(&input.name) {
                return Err(format!(
                    "input {:?} is not a name: use letters, digits, `_` or `-`, starting with a letter or `_`",
                    input.name
                ));
            }
            if !declared.insert(input.name.as_str()) {
                return Err(format!("input {:?} is declared twice", input.name));
            }
        }
        if declared.is_empty() {
            return Ok(());
        }
        for node in self.nodes.iter().filter(|n| n.kind == WorkflowNodeKind::Task) {
            let texts = [&node.task.title, &node.task.instructions].into_iter().chain(node.task.labels.values());
            for text in texts {
                if let Some(unknown) = placeholders(text).into_iter().find(|name| !declared.contains(name)) {
                    return Err(format!(
                        "node {:?} uses {{{{{unknown}}}}}, but the workflow declares no input {unknown:?}",
                        node.id
                    ));
                }
            }
        }
        Ok(())
    }

    /// Exits are ordered conditions on task nodes. A forward target must be
    /// an explicit edge; a backward target must be an ancestor task and is
    /// bounded. The backwards link stays out of `edges`, keeping the graph
    /// acyclic.
    fn validate_exits(&self) -> Result<(), String> {
        for node in &self.nodes {
            if !node.exits.is_empty()
                && node.kind != WorkflowNodeKind::Task
                && !(matches!(node.kind, WorkflowNodeKind::Review | WorkflowNodeKind::Gate)
                    && node.gate.as_ref().is_some_and(|g| g.locked))
            {
                return Err(format!(
                    "gate node {:?} cannot declare exits; only a task node can",
                    node.id
                ));
            }
            for (index, exit) in node.exits.iter().enumerate() {
                let ordinal = index + 1;
                if exit.check.is_some() == exit.agent.is_some() {
                    return Err(format!(
                        "node {:?} exit {ordinal} must declare exactly one of check or agent",
                        node.id
                    ));
                }
                if exit.check.as_deref().is_some_and(|c| c.trim().is_empty())
                    || exit.agent.as_deref().is_some_and(|a| a.trim().is_empty())
                {
                    return Err(format!(
                        "node {:?} exit {ordinal} has an empty condition",
                        node.id
                    ));
            }
                let target = self.node(&exit.to).ok_or_else(|| {
                    format!(
                        "node {:?} exit {ordinal} targets missing node {:?}",
                        node.id, exit.to
                    )
                })?;
                let backwards = self.ancestors(&node.id).contains(&exit.to);
                if backwards {
                    if target.kind != WorkflowNodeKind::Task {
                        return Err(format!(
                            "node {:?} exit {ordinal} points back to gate node {:?}; name a task node",
                            node.id, exit.to
                        ));
            }
                    if exit.max_rounds.unwrap_or(0) == 0 {
                        return Err(format!(
                            "node {:?} exit {ordinal} points backward and needs max_rounds of at least one",
                            node.id
                        ));
                }
                } else {
                    if !self
                        .edges
                        .iter()
                        .any(|edge| edge.from == node.id && edge.to == exit.to)
                    {
                        return Err(format!(
                            "node {:?} exit {ordinal} points forward to {:?} without an explicit edge",
                            node.id, exit.to
                        ));
            }
                    if exit.max_rounds.is_some() {
                return Err(format!(
                            "node {:?} exit {ordinal} points forward and must not declare max_rounds",
                            node.id
                ));
            }
        }
            }
        }
        Ok(())
    }

    /// What runs again when `from` routes work back to `to`: every node
    /// on a path from the target down to `from`, both included, and
    /// everything downstream of `from` -- none of which can have started,
    /// since it all waits on `from`. In the definition's own order. Empty
    /// when `to` is not an ancestor of `from`.
    pub fn route_back_body(&self, from: &str, to: &str) -> Vec<String> {
        if !self.ancestors(from).contains(to) {
            return Vec::new();
        }
        let below_to = self.descendants(to);
        let above_from = self.ancestors(from);
        let below_from = self.descendants(from);
        let in_body = |id: &str| {
            id == to || id == from || (below_to.contains(id) && above_from.contains(id)) || below_from.contains(id)
        };
        let order = self.validate().unwrap_or_else(|_| self.nodes.iter().map(|n| n.id.clone()).collect());
        order.into_iter().filter(|id| in_body(id)).collect()
    }

    /// This definition with a run's inputs written in (`#140`): every
    /// declared input must be given, nothing undeclared may be, and each
    /// `{{name}}` in a task node's title, instructions and label values is
    /// replaced verbatim. A gate's command is left as it is -- it is a
    /// command the daemon itself runs, and a value typed at start time is
    /// not something to splice into one.
    pub fn with_inputs(&self, given: &BTreeMap<String, String>) -> Result<WorkflowDefinition, String> {
        let declared: BTreeSet<&str> = self.inputs.iter().map(|i| i.name.as_str()).collect();
        if let Some(extra) = given.keys().find(|k| !declared.contains(k.as_str())) {
            return Err(format!("this workflow takes no input {extra:?}"));
        }
        let missing: Vec<&str> = self
            .inputs
            .iter()
            .filter(|i| given.get(&i.name).is_none_or(|v| v.trim().is_empty()))
            .map(|i| i.name.as_str())
            .collect();
        if !missing.is_empty() {
            return Err(format!("a run of this workflow needs {}", missing.join(", ")));
        }
        let mut out = self.clone();
        if given.is_empty() {
            return Ok(out);
        }
        for node in out.nodes.iter_mut().filter(|n| n.kind == WorkflowNodeKind::Task) {
            node.task.title = substitute(&node.task.title, given);
            node.task.instructions = substitute(&node.task.instructions, given);
            for value in node.task.labels.values_mut() {
                *value = substitute(value, given);
            }
            for exit in &mut node.exits {
                if let Some(check) = &mut exit.check {
                    *check = substitute(check, given);
                }
            }
        }
        Ok(out)
    }

    /// The category a task node is planned as: its own, the workflow's, or
    /// the default one.
    pub fn node_category(&self, node: &WorkflowNode) -> String {
        control_plan::effective_category(node.task.category.as_deref().or(self.category.as_deref())).to_string()
    }

    /// Every category a task node here is planned as -- what a caller
    /// resolves a plan for before calling [`inject`](Self::inject).
    pub fn categories(&self) -> BTreeSet<String> {
        self.nodes
            .iter()
            .filter(|n| n.kind == WorkflowNodeKind::Task)
            .map(|n| self.node_category(n))
            .collect()
    }

    /// The task node a gate node judges: its named `subject`, or else the
    /// one task node its parents lead back to through other gates. `None`
    /// when there is no such node, or more than one.
    pub fn gate_subject(&self, gate_id: &str) -> Option<String> {
        let mut seen = BTreeSet::new();
        self.gate_subject_inner(gate_id, &mut seen)
    }

    fn gate_subject_inner(&self, gate_id: &str, seen: &mut BTreeSet<String>) -> Option<String> {
        if !seen.insert(gate_id.to_string()) {
            return None;
        }
        let gate = self.node(gate_id)?;
        if let Some(subject) = gate.gate.as_ref().and_then(|g| g.subject.clone()) {
            return (self.node(&subject)?.kind == WorkflowNodeKind::Task).then_some(subject);
        }
        let mut subjects = BTreeSet::new();
        for edge in self.edges.iter().filter(|e| e.to == gate_id) {
            let parent = self.node(&edge.from)?;
            match parent.kind {
                WorkflowNodeKind::Task => {
                    subjects.insert(parent.id.clone());
                }
                WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval => {
                    subjects.insert(self.gate_subject_inner(&parent.id, seen)?);
                }
            }
        }
        if subjects.len() == 1 {
            subjects.into_iter().next()
        } else {
            None
        }
    }

    /// The gates that judge `node_id`'s run, in the order they run: the
    /// definition's own topological order. What a run of that node's task
    /// is held to (`Run::required_steps`).
    pub fn required_steps_for(&self, node_id: &str) -> Vec<RequiredStep> {
        let order = self.validate().unwrap_or_else(|_| self.nodes.iter().map(|n| n.id.clone()).collect());
        order
            .iter()
            .filter_map(|id| self.node(id))
            .filter(|n| {
                matches!(
                    n.kind,
                    WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval
                )
            })
            .filter(|n| self.gate_subject(&n.id).as_deref() == Some(node_id))
            .filter_map(|n| {
                let gate = n.gate.as_ref()?;
                Some(RequiredStep {
                    step: gate.step.clone(),
                    kind: match n.kind {
                        WorkflowNodeKind::Gate => StepKind::Gate,
                        WorkflowNodeKind::Review => StepKind::Review,
                        WorkflowNodeKind::Approval => StepKind::Approval,
                        WorkflowNodeKind::Task => return None,
                    },
                    command: gate.command.clone(),
                    timeout_seconds: gate.timeout_seconds,
                    required_by: gate.required_by.clone(),
                    by: gate.by.clone(),
                    actor: gate.actor.clone(),
                    node_id: Some(n.id.clone()),
                })
            })
            .collect()
    }

    /// The implicit one-node workflow a standalone task is planned as --
    /// `#118`'s "no second code path": a categorised task gets exactly the
    /// injection a workflow's task node gets, through [`inject`](Self::inject).
    /// Never stored; the single node is [`IMPLICIT_NODE`].
    pub fn implicit(task: &crate::task::Task) -> Self {
        let now = Utc::now();
        Self {
            id: format!("task:{}", task.id),
            name: task.title.clone(),
            description: String::new(),
            scope: task.scope.clone(),
            category: task.category.clone(),
            inputs: Vec::new(),
            nodes: vec![WorkflowNode {
                id: IMPLICIT_NODE.into(),
                position: CanvasPoint::default(),
                kind: WorkflowNodeKind::Task,
                task: NewTask {
                    title: task.title.clone(),
                    instructions: task.instructions.clone(),
                    scope: Some(task.scope.clone()),
                    category: task.category.clone(),
                    ..Default::default()
                },
                gate: None,
                exits: Vec::new(),
            }],
            edges: Vec::new(),
            revision: 1,
            created_at: now,
            updated_at: now,
        }
    }

    /// Merge each task node's control plan into a copy of this definition
    /// as locked `Gate` nodes (`#118`), and say what was done. `plans` is
    /// keyed by category ([`categories`](Self::categories)); a category with
    /// no entry has nothing required.
    ///
    /// Per task node, every enforced step of its plan either
    ///
    /// * is already satisfied -- an authored gate node for the same step
    ///   judges this node -- and that node is annotated with the controls
    ///   it carries out, or
    /// * gets a locked gate node, chained after the task node in the plan's
    ///   own order, and every edge that left the task node now leaves the
    ///   last of its gates instead -- so nothing downstream starts until the
    ///   work it consumes has been verified.
    ///
    /// Gates go after *every* task node, not only the last ones: each node
    /// works in a worktree of its own, so a gate after the leaves would
    /// judge none of the earlier nodes' work.
    pub fn inject(&self, plans: &BTreeMap<String, ControlPlan>) -> (WorkflowDefinition, Vec<Injection>) {
        let mut out = self.clone();
        let mut notes = Vec::new();
        let mut ids: BTreeSet<String> = self.nodes.iter().map(|n| n.id.clone()).collect();
        let mut edge_ids: BTreeSet<String> = self.edges.iter().map(|e| e.id.clone()).collect();
        let fresh = |base: String, taken: &mut BTreeSet<String>| {
            let mut id = base.clone();
            let mut n = 2;
            while taken.contains(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            taken.insert(id.clone());
            id
        };

        for work in self.nodes.iter().filter(|n| n.kind == WorkflowNodeKind::Task) {
            let category = self.node_category(work);
            let Some(plan) = plans.get(&category) else { continue };
            let authored: Vec<&WorkflowNode> = self
                .nodes
                .iter()
                .filter(|n| n.kind == WorkflowNodeKind::Gate)
                .filter(|n| self.gate_subject(&n.id).as_deref() == Some(work.id.as_str()))
                .collect();

            let mut to_inject: Vec<&PlanStep> = Vec::new();
            for step in plan.enforced() {
                let existing = (step.kind == StepKind::Gate)
                    .then(|| authored.iter().find(|g| g.gate.as_ref().is_some_and(|g| g.step == step.step)))
                    .flatten();
                if let Some(existing) = existing {
                    if let Some(node) = out.nodes.iter_mut().find(|n| n.id == existing.id) {
                        let gate = node.gate.get_or_insert_with(Default::default);
                        for by in &step.required_by {
                            if !gate.required_by.contains(by) {
                                gate.required_by.push(by.clone());
                            }
                        }
                    }
                    notes.push(Injection {
                        node_id: work.id.clone(),
                        category: category.clone(),
                        step: step.id.clone(),
                        required_by: step.required_by.clone(),
                        gate_node_id: existing.id.clone(),
                        satisfied_by_authored: true,
                    });
                } else {
                    to_inject.push(step);
                }
            }
            if to_inject.is_empty() {
                continue;
            }

            let approvals: Vec<_> = to_inject
                .iter()
                .copied()
                .filter(|s| s.kind == StepKind::Approval)
                .collect();
            let gates: Vec<_> = to_inject
                .iter()
                .copied()
                .filter(|s| s.kind == StepKind::Gate)
                .collect();
            let reviews: Vec<_> = to_inject
                .iter()
                .copied()
                .filter(|s| s.kind == StepKind::Review)
                .collect();
            // A resolved plan's unconstrained tie-break is lexical, which
            // would put `review` before `tests`. Runtime verification and the
            // documented workflow both spend deterministic gates first.
            // Keep the resolved order within each phase, but make the phase
            // boundary explicit in the immutable workflow snapshot.
            let after: Vec<_> = gates.into_iter().chain(reviews).collect();

            // Approval is a prerequisite: every former parent reaches the
            // approval chain, whose last node reaches the work. A root task
            // simply starts at its approval.
            let mut approval_first: Option<String> = None;
            let mut approval_previous: Option<String> = None;
            for (i, step) in approvals.iter().enumerate() {
                let node_id = fresh(format!("{}.{}", work.id, step.id), &mut ids);
                approval_first.get_or_insert_with(|| node_id.clone());
                out.nodes.push(control_node(self, work, step, &node_id, i, true));
                if let Some(previous) = approval_previous.replace(node_id.clone()) {
                    out.edges.push(WorkflowEdge {
                        id: fresh(format!("{previous}->{node_id}"), &mut edge_ids),
                        from: previous,
                        to: node_id.clone(),
                    });
                }
                notes.push(injection(work, &category, step, &node_id));
            }
            if let (Some(first), Some(last)) = (approval_first, approval_previous) {
                for edge in out.edges.iter_mut().filter(|e| e.to == work.id) {
                    edge.to = first.clone();
                }
                out.edges.push(WorkflowEdge {
                    id: fresh(format!("{last}->{}", work.id), &mut edge_ids),
                    from: last,
                    to: work.id.clone(),
                });
            }

            let mut previous = work.id.clone();
            let mut chain_edges = Vec::new();
            for (i, step) in after.iter().enumerate() {
                let gate_id = fresh(format!("{}.{}", work.id, step.id), &mut ids);
                out.nodes.push(control_node(self, work, step, &gate_id, i, false));
                chain_edges.push(WorkflowEdge {
                    id: fresh(format!("{previous}->{gate_id}"), &mut edge_ids),
                    from: previous.clone(),
                    to: gate_id.clone(),
                });
                notes.push(Injection {
                    node_id: work.id.clone(),
                    category: category.clone(),
                    step: step.id.clone(),
                    required_by: step.required_by.clone(),
                    gate_node_id: gate_id.clone(),
                    satisfied_by_authored: false,
                });
                previous = gate_id;
            }
            // Whatever followed the work now follows its last gate.
            for edge in out.edges.iter_mut().filter(|e| e.from == work.id) {
                edge.from = previous.clone();
            }
            out.edges.extend(chain_edges);
        }
        (out, notes)
    }
}

fn injection(work: &WorkflowNode, category: &str, step: &PlanStep, node_id: &str) -> Injection {
    Injection {
        node_id: work.id.clone(),
        category: category.to_string(),
        step: step.id.clone(),
        required_by: step.required_by.clone(),
        gate_node_id: node_id.to_string(),
        satisfied_by_authored: false,
    }
}

fn control_node(
    definition: &WorkflowDefinition,
    work: &WorkflowNode,
    step: &PlanStep,
    id: &str,
    index: usize,
    before: bool,
) -> WorkflowNode {
    let kind = match step.kind {
        StepKind::Gate => WorkflowNodeKind::Gate,
        StepKind::Review => WorkflowNodeKind::Review,
        StepKind::Approval => WorkflowNodeKind::Approval,
    };
    let noun = step.kind.as_str();
    WorkflowNode {
        id: id.to_string(),
        position: CanvasPoint {
            x: work.position.x + if before { -40.0 } else { 40.0 },
            y: work.position.y + if before { -90.0 } else { 90.0 } * (index as f64 + 1.0),
        },
        kind,
        task: NewTask {
            title: format!("{noun}: {}", step.step),
            instructions: step.command.clone().unwrap_or_default(),
            scope: Some(definition.scope.clone()),
            ..Default::default()
        },
        gate: Some(GateSpec {
            step: step.id.clone(),
            command: step.command.clone(),
            timeout_seconds: step.timeout_seconds,
            subject: Some(work.id.clone()),
            required_by: step.required_by.clone(),
            locked: true,
            by: step.by.clone(),
            actor: None,
        }),
        exits: if matches!(kind, WorkflowNodeKind::Review | WorkflowNodeKind::Gate) {
            vec![WorkflowExit {
                to: work.id.clone(),
                check: None,
                agent: Some(
                    "the independent review found concrete changes the subject agent must make"
                        .into(),
                ),
                max_rounds: Some(5),
            }]
        } else {
            Vec::new()
        },
    }
}

impl WorkflowDefinition {
    /// `before:` rules the graph breaks, in words (DECLARE's `precedence`):
    /// a plan step that must run before a node named `X` -- by node id, or
    /// by a gate's step -- while `X` can start with no gate for that step
    /// upstream of it. Read off an *injected* definition, so it reports what
    /// injection could not fix, e.g. a `publish` node with no work before
    /// it for the scan to follow.
    pub fn ordering_violations(&self, plans: &BTreeMap<String, ControlPlan>) -> Vec<String> {
        let mut out = BTreeSet::new();
        for plan in plans.values() {
            for step in plan.enforced() {
                for target in &step.before {
                    for node in self.nodes.iter().filter(|n| {
                        n.id == *target || n.gate.as_ref().is_some_and(|g| g.step == *target)
                    }) {
                        let preceded = self.ancestors(&node.id).iter().any(|a| {
                            self.node(a)
                                .and_then(|n| n.gate.as_ref())
                                .is_some_and(|g| g.step == step.id || g.step == step.step)
                        });
                        if !preceded {
                            out.insert(format!(
                                "{} ({}) can start without a {} gate before it; {} requires {} before {target}",
                                node.id,
                                node.task.title,
                                step.step,
                                step.required_by.join(", "),
                                step.step
                            ));
                        }
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    fn descendants(&self, id: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![id.to_string()];
        while let Some(current) = stack.pop() {
            for edge in self.edges.iter().filter(|e| e.from == current) {
                if seen.insert(edge.to.clone()) {
                    stack.push(edge.to.clone());
                }
            }
        }
        seen
    }

    pub fn ancestors(&self, id: &str) -> BTreeSet<String> {
        let mut seen = BTreeSet::new();
        let mut stack = vec![id.to_string()];
        while let Some(current) = stack.pop() {
            for edge in self.edges.iter().filter(|e| e.to == current) {
                if seen.insert(edge.from.clone()) {
                    stack.push(edge.from.clone());
                }
            }
        }
        seen
    }
}

/// An input's name: `[A-Za-z_][A-Za-z0-9_-]*`.
fn is_input_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Every `{{name}}` in `text` (spaces inside the braces allowed), in order.
/// Braces around anything that is not a name -- `{{.Names}}`, `{{}}` -- are
/// not placeholders and are left to whatever they were meant for.
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        let name = after[..close].trim();
        if is_input_name(name) {
            out.push(name);
        }
        rest = &after[close + 2..];
    }
    out
}

/// `text` with each `{{name}}` whose name is in `values` replaced.
fn substitute(text: &str, values: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else { break };
        match values.get(after[..close].trim()) {
            Some(value) => {
                out.push_str(&rest[..open]);
                out.push_str(value);
            }
            None => out.push_str(&rest[..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// `factory workflow lint`'s answer (`#118`): the effective plan for each
/// category a definition's task nodes are planned as, what injection adds,
/// which authored gates already satisfy a step, and the ordering rules the
/// graph breaks. An author-facing preview, never a gate of its own -- the
/// runtime check is the daemon's verifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowLint {
    /// The workflow's id, `task:<id>` for a task's implicit workflow, or
    /// empty for a bare scope-and-category preview.
    pub subject: String,
    pub scope: String,
    pub plans: Vec<ControlPlan>,
    #[serde(default)]
    pub injections: Vec<Injection>,
    #[serde(default)]
    pub violations: Vec<String>,
    /// The definition as a run of it would start: authored nodes plus the
    /// locked gates. Absent for a bare scope-and-category preview.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub injected: Option<WorkflowDefinition>,
}

/// The one node of [`WorkflowDefinition::implicit`].
pub const IMPLICIT_NODE: &str = "work";

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
    /// A task node whose run reported `done` and is being verified, or a
    /// gate node running its check (`#118`).
    Verifying,
    Done,
    Failed,
    Cancelled,
    /// Bypassed because an earlier node took a conditional exit around it.
    /// Unlike `Skipped`, this is a successful, finished route decision.
    SkippedByRoute,
    /// Never started because the workflow ended first.
    Skipped,
}

impl WorkflowNodeStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::SkippedByRoute | Self::Skipped
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
    /// How many times work was sent back through this node: 0 on its first
    /// pass; otherwise the backwards-exit rounds it has used.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: u32,
    /// The tasks earlier rounds spawned here, oldest first. Kept so their
    /// history stays findable; never mirrored, never recreated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superseded_task_ids: Vec<String>,
    /// On the node work was sent back to: who sent it and why, which its
    /// next task is dispatched with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rework_request: Option<ReworkRequest>,
    /// Set once this node's ordered exits have been evaluated. Necessary so
    /// a later reconciliation does not run a `check:` command twice.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exits_evaluated: bool,
    /// The exclusive target selected by an exit, if one held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routed_to: Option<String>,
    /// Human-readable route reason for `SkippedByRoute` nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_reason: Option<String>,
}

/// Why a node is running again -- see [`WorkflowNodeRun::rework_request`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReworkRequest {
    /// The node that sent the work back, and the task that did.
    pub from_node: String,
    pub from_task: String,
    /// This round, counted from 1, and how many there may be.
    pub round: u32,
    pub max_rounds: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
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
    /// What it was started with (`#140`), already written into
    /// `definition`; kept to say so.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
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
                    round: 0,
                    superseded_task_ids: Vec::new(),
                    rework_request: None,
                    exits_evaluated: false,
                    routed_to: None,
                    skip_reason: None,
                })
                .collect(),
            definition,
            status: WorkflowRunStatus::Running,
            failure_node_id: None,
            error: None,
            started_by,
            inputs: BTreeMap::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// What [`WorkflowRun::send_back`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendBack {
    /// The requested target is not a bounded backwards exit of this node.
    NoExit,
    /// The path runs again; this is round `round` of `max_rounds`.
    Sent { round: u32, max_rounds: u32 },
    /// Every round is used up: the route request must be refused.
    Exhausted { max_rounds: u32 },
}

impl WorkflowRun {
    /// Send `from`'s work back through its declared backwards exit, if it has one
    /// and a round is left: every node on the path from the target down to
    /// `from` goes back to `unstarted` with its task moved to
    /// `superseded_task_ids` and its round counted up, the target is told
    /// who sent it back, and the not-yet-started nodes below `from` (a gate
    /// mirrored `skipped` off the failed run, say) are `unstarted` again.
    /// Nothing is spawned here; the next advance does that.
    pub fn send_back(&mut self, from: &str, to: &str) -> SendBack {
        let Some(spec) = self
            .definition
            .nodes
            .iter()
            .find(|n| n.id == from)
            .and_then(|n| {
                n.exits
                    .iter()
                    .find(|exit| exit.to == to && exit.max_rounds.is_some())
            })
            .cloned()
        else {
            return SendBack::NoExit;
        };
        let Some(used) = self.nodes.iter().find(|n| n.node_id == from).map(|n| n.round) else {
            return SendBack::NoExit;
        };
        let max_rounds = spec.max_rounds.unwrap_or(0);
        if used >= max_rounds {
            return SendBack::Exhausted { max_rounds };
        }
        let from_task = self.nodes.iter().find(|n| n.node_id == from).and_then(|n| n.task_id.clone()).unwrap_or_default();
        let downstream = self.definition.descendants(from);
        let round = used + 1;
        for id in self.definition.route_back_body(from, to) {
            let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == id) else {
                continue;
            };
            if downstream.contains(&id) {
                // Waits on `from`, so it never started -- only a mirror
                // may have marked it.
                if node.task_id.is_none() {
                    node.status = WorkflowNodeStatus::Unstarted;
                    node.error = None;
                }
                continue;
            }
            if let Some(task) = node.task_id.take() {
                node.superseded_task_ids.push(task);
            }
            node.status = WorkflowNodeStatus::Unstarted;
            node.error = None;
            node.round = round;
            node.exits_evaluated = false;
            node.routed_to = None;
            node.skip_reason = None;
            node.rework_request = (id == spec.to).then(|| ReworkRequest {
                from_node: from.to_string(),
                from_task: from_task.clone(),
                round,
                max_rounds,
            });
        }
        self.updated_at = Utc::now();
        SendBack::Sent { round, max_rounds }
    }

    /// Take a forward exit exclusively. Nodes downstream of `from` that are
    /// not the target or downstream of it are bypassed by this route.
    pub fn route_forward(&mut self, from: &str, to: &str) {
        let below_from = self.definition.descendants(from);
        let mut kept = self.definition.descendants(to);
        kept.insert(to.to_string());
        for node in &mut self.nodes {
            if below_from.contains(&node.node_id)
                && !kept.contains(&node.node_id)
                && node.status == WorkflowNodeStatus::Unstarted
            {
                node.status = WorkflowNodeStatus::SkippedByRoute;
                node.skip_reason = Some(format!("skipped ({from} -> {to})"));
            }
        }
        if let Some(node) = self.nodes.iter_mut().find(|node| node.node_id == from) {
            node.routed_to = Some(to.to_string());
        }
        self.updated_at = Utc::now();
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
            gate: None,
            exits: Vec::new(),
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

    fn plan(category: &str, steps: &[(&str, Option<&str>, Option<&str>)]) -> BTreeMap<String, ControlPlan> {
        let steps = steps
            .iter()
            .map(|(step, command, before)| PlanStep {
                id: step.to_string(),
                step: step.to_string(),
                kind: StepKind::of(step),
                command: command.map(str::to_string),
                timeout_seconds: None,
                before: before.map(|b| vec![b.to_string()]).unwrap_or_default(),
                after: vec![],
                by: None,
                required_by: vec!["cra/security-testing".into()],
                enforced: StepKind::of(step).enforced(),
            })
            .collect();
        BTreeMap::from([(
            category.to_string(),
            ControlPlan { scope: "demo".into(), category: category.into(), steps, ..Default::default() },
        )])
    }

    fn gate_node(id: &str, step: &str) -> WorkflowNode {
        WorkflowNode {
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Gate,
            task: NewTask {
                title: format!("gate {step}"),
                ..Default::default()
            },
            gate: Some(GateSpec {
                step: step.into(),
                command: Some("true".into()),
                ..Default::default()
            }),
            exits: Vec::new(),
        }
    }

    fn e(from: &str, to: &str) -> WorkflowEdge {
        WorkflowEdge { id: format!("{from}-{to}"), from: from.into(), to: to.into() }
    }

    #[test]
    fn injection_puts_a_locked_gate_chain_after_every_task_node_and_moves_its_children_behind_it() {
        let def = definition(vec![node("a"), node("b")], vec![e("a", "b")]);
        let (out, notes) = def.inject(&plan("default", &[("lint", Some("make lint"), None), ("tests", Some("make test"), None)]));
        out.validate().unwrap();
        assert_eq!(notes.len(), 4, "two steps for each of two nodes: {notes:#?}");
        let a_gates = out.required_steps_for("a");
        assert_eq!(a_gates.iter().map(|g| g.step.as_str()).collect::<Vec<_>>(), vec!["lint", "tests"]);
        assert!(out.nodes.iter().filter(|n| n.kind == WorkflowNodeKind::Gate).all(|n| n.gate.as_ref().unwrap().locked));
        // b now waits on a's last gate, not on a itself.
        assert!(out.edges.iter().any(|e| e.from == "a.tests" && e.to == "b"), "{:#?}", out.edges);
        assert!(!out.edges.iter().any(|e| e.from == "a" && e.to == "b"));
        assert_eq!(out.required_steps_for("b").len(), 2);
    }

    #[test]
    fn an_empty_plan_injects_nothing_and_leaves_the_definition_as_it_was() {
        let def = definition(vec![node("a")], vec![]);
        let (out, notes) = def.inject(&BTreeMap::new());
        assert!(notes.is_empty());
        assert_eq!(out.nodes.len(), 1);
    }

    #[test]
    fn an_authored_gate_for_the_same_step_satisfies_the_requirement_and_is_annotated() {
        let def = definition(vec![node("a"), gate_node("my-tests", "tests")], vec![e("a", "my-tests")]);
        def.validate().unwrap();
        let (out, notes) = def.inject(&plan("default", &[("tests", Some("make test"), None)]));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].satisfied_by_authored);
        assert_eq!(notes[0].gate_node_id, "my-tests");
        assert_eq!(out.nodes.len(), 2, "nothing injected");
        let gate = out.nodes.iter().find(|n| n.id == "my-tests").unwrap().gate.clone().unwrap();
        assert_eq!(gate.required_by, vec!["cra/security-testing"]);
        assert_eq!(out.required_steps_for("a")[0].command.as_deref(), Some("true"), "the authored command runs");
    }

    #[test]
    fn a_node_category_overrides_the_workflows_and_only_its_plan_applies() {
        let mut def = definition(vec![node("a"), node("b")], vec![]);
        def.category = Some("feature".into());
        def.nodes[1].task.category = Some("docs".into());
        let (out, _) = def.inject(&plan("feature", &[("tests", Some("true"), None)]));
        assert_eq!(out.required_steps_for("a").len(), 1);
        assert!(out.required_steps_for("b").is_empty());
    }

    #[test]
    fn review_steps_are_injected_after_gates_with_a_bounded_rework_exit() {
        let def = definition(vec![node("a")], vec![]);
        let (out, notes) = def.inject(&plan(
            "default",
            &[("tests", Some("true"), None), ("review", None, None)],
        ));
        assert_eq!(notes.len(), 2);
        let review = out
            .nodes
            .iter()
            .find(|n| n.kind == WorkflowNodeKind::Review)
            .unwrap();
        assert_eq!(review.exits[0].to, "a");
        assert_eq!(review.exits[0].max_rounds, Some(5));
        let steps = out.required_steps_for("a");
        assert_eq!(
            steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
            vec![StepKind::Gate, StepKind::Review]
        );
    }

    #[test]
    fn resolved_lexical_plan_still_injects_deterministic_gates_before_review() {
        let requirement = |step: &str, gate: Option<&str>, by: Option<&str>| {
            crate::control_plan::Requirement {
                applies_to: vec!["feature".into()],
                step: step.into(),
                gate: gate.map(str::to_string),
                by: by.map(str::to_string),
                before: None,
                after: None,
                timeout_seconds: None,
            }
        };
        let applied = crate::policy::Applied {
            control: crate::policy::ControlRef::new("house", "tested"),
            title: "Tested".into(),
            kind: crate::policy::Kind::BestPractice,
            maps_to: Vec::new(),
            evidence: Vec::new(),
            max_age: None,
            not_applicable: None,
            remediation: None,
            requires: vec![
                requirement("tests", Some("true"), None),
                requirement("review", None, Some("independent")),
            ],
        };
        let resolved = control_plan::resolve("demo", "feature", &[applied], &[]);
        assert_eq!(
            resolved.steps.iter().map(|s| s.step.as_str()).collect::<Vec<_>>(),
            vec!["review", "tests"],
            "the unconstrained plan demonstrates its lexical tie-break"
        );

        let mut def = definition(vec![node("a")], vec![]);
        def.category = Some("feature".into());
        let (out, _) = def.inject(&BTreeMap::from([("feature".into(), resolved)]));
        assert_eq!(
            out.required_steps_for("a")
                .iter()
                .map(|s| s.kind)
                .collect::<Vec<_>>(),
            vec![StepKind::Gate, StepKind::Review]
        );
        assert!(out.edges.iter().any(|edge| edge.from == "a.tests" && edge.to == "a.review"));
    }

    #[test]
    fn approval_is_injected_before_the_subject_and_review_after_its_gate() {
        let def = definition(vec![node("a"), node("b")], vec![e("a", "b")]);
        let (out, _) = def.inject(&plan(
            "default",
            &[
                ("approval", None, None),
                ("tests", Some("true"), None),
                ("review", None, None),
            ],
        ));
        let approval = out
            .nodes
            .iter()
            .find(|n| n.kind == WorkflowNodeKind::Approval)
            .unwrap();
        let review = out
            .nodes
            .iter()
            .find(|n| n.kind == WorkflowNodeKind::Review)
            .unwrap();
        assert!(out
            .edges
            .iter()
            .any(|e| e.from == approval.id && e.to == "a"));
        assert!(out.ancestors(&review.id).contains("a"));
        assert!(out.descendants(&review.id).contains("b"));
    }

    #[test]
    fn a_gate_must_judge_exactly_one_task_node() {
        let def = definition(
            vec![node("a"), node("b"), gate_node("g", "tests")],
            vec![e("a", "g"), e("b", "g")],
        );
        let error = def.validate().unwrap_err();
        assert!(error.contains("exactly one task node"), "{error}");
        let orphan = definition(vec![node("a"), gate_node("g", "tests")], vec![]);
        assert!(orphan.validate().is_err());
    }

    #[test]
    fn an_authored_gate_without_a_command_is_refused_but_a_locked_one_may_lack_it() {
        let mut g = gate_node("g", "tests");
        g.gate.as_mut().unwrap().command = None;
        let def = definition(vec![node("a"), g.clone()], vec![e("a", "g")]);
        assert!(def.validate().unwrap_err().contains("command"));
        g.gate.as_mut().unwrap().locked = true;
        let def = definition(vec![node("a"), g], vec![e("a", "g")]);
        def.validate().unwrap();
    }

    #[test]
    fn a_bad_category_is_refused() {
        let mut def = definition(vec![node("a")], vec![]);
        def.category = Some("Not A Name".into());
        assert!(def.validate().is_err());
    }

    #[test]
    fn the_implicit_workflow_of_a_task_is_one_node_carrying_its_category() {
        let task = crate::adapter::store::task_from_new(
            NewTask { title: "ship it".into(), category: Some("release".into()), ..Default::default() },
            "demo".into(),
            "shell".into(),
            "herdr".into(),
        );
        let def = WorkflowDefinition::implicit(&task);
        def.validate().unwrap();
        let (out, _) = def.inject(&plan("release", &[("sbom", Some("make sbom"), None)]));
        let steps = out.required_steps_for(IMPLICIT_NODE);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].node_id.as_deref(), Some("work.sbom"));
    }

    #[test]
    fn a_before_rule_the_graph_cannot_honour_is_an_ordering_violation() {
        // `publish` is a root: nothing runs before it, so no scan can either.
        let def = definition(vec![node("publish"), node("build")], vec![e("publish", "build")]);
        let plans = plan("default", &[("security_scan", Some("true"), Some("publish"))]);
        let (out, _) = def.inject(&plans);
        let violations = out.ordering_violations(&plans);
        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("publish"), "{violations:?}");

        // Built first, then published: the build's scan precedes publish.
        let def = definition(vec![node("build"), node("publish")], vec![e("build", "publish")]);
        let (out, _) = def.inject(&plans);
        assert!(out.ordering_violations(&plans).is_empty());
    }

    #[test]
    fn a_schedule_on_a_node_is_refused() {
        let mut a = node("a");
        a.task.schedule = Some(crate::task::Schedule::Cron("* * * * *".into()));
        let error = definition(vec![a], vec![]).validate().unwrap_err();
        assert!(error.contains("schedule"), "{error}");
    }

    fn reviewing(max_rounds: u32) -> WorkflowDefinition {
        let mut review = node("review");
        review.exits = vec![WorkflowExit {
            to: "implement".into(),
            check: None,
            agent: Some("findings the implementer can fix".into()),
            max_rounds: Some(max_rounds),
        }];
        definition(
            vec![node("triage"), node("implement"), gate_node("implement.tests", "tests"), review, node("finalize")],
            vec![
                e("triage", "implement"),
                e("implement", "implement.tests"),
                e("implement.tests", "review"),
                e("review", "finalize"),
            ],
        )
    }

    #[test]
    fn exits_validate_topology_conditions_and_rounds() {
        assert!(reviewing(5).validate().is_ok());

        let mut forward = reviewing(5);
        forward.nodes[1].exits = vec![WorkflowExit {
            to: "review".into(),
            check: None,
            agent: Some("skip".into()),
            max_rounds: None,
        }];
        assert!(forward
            .validate()
            .unwrap_err()
            .contains("without an explicit edge"));

        let mut to_gate = reviewing(5);
        to_gate.nodes[3].exits = vec![WorkflowExit {
            to: "implement.tests".into(),
            check: None,
            agent: Some("retry".into()),
            max_rounds: Some(1),
        }];
        assert!(to_gate.validate().unwrap_err().contains("gate node"));

        assert!(reviewing(0)
            .validate()
            .unwrap_err()
            .contains("at least one"));

        let mut both = reviewing(1);
        both.nodes[3].exits[0].check = Some("true".into());
        assert!(both.validate().unwrap_err().contains("exactly one"));

        let mut neither = reviewing(1);
        neither.nodes[3].exits[0].agent = None;
        assert!(neither.validate().unwrap_err().contains("exactly one"));
    }

    #[test]
    fn legacy_rework_loads_as_one_agent_exit_in_definitions_and_in_flight_runs() {
        let node: WorkflowNode = serde_yaml_ng::from_str(
            "id: review\ntask: { title: Review }\nrework: { to: implement, max_rounds: 5 }\n",
        )
        .unwrap();
        assert_eq!(node.exits.len(), 1);
        assert_eq!(node.exits[0].to, "implement");
        assert!(node.exits[0].agent.is_some());
        assert_eq!(node.exits[0].max_rounds, Some(5));

        let mut run = WorkflowRun::new(reviewing(2), WorkflowActor::Owner);
        let review_index = run
            .definition
            .nodes
            .iter()
            .position(|node| node.id == "review")
            .unwrap();
        run.nodes
            .iter_mut()
            .find(|node| node.node_id == "review")
            .unwrap()
            .round = 1;
        let mut stored = serde_json::to_value(&run).unwrap();
        let review = &mut stored["definition"]["nodes"][review_index];
        review.as_object_mut().unwrap().remove("exits");
        review["rework"] = serde_json::json!({ "to": "implement", "max_rounds": 2 });
        let mut loaded: WorkflowRun = serde_json::from_value(stored).unwrap();
        assert_eq!(
            loaded.definition.nodes[review_index].exits[0].to,
            "implement"
        );
        assert_eq!(
            loaded.send_back("review", "implement"),
            SendBack::Sent {
                round: 2,
                max_rounds: 2
            },
            "a run already one round into legacy rework keeps its remaining round"
        );
    }

    #[test]
    fn a_forward_route_skips_the_default_path_and_skipped_by_route_is_terminal() {
        let mut run = WorkflowRun::new(
            definition(
                vec![node("a"), node("b"), node("c")],
                vec![e("a", "b"), e("b", "c"), e("a", "c")],
            ),
            WorkflowActor::Owner,
        );
        ran(&mut run, "a", WorkflowNodeStatus::Done, "a1");
        run.route_forward("a", "c");
        assert_eq!(
            run.nodes.iter().find(|n| n.node_id == "b").unwrap().status,
            WorkflowNodeStatus::SkippedByRoute
        );
        assert!(WorkflowNodeStatus::SkippedByRoute.is_terminal());
        assert_eq!(
            run.nodes.iter().find(|n| n.node_id == "c").unwrap().status,
            WorkflowNodeStatus::Unstarted
        );
    }

    #[test]
    fn the_rework_body_is_the_path_back_and_everything_below_it() {
        assert_eq!(
            reviewing(5).route_back_body("review", "implement"),
            vec!["implement", "implement.tests", "review", "finalize"]
        );
        assert!(reviewing(5)
            .route_back_body("implement", "review")
            .is_empty());
    }

    fn ran(run: &mut WorkflowRun, id: &str, status: WorkflowNodeStatus, task: &str) {
        let node = run.nodes.iter_mut().find(|n| n.node_id == id).unwrap();
        node.status = status;
        node.task_id = (!task.is_empty()).then(|| task.to_string());
    }

    #[test]
    fn sending_back_resets_the_path_keeps_history_and_stops_at_the_budget() {
        let mut run = WorkflowRun::new(reviewing(2), WorkflowActor::Owner);
        ran(&mut run, "triage", WorkflowNodeStatus::Done, "t1");
        ran(&mut run, "implement", WorkflowNodeStatus::Done, "i1");
        ran(&mut run, "implement.tests", WorkflowNodeStatus::Done, "");
        ran(&mut run, "review", WorkflowNodeStatus::Failed, "r1");
        ran(&mut run, "finalize", WorkflowNodeStatus::Skipped, "");

        assert_eq!(
            run.send_back("review", "implement"),
            SendBack::Sent {
                round: 1,
                max_rounds: 2
            }
        );
        let node = |run: &WorkflowRun, id: &str| {
            run.nodes.iter().find(|n| n.node_id == id).unwrap().clone()
        };
        assert_eq!(
            node(&run, "triage").status,
            WorkflowNodeStatus::Done,
            "above the target is untouched"
        );
        let implement = node(&run, "implement");
        assert_eq!(implement.status, WorkflowNodeStatus::Unstarted);
        assert_eq!(implement.task_id, None);
        assert_eq!(implement.superseded_task_ids, vec!["i1"]);
        assert_eq!(
            implement.rework_request,
            Some(ReworkRequest { from_node: "review".into(), from_task: "r1".into(), round: 1, max_rounds: 2 })
        );
        assert_eq!(node(&run, "review").round, 1);
        assert_eq!(node(&run, "review").rework_request, None);
        assert_eq!(node(&run, "finalize").status, WorkflowNodeStatus::Unstarted);

        ran(&mut run, "review", WorkflowNodeStatus::Failed, "r2");
        assert_eq!(
            run.send_back("review", "implement"),
            SendBack::Sent {
                round: 2,
                max_rounds: 2
            }
        );
        assert_eq!(
            node(&run, "implement").superseded_task_ids,
            vec!["i1"],
            "i2 never existed here"
        );
        ran(&mut run, "review", WorkflowNodeStatus::Failed, "r3");
        assert_eq!(
            run.send_back("review", "implement"),
            SendBack::Exhausted { max_rounds: 2 }
        );
        assert_eq!(
            node(&run, "review").task_id.as_deref(),
            Some("r3"),
            "an exhausted budget changes nothing"
        );
        assert_eq!(run.send_back("triage", "implement"), SendBack::NoExit);
    }

    #[test]
    fn inputs_are_required_substituted_and_typos_refused() {
        let mut def = definition(vec![node("work")], Vec::new());
        def.inputs = vec![WorkflowInput { name: "issue".into(), description: "the issue".into() }];
        def.nodes[0].task.title = "Triage #{{ issue }}".into();
        def.nodes[0].task.instructions = "gh issue view {{issue}} --json title --jq '{{.title}}'".into();
        def.nodes[0].task.labels.insert("issue".into(), "{{issue}}".into());
        def.validate().unwrap();

        let given = BTreeMap::from([("issue".to_string(), "42".to_string())]);
        let out = def.with_inputs(&given).unwrap();
        assert_eq!(out.nodes[0].task.title, "Triage #42");
        assert_eq!(out.nodes[0].task.instructions, "gh issue view 42 --json title --jq '{{.title}}'");
        assert_eq!(out.nodes[0].task.labels["issue"], "42");

        assert!(def.with_inputs(&BTreeMap::new()).unwrap_err().contains("needs issue"));
        let extra = BTreeMap::from([("issue".to_string(), "1".to_string()), ("pr".to_string(), "2".to_string())]);
        assert!(def.with_inputs(&extra).unwrap_err().contains("no input \"pr\""));

        def.nodes[0].task.instructions = "{{isue}}".into();
        assert!(def.validate().unwrap_err().contains("no input \"isue\""));

        // With no inputs declared, braces are nobody's business.
        let mut plain = definition(vec![node("work")], Vec::new());
        plain.nodes[0].task.instructions = "docker ps --format '{{Names}}'".into();
        plain.validate().unwrap();
        assert_eq!(plain.with_inputs(&BTreeMap::new()).unwrap().nodes[0].task.instructions, "docker ps --format '{{Names}}'");
    }
}
