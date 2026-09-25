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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
                (WorkflowNodeKind::Gate, None) => {
                    return Err(format!("gate node {:?} needs a gate: a step and a command", node.id));
                }
                (WorkflowNodeKind::Gate, Some(gate)) => {
                    if !control_plan::is_name(&gate.step) {
                        return Err(format!("gate node {:?} names step {:?}, which is not a step name", node.id, gate.step));
                    }
                    // A locked gate is the plan's, and a plan may require a
                    // gate nobody gave a command -- it blocks, which is the
                    // point. An author placing one by hand says what it runs.
                    if !gate.locked && gate.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
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
        for node in self.nodes.iter().filter(|n| n.kind == WorkflowNodeKind::Gate) {
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
                WorkflowNodeKind::Gate => {
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
            .filter(|n| n.kind == WorkflowNodeKind::Gate)
            .filter(|n| self.gate_subject(&n.id).as_deref() == Some(node_id))
            .filter_map(|n| {
                let gate = n.gate.as_ref()?;
                Some(RequiredStep {
                    step: gate.step.clone(),
                    kind: StepKind::Gate,
                    command: gate.command.clone(),
                    timeout_seconds: gate.timeout_seconds,
                    required_by: gate.required_by.clone(),
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
                if let Some(existing) = authored.iter().find(|g| g.gate.as_ref().is_some_and(|g| g.step == step.step)) {
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

            let mut previous = work.id.clone();
            let mut chain_edges = Vec::new();
            for (i, step) in to_inject.iter().enumerate() {
                let gate_id = fresh(format!("{}.{}", work.id, step.id), &mut ids);
                out.nodes.push(WorkflowNode {
                    id: gate_id.clone(),
                    position: CanvasPoint { x: work.position.x + 40.0, y: work.position.y + 90.0 * (i as f64 + 1.0) },
                    kind: WorkflowNodeKind::Gate,
                    task: NewTask {
                        title: format!("gate: {}", step.step),
                        instructions: step.command.clone().unwrap_or_default(),
                        scope: Some(self.scope.clone()),
                        ..Default::default()
                    },
                    gate: Some(GateSpec {
                        step: step.id.clone(),
                        command: step.command.clone(),
                        timeout_seconds: step.timeout_seconds,
                        subject: Some(work.id.clone()),
                        required_by: step.required_by.clone(),
                        locked: true,
                    }),
                });
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

    fn ancestors(&self, id: &str) -> BTreeSet<String> {
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
            gate: None,
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
            task: NewTask { title: format!("gate {step}"), ..Default::default() },
            gate: Some(GateSpec { step: step.into(), command: Some("true".into()), ..Default::default() }),
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
    fn review_steps_are_not_injected_in_v1() {
        let def = definition(vec![node("a")], vec![]);
        let (out, notes) = def.inject(&plan("default", &[("review", None, None)]));
        assert!(notes.is_empty());
        assert_eq!(out.nodes.len(), 1);
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
}
