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
    /// Feedback resumes by default. Independent work may explicitly opt out.
    #[serde(default, skip_serializing_if = "SessionPolicy::is_resume")]
    pub session: SessionPolicy,
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
    /// Fan-out/join policy for an `Expand` node.  Absent everywhere else.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<ExpandSpec>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionPolicy {
    #[default]
    Resume,
    Fresh,
}

impl SessionPolicy {
    fn is_resume(&self) -> bool {
        *self == Self::Resume
    }
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
    pub session: SessionPolicy,
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
    pub expand: Option<ExpandSpec>,
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
            session: wire.session,
            position: wire.position,
            kind: wire.kind,
            task: wire.task,
            gate: wire.gate,
            exits: wire.exits,
            expand: wire.expand,
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
    /// A daemon-owned fan-out boundary.  It never spawns an agent itself;
    /// its children are task nodes materialised from an approved intake
    /// decomposition in this run's snapshot.
    Expand,
}

/// When an expand node's children are joined, tolerate this many failed
/// independent branches.  Zero is the safe/default `all_succeeded` policy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandJoin {
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub tolerate: u8,
}

fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

/// What cancelling a parent workflow does to children already in flight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpandCancelPolicy {
    #[default]
    Terminate,
    Abandon,
}

/// Runtime policy carried by an `expand` node.  `children` is explicit in
/// the immutable run snapshot, making restart recovery deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExpandSpec {
    #[serde(default)]
    pub join: ExpandJoin,
    #[serde(default)]
    pub cancel: ExpandCancelPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
    #[serde(default = "default_rework_rounds")]
    pub max_rework_rounds: u32,
}

fn default_rework_rounds() -> u32 {
    3
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

/// Declares a definition a **part workflow** (`#235`): the steps every part
/// of an Intake decomposition runs through before the integrator merges it.
/// `start_decomposition_workflow` copies the definition once per part into
/// the one generated run. Either field may be left out when the graph makes
/// it unambiguous -- see [`WorkflowDefinition::part_shape`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartSpec {
    /// The task node whose worktree branch is integrated, and which merge
    /// conflicts and failed combined checks are sent back to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliverable: Option<String>,
    /// The node whose `done` releases the merge: the one node nothing
    /// follows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

/// The reserved inputs a part workflow is filled with, once per part
/// (`#235`). Nothing else may be written `{{...}}` in one.
pub const PART_INPUTS: [&str; 8] = [
    "part_id",
    "part_title",
    "part_instructions",
    "part_acceptance",
    "part_owns",
    "part_interface",
    "parent_title",
    "parent_instructions",
];

/// A part workflow's three roles, resolved: the node `expand` and every
/// prerequisite part lead into, the node whose branch is merged, and the
/// node whose `done` releases that merge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartShape {
    pub entry: String,
    pub deliverable: String,
    pub terminal: String,
}

/// One part's copy of a part workflow, namespaced `<part>-<node>`, with the
/// part's values written in. See [`WorkflowDefinition::expand_part`].
#[derive(Debug, Clone)]
pub struct PartCopy {
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub shape: PartShape,
}

/// The id a part workflow's node `node` gets in part `part`'s copy.
pub fn part_node_id(part: &str, node: &str) -> String {
    format!("{part}-{node}")
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkflowDraft {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub scope: String,
    /// Optional git revision for task workspaces. Resolved to one immutable
    /// commit before a run starts; every git-backed task scope must contain it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_ref: Option<String>,
    /// The category every task node is planned as unless it names its own
    /// (`#118`). Absent is the default category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// What a run of it must be started with (`#140`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<WorkflowInput>,
    /// Present on a part workflow (`#235`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<PartSpec>,
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
    /// Authored revision on the definition; frozen commit on a run snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_ref: Option<String>,
    /// See `WorkflowDraft::category`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// See `WorkflowDraft::inputs`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<WorkflowInput>,
    /// See `WorkflowDraft::part`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<PartSpec>,
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
            workspace_ref: draft.workspace_ref,
            category: draft.category,
            inputs: draft.inputs,
            part: draft.part,
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
        self.workspace_ref = draft.workspace_ref;
        self.inputs = draft.inputs;
        self.part = draft.part;
        self.nodes = draft.nodes;
        self.edges = draft.edges;
        self.revision += 1;
        self.updated_at = Utc::now();
    }

    /// Validate the definition and return a topological order. The order is
    /// also the accessible textual representation used by the UI.
    pub fn validate(&self) -> Result<Vec<String>, String> {
        if self.workspace_ref.as_ref().is_some_and(|reference| reference.trim().is_empty()
            || reference.len() > 256 || reference.chars().any(char::is_control)) {
            return Err("workspace_ref must be a nonempty git revision of at most 256 characters".into());
        }
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
            match (node.kind, &node.gate, &node.expand) {
                (WorkflowNodeKind::Task, Some(_), _) => {
                    return Err(format!("node {:?} is a task node but carries a gate; make it a gate node", node.id));
                }
                (WorkflowNodeKind::Task, _, Some(_)) => {
                    return Err(format!("node {:?} is a task node but carries expand policy; make it an expand node", node.id));
                }
                (WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval, None, _) => {
                    return Err(format!("control node {:?} needs a gate spec naming its step", node.id));
                }
                (WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval, Some(gate), None) => {
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
                (WorkflowNodeKind::Gate | WorkflowNodeKind::Review | WorkflowNodeKind::Approval, _, Some(_)) => {
                    return Err(format!(
                        "gate node {:?} cannot carry expand policy",
                        node.id
                    ));
                }
                (WorkflowNodeKind::Expand, Some(_), _) => {
                    return Err(format!("expand node {:?} cannot carry a gate", node.id));
                }
                (WorkflowNodeKind::Expand, None, None) => {
                    return Err(format!("expand node {:?} needs expand policy", node.id));
                }
                (WorkflowNodeKind::Expand, None, Some(expand)) => {
                    if expand.max_rework_rounds == 0 {
                        return Err(format!(
                            "expand node {:?} needs at least one rework round",
                            node.id
                        ));
                    }
                    let mut children = BTreeSet::new();
                    for child in &expand.children {
                        if !children.insert(child.as_str()) {
                            return Err(format!(
                                "expand node {:?} names child {child:?} twice",
                                node.id
                            ));
                        }
                    }
                }
                (WorkflowNodeKind::Task, None, None) => {}
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
        for node in self
            .nodes
            .iter()
            .filter(|node| node.kind == WorkflowNodeKind::Expand)
        {
            let expand = node.expand.as_ref().expect("expand shape checked above");
            if expand.join.tolerate as usize > expand.children.len() {
                return Err(format!(
                    "expand node {:?} tolerates {} failures but has only {} children",
                    node.id,
                    expand.join.tolerate,
                    expand.children.len()
                ));
            }
            for child in &expand.children {
                let Some(target) = self.node(child) else {
                    return Err(format!(
                        "expand node {:?} names missing child {child:?}",
                        node.id
                    ));
                };
                if target.kind != WorkflowNodeKind::Task {
                    return Err(format!(
                        "expand node {:?} child {child:?} is not a task node",
                        node.id
                    ));
                }
                // Joined means reached from the expand node. Not necessarily
                // by an edge straight from it or from a sibling: control-plan
                // injection puts gates between a child and what follows it,
                // and a part workflow's steps follow one another (#235).
                if !self.ancestors(child).contains(&node.id) {
                    return Err(format!(
                        "expand node {:?} child {child:?} is not joined to the expand graph",
                        node.id
                    ));
                }
            }
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
        // A part workflow's contract (#235) speaks first: "an exit that
        // leaves the part" says more than the generic "missing node".
        if self.part.is_some() {
            self.part_shape()?;
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
                let kind = match node.kind {
                    WorkflowNodeKind::Gate => "gate",
                    WorkflowNodeKind::Review => "review",
                    WorkflowNodeKind::Approval => "approval",
                    WorkflowNodeKind::Expand => "expand",
                    WorkflowNodeKind::Task => unreachable!(),
                };
                return Err(format!(
                    "{kind} node {:?} cannot declare exits; only a task node can",
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
                        let kind = match target.kind {
                            WorkflowNodeKind::Gate => "gate",
                            WorkflowNodeKind::Review => "review",
                            WorkflowNodeKind::Approval => "approval",
                            WorkflowNodeKind::Expand => "expand",
                            WorkflowNodeKind::Task => unreachable!(),
                        };
                        return Err(format!(
                            "node {:?} exit {ordinal} points back to {kind} node {:?}; name a task node",
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

    /// Approval is held by the subject run, not a harness task. Ignore the
    /// approval decision when admitting that run, but never its upstream
    /// work: otherwise an approval injected before a downstream task could
    /// bypass every policy gate on the task's former parents.
    pub fn prerequisite_edges(&self, target: &str) -> Vec<&WorkflowEdge> {
        let mut pending = vec![target.to_string()];
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        while let Some(target) = pending.pop() {
            if !seen.insert(target.clone()) { continue; }
            for edge in self.edges.iter().filter(|edge| edge.to == target) {
                if self.node(&edge.from).is_some_and(|node| node.kind == WorkflowNodeKind::Approval) {
                    pending.push(edge.from.clone());
                } else {
                    out.push(edge);
                }
            }
        }
        out
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
                WorkflowNodeKind::Expand => return None,
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
                        WorkflowNodeKind::Task | WorkflowNodeKind::Expand => return None,
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
            workspace_ref: None,
            category: task.category.clone(),
            inputs: Vec::new(),
            part: None,
            nodes: vec![WorkflowNode {
                session: Default::default(),
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
                expand: None,
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
        session: SessionPolicy::Fresh,
        id: id.to_string(),
        position: CanvasPoint {
            x: work.position.x + if before { -40.0 } else { 40.0 },
            // Leave room for functionary/required-by labels and rework
            // findings; control cards are taller than plain task cards.
            y: work.position.y + if before { -220.0 } else { 220.0 } * (index as f64 + 1.0),
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
        expand: None,
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

/// `{{part_id}}, {{part_title}}, ...` -- the list a refusal names.
fn part_inputs_words() -> String {
    PART_INPUTS
        .iter()
        .map(|name| format!("{{{{{name}}}}}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl WorkflowDefinition {
    /// Check this definition against the part-workflow contract (`#235`)
    /// and say which node plays each role:
    ///
    /// * exactly one **entry** node (no incoming edge), a task node;
    /// * exactly one **terminal** node (no outgoing edge), a task node --
    ///   `part.terminal`, if given, has to be that node;
    /// * exactly one **deliverable**: `part.deliverable`, or else the only
    ///   task node that works in a worktree. Its branch is what the
    ///   integrator merges, so it may not opt out of one;
    /// * no expand node -- decomposition is one level deep;
    /// * every exit stays inside the workflow;
    /// * no input but the reserved [`PART_INPUTS`], declared or written
    ///   `{{...}}`, and none of those spliced into a command the daemon
    ///   runs (an exit's `check:` or a gate's command).
    ///
    /// That it opens no PR of its own is the template's instructions' job;
    /// nothing here can see it. [`validate`](Self::validate) calls this for
    /// a definition that declares `part:`; the daemon calls it again at
    /// expansion, so a template edited after a plan named it still fails
    /// with these words. It assumes the graph itself is sound -- unique
    /// ids, edges between real nodes, no cycle.
    pub fn part_shape(&self) -> Result<PartShape, String> {
        let spec = self.part.clone().unwrap_or_default();
        if let Some(node) = self.nodes.iter().find(|n| n.kind == WorkflowNodeKind::Expand) {
            return Err(format!(
                "a part workflow cannot contain expand node {:?}: decomposition is one level deep",
                node.id
            ));
        }
        let ids: BTreeSet<&str> = self.nodes.iter().map(|n| n.id.as_str()).collect();
        for node in &self.nodes {
            for (index, exit) in node.exits.iter().enumerate() {
                if !ids.contains(exit.to.as_str()) {
                    return Err(format!(
                        "node {:?} exit {} leads to {:?}, which is outside this part workflow; a part's exits stay inside it",
                        node.id,
                        index + 1,
                        exit.to
                    ));
                }
            }
        }
        let one = |role: &str, rule: &str, found: Vec<&str>| -> Result<String, String> {
            match found.as_slice() {
                [only] => Ok(only.to_string()),
                [] => Err(format!("a part workflow needs exactly one {role} node ({rule}); it has none")),
                many => Err(format!(
                    "a part workflow needs exactly one {role} node ({rule}); it has {}: {}",
                    many.len(),
                    many.join(", ")
                )),
            }
        };
        let entry = one(
            "entry",
            "one with no incoming edge",
            self.nodes
                .iter()
                .filter(|n| !self.edges.iter().any(|e| e.to == n.id))
                .map(|n| n.id.as_str())
                .collect(),
        )?;
        let terminal = one(
            "terminal",
            "one with no outgoing edge",
            self.nodes
                .iter()
                .filter(|n| !self.edges.iter().any(|e| e.from == n.id))
                .map(|n| n.id.as_str())
                .collect(),
        )?;
        if let Some(declared) = &spec.terminal {
            if declared != &terminal {
                return Err(format!(
                    "part.terminal names {declared:?}, but the node nothing follows is {terminal:?}"
                ));
            }
        }
        for (role, id) in [("entry", &entry), ("terminal", &terminal)] {
            if self.node(id).is_some_and(|n| n.kind != WorkflowNodeKind::Task) {
                return Err(format!("the part workflow's {role} node {id:?} has to be a task node"));
            }
        }
        let deliverable = match &spec.deliverable {
            Some(id) => {
                let node = self
                    .node(id)
                    .ok_or_else(|| format!("part.deliverable names {id:?}, which is not a node of this workflow"))?;
                if node.kind != WorkflowNodeKind::Task {
                    return Err(format!("part.deliverable {id:?} has to be a task node"));
                }
                if node.task.worktree == Some(false) {
                    return Err(format!(
                        "part.deliverable {id:?} works without a worktree, but its branch is what the integrator merges; give it `worktree: true`"
                    ));
                }
                id.clone()
            }
            None => {
                let candidates: Vec<&str> = self
                    .nodes
                    .iter()
                    .filter(|n| n.kind == WorkflowNodeKind::Task && n.task.worktree != Some(false))
                    .map(|n| n.id.as_str())
                    .collect();
                match candidates.as_slice() {
                    [only] => only.to_string(),
                    [] => {
                        return Err(
                            "no task node of this part workflow works in a worktree, but the deliverable's branch is what the integrator merges".into(),
                        )
                    }
                    many => {
                        return Err(format!(
                            "{} task nodes work in a worktree ({}); name the one whose branch is integrated with part.deliverable",
                            many.len(),
                            many.join(", ")
                        ))
                    }
                }
            }
        };
        for input in &self.inputs {
            if !PART_INPUTS.contains(&input.name.as_str()) {
                return Err(format!(
                    "a part workflow is filled with each part's own values and takes no other input; it declares {:?}, but only {} exist",
                    input.name,
                    part_inputs_words()
                ));
            }
        }
        for node in &self.nodes {
            let texts = [&node.task.title, &node.task.instructions].into_iter().chain(node.task.labels.values());
            for text in texts {
                if let Some(unknown) = placeholders(text).into_iter().find(|name| !PART_INPUTS.contains(name)) {
                    return Err(format!(
                        "node {:?} uses {{{{{unknown}}}}}, which is not a part input; a part workflow may only use {}",
                        node.id,
                        part_inputs_words()
                    ));
                }
            }
            let commands = node
                .exits
                .iter()
                .filter_map(|exit| exit.check.as_deref())
                .chain(node.gate.as_ref().and_then(|gate| gate.command.as_deref()));
            for command in commands {
                if let Some(name) = placeholders(command).into_iter().next() {
                    return Err(format!(
                        "node {:?} writes {{{{{name}}}}} into a command; part values are never spliced into a command the daemon runs",
                        node.id
                    ));
                }
            }
        }
        Ok(PartShape { entry, deliverable, terminal })
    }

    /// This part workflow's copy for one part (`#235`): every node and edge
    /// namespaced `<part>-<id>` ([`part_node_id`]), exit targets and gate
    /// subjects rewritten to match, and `values` (the [`PART_INPUTS`])
    /// written into each node's title and label values and each task
    /// node's instructions. A task node that uses no part input is not
    /// left blind to its part: its title gets `: <part title>` appended and
    /// `brief` -- what a part without a template is told -- follows its own
    /// instructions (or is them, when it has none). Commands are never
    /// substituted. `shape` is [`part_shape`](Self::part_shape)'s answer;
    /// this assumes the contract holds.
    pub fn expand_part(
        &self,
        shape: &PartShape,
        part: &str,
        values: &BTreeMap<String, String>,
        brief: &str,
    ) -> PartCopy {
        let ns = |id: &str| part_node_id(part, id);
        let uses_part = |text: &str| placeholders(text).iter().any(|name| PART_INPUTS.contains(name));
        let part_title = values.get("part_title").map(String::as_str).unwrap_or(part);
        let nodes = self
            .nodes
            .iter()
            .map(|node| {
                let mut copy = node.clone();
                copy.id = ns(&node.id);
                for exit in &mut copy.exits {
                    exit.to = ns(&exit.to);
                }
                if let Some(subject) = copy.gate.as_mut().and_then(|gate| gate.subject.as_mut()) {
                    *subject = ns(subject);
                }
                let task_node = node.kind == WorkflowNodeKind::Task;
                copy.task.title = if task_node && !uses_part(&node.task.title) {
                    format!("{}: {part_title}", node.task.title.trim())
                } else {
                    substitute(&node.task.title, values)
                };
                if task_node {
                    copy.task.instructions = if uses_part(&node.task.instructions) {
                        substitute(&node.task.instructions, values)
                    } else if node.task.instructions.trim().is_empty() {
                        brief.to_string()
                    } else {
                        format!("{}\n\n---\n{brief}", node.task.instructions.trim_end())
                    };
                }
                for value in copy.task.labels.values_mut() {
                    *value = substitute(value, values);
                }
                copy
            })
            .collect();
        let edges = self
            .edges
            .iter()
            .map(|edge| WorkflowEdge { id: ns(&edge.id), from: ns(&edge.from), to: ns(&edge.to) })
            .collect();
        PartCopy {
            nodes,
            edges,
            shape: PartShape {
                entry: ns(&shape.entry),
                deliverable: ns(&shape.deliverable),
                terminal: ns(&shape.terminal),
            },
        }
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

pub use factory_kernel::WorkflowRunStatus;

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
    /// Run history of the same task, oldest first. Legacy task ids stay separate.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<WorkflowAttempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// Persisted creation receipt distinguishes crash repair from deletion.
    /// Legacy ids are conservatively treated as previously created.
    #[serde(default = "assume_task_created", skip_serializing_if = "is_true")]
    pub task_created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// How many times work was sent back through this node: 0 on its first
    /// pass; otherwise the rounds it has run again for, which every run of
    /// its task records as `workflow_round` -- strictly increasing, so a
    /// newer round never mistakes the last one's `done` for its own.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub round: u32,
    /// How many of those rounds the integrator sent it back for (`#235`):
    /// a merge conflict or a failed combined check on its part. Counted
    /// apart so a part's review loop and its integration rework each keep
    /// their own budget -- see [`exit_rounds`](Self::exit_rounds).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub integration_rounds: u32,
    /// Legacy tasks earlier rounds spawned here, oldest first. Kept so their
    /// history stays findable; never mirrored, never recreated.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub superseded_task_ids: Vec<String>,
    /// On the node work was sent back to: who sent it and why, which its
    /// next run is dispatched with.
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

impl WorkflowNodeRun {
    /// The rounds this node's own backwards exits have used: every round
    /// but the ones integration rework added (`#235`). What an exit's
    /// `max_rounds` is checked against.
    pub fn exit_rounds(&self) -> u32 {
        self.round.saturating_sub(self.integration_rounds)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowAttempt {
    pub run_id: String,
    pub attempt: u32,
    pub round: u32,
    pub status: crate::run::RunStatus,
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
    /// Daemon-produced integration or combined-gate feedback.  Ordinary
    /// agent exits keep this empty and read the sending task's result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn assume_task_created() -> bool {
    true
}
fn is_true(value: &bool) -> bool {
    *value
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
    /// Present only for an intake decomposition that is assembled on one
    /// integration branch and handed over as one pull request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration: Option<WorkflowIntegration>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl WorkflowRun {
    /// Integration tasks start on the single writer's branch. Other
    /// revision-pinned workflows start on their frozen snapshot commit.
    pub fn task_workspace(&self) -> Option<crate::task::WorkflowWorkspace> {
        self.integration.as_ref().map(|integration| integration.branch.clone())
            .or_else(|| self.definition.workspace_ref.clone())
            .map(|base_ref| crate::task::WorkflowWorkspace { base_ref })
    }
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
                    attempts: Vec::new(),
                    task_id: None,
                    task_created: false,
                    error: None,
                    round: 0,
                    integration_rounds: 0,
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
            integration: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// One child slice's contract at the integration boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationPart {
    /// The part's **deliverable**: the task node whose newest run's branch
    /// is merged, and which a merge conflict or a failed combined check is
    /// sent back to. `merged_nodes` is keyed by it.
    pub node_id: String,
    pub part_id: String,
    pub acceptance: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub owns: Vec<String>,
    /// The part's **terminal** node, whose `done` releases the merge, when
    /// the part ran through a part workflow (`#235`). Absent means the
    /// part is the one node `node_id` -- every run stored before #235, and
    /// every plan without a template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_node: Option<String>,
}

impl IntegrationPart {
    /// The node whose `done` releases this part's merge.
    pub fn terminal(&self) -> &str {
        self.terminal_node.as_deref().unwrap_or(&self.node_id)
    }
}

/// Durable state for the single-writer integration phase of a decomposed
/// intake item.  Paths and merged/check state live on the workflow run so a
/// daemon restart reconciles instead of starting a second branch or PR.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowIntegration {
    pub parent_task_id: String,
    pub base_ref: String,
    pub branch: String,
    pub worktree_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_number: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<IntegrationPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merged_nodes: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub checks_passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_url: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cleanup_complete: bool,
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
    /// `from` goes back to `unstarted` with its task retained and its round
    /// counted up, the target is told
    /// who sent it back, and the not-yet-started nodes below `from` (a gate
    /// mirrored `skipped` off the failed run, say) are `unstarted` again.
    /// Nothing is dispatched here; the next advance starts another run.
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
        let Some((used, sequence)) = self
            .nodes
            .iter()
            .find(|n| n.node_id == from)
            .map(|n| (n.exit_rounds(), n.round))
        else {
            return SendBack::NoExit;
        };
        let max_rounds = spec.max_rounds.unwrap_or(0);
        if used >= max_rounds {
            return SendBack::Exhausted { max_rounds };
        }
        let from_task = self.nodes.iter().find(|n| n.node_id == from).and_then(|n| n.task_id.clone()).unwrap_or_default();
        let downstream = self.definition.descendants(from);
        // `round` counts this exit's rounds against its budget; `next` is
        // the run sequence every node in the body moves to. They are the
        // same number unless integration rework (#235) has sent this part
        // round before, which never spends a review's budget.
        let round = used + 1;
        let next = sequence + 1;
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
            // The task is standing intent; another round is another run.
            // Leave legacy superseded ids untouched rather than rewriting history.
            node.status = WorkflowNodeStatus::Unstarted;
            node.error = None;
            node.round = next;
            node.exits_evaluated = false;
            node.routed_to = None;
            node.skip_reason = None;
            node.rework_request = (id == spec.to).then(|| ReworkRequest {
                from_node: from.to_string(),
                from_task: from_task.clone(),
                round,
                max_rounds,
                feedback: None,
            });
        }
        self.updated_at = Utc::now();
        SendBack::Sent { round, max_rounds }
    }

    /// The integration part `node_id` belongs to (`#235`): the one a task
    /// node's `decomposition_part` names, or the one of the task node a
    /// control node judges. `None` outside an integrated decomposition.
    pub fn part_of(&self, node_id: &str) -> Option<&IntegrationPart> {
        let integration = self.integration.as_ref()?;
        let node = self.definition.node(node_id)?;
        let work = if node.kind == WorkflowNodeKind::Task {
            node
        } else {
            self.definition.node(&self.definition.gate_subject(node_id)?)?
        };
        let part = work.task.decomposition_part.as_deref()?;
        integration.parts.iter().find(|candidate| candidate.part_id == part)
    }

    /// The parts that have to be merged before any of `part`'s work may
    /// start: every other part one of its nodes descends from. With a part
    /// workflow, the edge between two parts runs from the prerequisite's
    /// terminal to the dependant's entry, and neither end is a deliverable,
    /// so this reads ancestry rather than direct edges.
    pub fn prerequisite_parts(&self, part: &IntegrationPart) -> BTreeSet<String> {
        self.definition
            .ancestors(&part.node_id)
            .iter()
            .filter_map(|id| self.definition.node(id))
            .filter(|node| node.kind == WorkflowNodeKind::Task)
            .filter_map(|node| node.task.decomposition_part.clone())
            .filter(|id| id != &part.part_id)
            .collect()
    }

    /// Whether the part with id `part_id` is on the integration branch.
    pub fn part_merged(&self, part_id: &str) -> bool {
        self.integration.as_ref().is_some_and(|integration| {
            integration.parts.iter().any(|part| {
                part.part_id == part_id && integration.merged_nodes.contains(&part.node_id)
            })
        })
    }

    /// Integration sent `part`'s deliverable back (`#235`): put every node
    /// of the part between the deliverable and its terminal back to
    /// `unstarted` for another round on the same task -- so the fix made
    /// during integration is reviewed before it is merged -- and the
    /// part's own not-yet-started nodes below the terminal (its gates)
    /// with them, so none of them keeps showing the previous round's
    /// `done`. The deliverable itself is the caller's: it is continued at
    /// once rather than waiting for the graph. A part with no
    /// `terminal_node` is the one deliverable node, and nothing else moves
    /// -- exactly what integration rework always did.
    pub fn rework_part_body(&mut self, part: &IntegrationPart) {
        if part.terminal_node.is_none() {
            return;
        }
        let terminal = part.terminal().to_string();
        let below_deliverable = self.definition.descendants(&part.node_id);
        let above_terminal = self.definition.ancestors(&terminal);
        let below_terminal = self.definition.descendants(&terminal);
        let ids: Vec<String> = self.nodes.iter().map(|node| node.node_id.clone()).collect();
        for id in ids {
            if id == part.node_id || self.part_of(&id).is_none_or(|owner| owner.part_id != part.part_id) {
                continue;
            }
            let on_path = below_deliverable.contains(&id) && (id == terminal || above_terminal.contains(&id));
            let Some(node) = self.nodes.iter_mut().find(|node| node.node_id == id) else { continue };
            if on_path {
                node.status = WorkflowNodeStatus::Unstarted;
                node.error = None;
                node.round += 1;
                node.integration_rounds += 1;
                node.exits_evaluated = false;
                node.routed_to = None;
                node.skip_reason = None;
                node.rework_request = None;
            } else if below_terminal.contains(&id) && node.task_id.is_none() {
                node.status = WorkflowNodeStatus::Unstarted;
                node.error = None;
            }
        }
        self.updated_at = Utc::now();
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
            session: Default::default(),
            id: id.into(),
            position: CanvasPoint::default(),
            kind: WorkflowNodeKind::Task,
            task: NewTask {
                title: id.into(),
                ..Default::default()
            },
            gate: None,
            exits: Vec::new(),
            expand: None,
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
    fn approval_admission_still_requires_every_upstream_gate() {
        let mut approval = node("approval");
        approval.kind = WorkflowNodeKind::Approval;
        let mut second = approval.clone();
        second.id = "second".into();
        let def = definition(vec![node("tested"), approval, second, node("deploy")], vec![
            WorkflowEdge { id: "tested-approval".into(), from: "tested".into(), to: "approval".into() },
            WorkflowEdge { id: "approval-second".into(), from: "approval".into(), to: "second".into() },
            WorkflowEdge { id: "second-deploy".into(), from: "second".into(), to: "deploy".into() },
        ]);
        assert_eq!(def.prerequisite_edges("deploy").iter().map(|e| e.from.as_str()).collect::<Vec<_>>(), ["tested"]);
        let root = definition(vec![def.nodes[1].clone(), node("deploy")], vec![
            WorkflowEdge { id: "approval-deploy".into(), from: "approval".into(), to: "deploy".into() },
        ]);
        assert!(root.prerequisite_edges("deploy").is_empty(), "a root approval may hold a subject run before dispatch");
    }

    #[test]
    fn workspace_revision_is_optional_validated_and_frozen_in_a_run() {
        let mut def = definition(vec![node("release")], vec![]);
        assert!(def.workspace_ref.is_none());
        assert!(serde_json::to_value(&def).unwrap().get("workspace_ref").is_none());
        for invalid in ["", " \t ", "main\nHEAD"] {
            def.workspace_ref = Some(invalid.into());
            assert!(def.validate().is_err(), "{invalid:?}");
        }
        def.workspace_ref = Some("a".repeat(40));
        def.validate().unwrap();
        let encoded = serde_json::to_value(&def).unwrap();
        let decoded: WorkflowDefinition = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.workspace_ref, def.workspace_ref);
    }

    #[test]
    fn feedback_session_policy_defaults_to_resume_and_round_trips_fresh() {
        let legacy: WorkflowNode = serde_yaml_ng::from_str("id: review\ntask: {title: Review}").unwrap();
        assert_eq!(legacy.session, SessionPolicy::Resume);
        assert!(serde_json::to_value(&legacy).unwrap().get("session").is_none());
        let mut independent = legacy;
        independent.session = SessionPolicy::Fresh;
        let json = serde_json::to_value(&independent).unwrap();
        assert_eq!(json["session"], "fresh");
        assert_eq!(serde_json::from_value::<WorkflowNode>(json).unwrap().session, SessionPolicy::Fresh);
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
    fn expand_nodes_validate_their_explicit_children_and_join_policy() {
        let mut expand = node("expand");
        expand.kind = WorkflowNodeKind::Expand;
        expand.expand = Some(ExpandSpec {
            children: vec!["a".into(), "b".into()],
            join: ExpandJoin { tolerate: 1 },
            cancel: ExpandCancelPolicy::Abandon,
            max_rework_rounds: 2,
        });
        let valid = definition(
            vec![expand.clone(), node("a"), node("b")],
            vec![
                WorkflowEdge {
                    id: "expand-a".into(),
                    from: "expand".into(),
                    to: "a".into(),
                },
                WorkflowEdge {
                    id: "a-b".into(),
                    from: "a".into(),
                    to: "b".into(),
                },
            ],
        );
        valid.validate().unwrap();

        expand.expand.as_mut().unwrap().join.tolerate = 3;
        let invalid = definition(
            vec![expand, node("a"), node("b")],
            vec![
                WorkflowEdge {
                    id: "expand-a".into(),
                    from: "expand".into(),
                    to: "a".into(),
                },
                WorkflowEdge {
                    id: "a-b".into(),
                    from: "a".into(),
                    to: "b".into(),
                },
            ],
        );
        assert!(invalid
            .validate()
            .unwrap_err()
            .contains("tolerates 3 failures"));
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
            session: Default::default(),
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
            expand: None,
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
        let gate = out.nodes.iter().find(|n| n.kind == WorkflowNodeKind::Gate).unwrap();
        let subject = out.nodes.iter().find(|n| n.id == "a").unwrap();
        assert!(subject.position.y - approval.position.y >= 220.0);
        assert!(gate.position.y - subject.position.y >= 220.0);
        assert!(review.position.y - gate.position.y >= 220.0);
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
        assert_eq!(implement.task_id.as_deref(), Some("i1"));
        assert!(implement.superseded_task_ids.is_empty());
        assert_eq!(
            implement.rework_request,
            Some(ReworkRequest {
                from_node: "review".into(),
                from_task: "r1".into(),
                round: 1,
                max_rounds: 2,
                feedback: None,
            })
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
            Vec::<String>::new(),
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

    #[test]
    fn the_factory_dependency_scan_example_is_a_valid_workflow() {
        let draft: WorkflowDraft = serde_yaml_ng::from_str(include_str!(
            "../../../workflows/factory-dependency-scan.yaml"
        ))
        .unwrap();
        let definition = WorkflowDefinition::from_draft(draft);
        assert_eq!(
            definition.validate().unwrap(),
            vec!["built", "running", "source-reachability"]
        );
        let source = definition
            .nodes
            .iter()
            .find(|node| node.id == "source-reachability")
            .unwrap();
        assert_eq!(
            source.task.instructions,
            "examples/dependency-scan.sh --reachability"
        );
        assert_eq!(
            source.task.labels.get("lifecycle").map(String::as_str),
            Some("declared")
        );
        assert!(
            definition.edges.is_empty(),
            "source analysis cannot describe the separate release/installed scans"
        );
    }

    // --- #235: part workflows -------------------------------------------

    /// implement (its own worktree) -> review (none), review sending the
    /// work back at most `max_rounds` times: the smallest part workflow.
    fn part_template(max_rounds: u32) -> WorkflowDefinition {
        let mut implement = node("implement");
        implement.task.worktree = Some(true);
        implement.task.title = "Implement {{part_title}}".into();
        implement.task.instructions = "Do {{part_instructions}}; done when {{part_acceptance}}".into();
        implement.task.labels.insert("part".into(), "{{part_id}}".into());
        let mut review = node("review");
        review.task.worktree = Some(false);
        review.exits = vec![WorkflowExit {
            to: "implement".into(),
            check: None,
            agent: Some("fixable findings".into()),
            max_rounds: Some(max_rounds),
        }];
        let mut template = definition(vec![implement, review], vec![e("implement", "review")]);
        template.part = Some(PartSpec::default());
        template
    }

    #[test]
    fn the_epic_part_example_is_a_valid_part_workflow() {
        let draft: WorkflowDraft =
            serde_yaml_ng::from_str(include_str!("../../../workflows/epic-part.yaml")).unwrap();
        let definition = WorkflowDefinition::from_draft(draft);
        assert_eq!(definition.validate().unwrap(), vec!["implement", "review"]);
        assert_eq!(
            definition.part_shape().unwrap(),
            PartShape { entry: "implement".into(), deliverable: "implement".into(), terminal: "review".into() }
        );
        let review = definition.nodes.iter().find(|n| n.id == "review").unwrap();
        assert_eq!(review.exits[0].max_rounds, Some(5));
        assert_eq!(review.task.worktree, Some(true), "review detaches onto the part branch in a worktree of its own");
        for node in &definition.nodes {
            assert!(!node.task.instructions.contains("gh pr create"), "a part opens no PR of its own");
        }
    }

    #[test]
    fn a_part_workflow_derives_its_roles_when_the_graph_says_them() {
        let template = part_template(5);
        template.validate().unwrap();
        assert_eq!(
            template.part_shape().unwrap(),
            PartShape { entry: "implement".into(), deliverable: "implement".into(), terminal: "review".into() }
        );
        // One node is all three roles.
        let mut single = definition(vec![node("work")], vec![]);
        single.part = Some(PartSpec::default());
        assert_eq!(single.part_shape().unwrap().terminal, "work");
    }

    #[test]
    fn the_part_workflow_contract_refuses_with_words() {
        let refused = |change: &dyn Fn(&mut WorkflowDefinition)| {
            let mut template = part_template(5);
            change(&mut template);
            template.validate().unwrap_err()
        };

        let two_terminals = refused(&|t| {
            t.nodes.push(node("docs"));
            t.nodes.last_mut().unwrap().task.worktree = Some(false);
            t.edges.push(e("implement", "docs"));
        });
        assert!(two_terminals.contains("exactly one terminal node"), "{two_terminals}");
        assert!(two_terminals.contains("docs") && two_terminals.contains("review"), "{two_terminals}");

        let two_entries = refused(&|t| {
            t.nodes.push(node("plan"));
            t.nodes.last_mut().unwrap().task.worktree = Some(false);
            t.edges.push(e("plan", "review"));
        });
        assert!(two_entries.contains("exactly one entry node"), "{two_entries}");

        let leaves = refused(&|t| t.nodes[1].exits[0].to = "ship".into());
        assert!(leaves.contains("outside this part workflow"), "{leaves}");

        let unknown = refused(&|t| t.nodes[0].task.title = "Implement #{{issue}}".into());
        assert!(unknown.contains("{{issue}}") && unknown.contains("not a part input"), "{unknown}");
        assert!(unknown.contains("{{part_title}}"), "the refusal lists what may be used: {unknown}");

        let declared = refused(&|t| t.inputs.push(WorkflowInput { name: "issue".into(), description: String::new() }));
        assert!(declared.contains("takes no other input"), "{declared}");

        let spliced = refused(&|t| {
            t.nodes[1].exits.insert(0, WorkflowExit {
                to: "implement".into(),
                check: Some("test -f {{part_id}}.txt".into()),
                agent: None,
                max_rounds: Some(1),
            });
        });
        assert!(spliced.contains("into a command"), "{spliced}");

        let ambiguous = refused(&|t| t.nodes[1].task.worktree = None);
        assert!(ambiguous.contains("name the one whose branch is integrated with part.deliverable"), "{ambiguous}");

        let no_worktree = refused(&|t| {
            t.nodes[0].task.worktree = Some(false);
            t.part = Some(PartSpec { deliverable: Some("implement".into()), terminal: None });
        });
        assert!(no_worktree.contains("worktree: true"), "{no_worktree}");

        let wrong_terminal = refused(&|t| t.part = Some(PartSpec { deliverable: None, terminal: Some("implement".into()) }));
        assert!(wrong_terminal.contains("the node nothing follows is \"review\""), "{wrong_terminal}");

        // The contract is only a part workflow's: the same graph with two
        // ends is an ordinary workflow.
        let mut ordinary = part_template(5);
        ordinary.part = None;
        ordinary.nodes.push(node("docs"));
        ordinary.edges.push(e("implement", "docs"));
        ordinary.validate().unwrap();

        // One level deep: an expand node is no part.
        let mut nested = part_template(5);
        nested.nodes.push(WorkflowNode {
            kind: WorkflowNodeKind::Expand,
            expand: Some(ExpandSpec { max_rework_rounds: 3, ..Default::default() }),
            ..node("inner")
        });
        assert!(nested.part_shape().unwrap_err().contains("one level deep"));
    }

    #[test]
    fn expanding_a_part_namespaces_every_reference_and_writes_the_part_in() {
        let mut template = part_template(5);
        template.nodes[1].task.title = "Review".into();
        template.nodes[1].task.instructions = "Look hard.".into();
        let mut tests = gate_node("tests", "tests");
        tests.gate.as_mut().unwrap().command = Some("cargo test".into());
        tests.gate.as_mut().unwrap().subject = Some("implement".into());
        template.nodes.push(tests);
        template.edges.push(e("implement", "tests"));
        template.edges.push(e("tests", "review"));
        template.edges.retain(|edge| edge.id != "implement-review");
        let shape = template.part_shape().unwrap();
        let values: BTreeMap<String, String> = [
            ("part_id", "api"),
            ("part_title", "The API"),
            ("part_instructions", "add the endpoint"),
            ("part_acceptance", "cargo test api"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

        let copy = template.expand_part(&shape, "api", &values, "THE BRIEF");
        let ids: Vec<&str> = copy.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["api-implement", "api-review", "api-tests"]);
        assert_eq!(
            copy.shape,
            PartShape { entry: "api-implement".into(), deliverable: "api-implement".into(), terminal: "api-review".into() }
        );
        let implement = &copy.nodes[0];
        assert_eq!(implement.task.title, "Implement The API");
        assert_eq!(implement.task.instructions, "Do add the endpoint; done when cargo test api");
        assert_eq!(implement.task.labels["part"], "api");
        let review = &copy.nodes[1];
        assert_eq!(review.exits[0].to, "api-implement");
        assert_eq!(review.task.title, "Review: The API", "a title with no part input still names the part");
        assert_eq!(review.task.instructions, "Look hard.\n\n---\nTHE BRIEF");
        let gate = copy.nodes[2].gate.as_ref().unwrap();
        assert_eq!(gate.subject.as_deref(), Some("api-implement"));
        assert_eq!(gate.command.as_deref(), Some("cargo test"), "commands are never substituted");
        let edges: Vec<(&str, &str, &str)> =
            copy.edges.iter().map(|e| (e.id.as_str(), e.from.as_str(), e.to.as_str())).collect();
        assert!(edges.contains(&("api-implement-tests", "api-implement", "api-tests")), "{edges:?}");
        assert!(edges.contains(&("api-tests-review", "api-tests", "api-review")), "{edges:?}");

        // A step with no instructions of its own is the brief.
        let mut bare = part_template(5);
        bare.nodes[1].task.instructions = String::new();
        let shape = bare.part_shape().unwrap();
        assert_eq!(bare.expand_part(&shape, "api", &values, "THE BRIEF").nodes[1].task.instructions, "THE BRIEF");
    }

    /// Two parts of `part_template`, `b` after `a`, under one expand node --
    /// what `start_decomposition_workflow` generates, minus the task fields.
    fn two_part_run(category: &str) -> WorkflowRun {
        let template = part_template(1);
        let shape = template.part_shape().unwrap();
        let mut nodes = vec![WorkflowNode {
            kind: WorkflowNodeKind::Expand,
            expand: Some(ExpandSpec { max_rework_rounds: 3, ..Default::default() }),
            ..node("expand")
        }];
        let mut edges = vec![e("expand", "a-implement")];
        for part in ["a", "b"] {
            let copy = template.expand_part(&shape, part, &BTreeMap::new(), "brief");
            for mut copied in copy.nodes {
                copied.task.decomposition_part = Some(part.into());
                nodes.push(copied);
            }
            edges.extend(copy.edges);
        }
        edges.push(e("a-review", "b-implement"));
        let children = ["a-implement", "a-review", "b-implement", "b-review"].map(String::from).to_vec();
        nodes[0].expand.as_mut().unwrap().children = children;
        let mut def = definition(nodes, edges);
        def.category = Some(category.into());
        let mut run = WorkflowRun::new(def, WorkflowActor::Owner);
        with_tasks(&mut run);
        run.integration = Some(WorkflowIntegration {
            parts: ["a", "b"]
                .map(|part| IntegrationPart {
                    node_id: format!("{part}-implement"),
                    part_id: part.into(),
                    acceptance: "true".into(),
                    owns: Vec::new(),
                    terminal_node: Some(format!("{part}-review")),
                })
                .to_vec(),
            ..Default::default()
        });
        run
    }

    /// Every executable node has its task from the start, as the expand
    /// node materialises them.
    fn with_tasks(run: &mut WorkflowRun) {
        for node in &mut run.nodes {
            let executable = run.definition.nodes.iter().any(|n| {
                n.id == node.node_id && matches!(n.kind, WorkflowNodeKind::Task | WorkflowNodeKind::Review)
            });
            if executable {
                node.task_id = Some(format!("task-{}", node.node_id));
            }
        }
    }

    #[test]
    fn injection_reaches_every_copied_task_node_and_the_expand_graph_stays_joined() {
        let run = two_part_run("feature");
        run.definition.validate().unwrap();
        let plans = plan("feature", &[("tests", Some("cargo test"), None)]);
        let (injected, notes) = run.definition.inject(&plans);
        injected.validate().expect("gates between a part's steps keep every child joined to expand");
        for work in ["a-implement", "a-review", "b-implement", "b-review"] {
            let gate = injected.nodes.iter().find(|n| n.id == format!("{work}.tests")).unwrap();
            assert_eq!(gate.gate.as_ref().unwrap().subject.as_deref(), Some(work));
            assert!(notes.iter().any(|note| note.node_id == work && note.gate_node_id == gate.id));
        }
        // The edge between the parts now leaves a's review's gate.
        assert!(injected.edges.iter().any(|edge| edge.from == "a-review.tests" && edge.to == "b-implement"));

        let mut injected_run = run.clone();
        injected_run.definition = injected;
        let b = injected_run.integration.as_ref().unwrap().parts[1].clone();
        assert_eq!(injected_run.prerequisite_parts(&b), BTreeSet::from(["a".to_string()]), "read through the gates");
        let a = injected_run.integration.as_ref().unwrap().parts[0].clone();
        assert!(injected_run.prerequisite_parts(&a).is_empty(), "its own steps are not a prerequisite");
        assert_eq!(injected_run.part_of("b-review.tests").map(|p| p.part_id.as_str()), Some("b"));
        assert_eq!(injected_run.part_of("expand"), None);
    }

    #[test]
    fn integration_rework_sends_the_part_down_to_its_terminal_and_nothing_else() {
        let mut run = two_part_run("feature");
        let plans = plan("feature", &[("tests", Some("cargo test"), None)]);
        run.definition = run.definition.inject(&plans).0;
        run.nodes = WorkflowRun::new(run.definition.clone(), WorkflowActor::Owner).nodes;
        with_tasks(&mut run);
        for node in &mut run.nodes {
            node.status = WorkflowNodeStatus::Done;
            node.exits_evaluated = true;
        }
        let a = run.integration.as_ref().unwrap().parts[0].clone();
        run.rework_part_body(&a);

        let status = |run: &WorkflowRun, id: &str| node_status_of(run, id);
        assert_eq!(status(&run, "a-implement"), WorkflowNodeStatus::Done, "the deliverable is the caller's");
        for id in ["a-implement.tests", "a-review"] {
            assert_eq!(status(&run, id), WorkflowNodeStatus::Unstarted, "{id}");
        }
        assert_eq!(status(&run, "a-review.tests"), WorkflowNodeStatus::Unstarted, "no stale done below the terminal");
        let review = run.nodes.iter().find(|n| n.node_id == "a-review").unwrap();
        assert_eq!((review.round, review.integration_rounds, review.exit_rounds()), (1, 1, 0));
        assert!(!review.exits_evaluated);
        for id in ["b-implement", "b-implement.tests", "b-review", "b-review.tests"] {
            assert_eq!(status(&run, id), WorkflowNodeStatus::Done, "another part is untouched: {id}");
        }

        // A single-node part is what integration rework always moved: the
        // deliverable, which is the caller's -- nothing here.
        let mut legacy = run.clone();
        for node in &mut legacy.nodes {
            node.status = WorkflowNodeStatus::Done;
        }
        let mut single = legacy.integration.as_ref().unwrap().parts[1].clone();
        single.terminal_node = None;
        legacy.rework_part_body(&single);
        assert!(legacy.nodes.iter().all(|n| n.status == WorkflowNodeStatus::Done));
    }

    fn node_status_of(run: &WorkflowRun, id: &str) -> WorkflowNodeStatus {
        run.nodes.iter().find(|n| n.node_id == id).unwrap_or_else(|| panic!("no node {id}")).status
    }

    #[test]
    fn integration_rework_never_spends_a_reviews_rounds() {
        let mut run = two_part_run("feature");
        for node in &mut run.nodes {
            node.status = WorkflowNodeStatus::Done;
        }
        // Integration sent part a back once: the deliverable and the
        // review both moved a round on, neither by its own exit.
        for id in ["a-implement", "a-review"] {
            let node = run.nodes.iter_mut().find(|n| n.node_id == id).unwrap();
            node.round = 1;
            node.integration_rounds = 1;
        }
        assert_eq!(
            run.send_back("a-review", "a-implement"),
            SendBack::Sent { round: 1, max_rounds: 1 },
            "the review still has its one round"
        );
        let implement = run.nodes.iter().find(|n| n.node_id == "a-implement").unwrap();
        assert_eq!(implement.round, 2, "the run sequence still moves forward");
        assert_eq!(implement.rework_request.as_ref().unwrap().round, 1);
        assert_eq!(node_status_of(&run, "b-implement"), WorkflowNodeStatus::Done, "the loop stays inside its part");
        run.nodes.iter_mut().find(|n| n.node_id == "a-review").unwrap().status = WorkflowNodeStatus::Done;
        assert_eq!(run.send_back("a-review", "a-implement"), SendBack::Exhausted { max_rounds: 1 });
    }

    #[test]
    fn a_run_stored_before_part_workflows_reads_each_part_as_its_own_terminal() {
        let part: IntegrationPart =
            serde_json::from_value(serde_json::json!({ "node_id": "api", "part_id": "api", "acceptance": "true" })).unwrap();
        assert_eq!(part.terminal(), "api");
        assert!(serde_json::to_value(&part).unwrap().get("terminal_node").is_none(), "nothing new is written for it");
        let node: WorkflowNodeRun =
            serde_json::from_value(serde_json::json!({ "node_id": "api", "status": "done", "round": 2 })).unwrap();
        assert_eq!((node.round, node.integration_rounds, node.exit_rounds()), (2, 0, 2));
        let definition: WorkflowDefinition = serde_json::from_value(serde_json::to_value(definition(vec![node_named("a")], vec![])).unwrap()).unwrap();
        assert!(definition.part.is_none());
    }

    fn node_named(id: &str) -> WorkflowNode {
        node(id)
    }
}
