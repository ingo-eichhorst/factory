//! Workflow domain rules that never touch the DOM: validation messages that
//! mirror the server's, node/edge construction, folding a server event into
//! client state, and what the toolbar buttons should look like. `workflows.js`
//! decides nothing these functions could decide instead -- it calls in here,
//! then paints exactly what comes back, so every decision here is testable
//! without a browser at all.

import { ancestors, freePosition, nodeTitle, topologicalSummary } from "./workflow-graph.js";

const uid = prefix =>
  `${prefix}-${globalThis.crypto?.randomUUID?.() || Math.random().toString(36).slice(2)}`;

export function blankWorkflow(scope) {
  return { id: null, name: "Untitled workflow", description: "", scope, revision: null, inputs: [], nodes: [], edges: [] };
}

// ------------------------------------------------------------------ inputs

/// An input's name, `[A-Za-z_][A-Za-z0-9_-]*` -- `is_input_name` in
/// `workflow.rs`.
export function isInputName(name) {
  return /^[A-Za-z_][A-Za-z0-9_-]*$/.test(name ?? "");
}

/// Every `{{name}}` in `text`, in order, with spaces inside the braces
/// allowed; braces around anything that is not a name (`{{.Names}}`, `{{}}`)
/// are not placeholders. The same scan as `placeholders` in `workflow.rs`,
/// not a regex, so the edge cases (`{{{a}}`, an unclosed `{{`) agree too.
export function placeholders(text) {
  const out = [];
  let rest = text ?? "";
  for (;;) {
    const open = rest.indexOf("{{");
    if (open < 0) break;
    const after = rest.slice(open + 2);
    const close = after.indexOf("}}");
    if (close < 0) break;
    const name = after.slice(0, close).trim();
    if (isInputName(name)) out.push(name);
    rest = after.slice(close + 2);
  }
  return out;
}

/// The texts of a task node the server writes a run's inputs into: title,
/// instructions and label values.
function nodeTexts(node) {
  return [node.task.title, node.task.instructions, ...Object.values(node.task.labels ?? {})];
}

/// Which inputs a node's text uses, once each, in order -- and which of
/// those the workflow does not declare. The inspector's "uses" line.
export function nodeInputUsage(node, inputs) {
  const declared = new Set((inputs ?? []).map(input => input.name));
  const used = [...new Set(nodeTexts(node).flatMap(placeholders))];
  return { used, undeclared: used.filter(name => !declared.has(name)) };
}

/// The server's `validate_inputs`: names are names and are declared once,
/// and -- only once any input is declared -- every `{{name}}` in a task
/// node names one. A workflow with no inputs leaves braces alone, since
/// they may be meant for some other tool.
export function inputProblems(workflow) {
  const errors = [];
  const declared = new Set();
  for (const input of workflow.inputs ?? []) {
    if (!isInputName(input.name)) {
      errors.push({
        message: `input ${JSON.stringify(input.name)} is not a name: use letters, digits, "_" or "-", starting with a letter or "_"`,
      });
    } else if (declared.has(input.name)) {
      errors.push({ message: `input ${JSON.stringify(input.name)} is declared twice` });
    }
    declared.add(input.name);
  }
  if (!declared.size) return errors;
  for (const node of workflow.nodes) {
    if (node.kind === "gate") continue;
    const unknown = [...new Set(nodeTexts(node).flatMap(placeholders))].filter(name => !declared.has(name));
    for (const name of unknown) {
      errors.push({
        nodeId: node.id,
        message: `${nodeLabel(node)} uses {{${name}}}, but the workflow declares no input "${name}"`,
      });
    }
  }
  return errors;
}

// ------------------------------------------------------------------ rework

/// The server's `validate_rework`, named by title: only a task node sends
/// work back, at least once, to a task node that comes before it. Kept
/// apart from `validate` so deleting a node or a link can show what it
/// broke straight away, without the rest of `validate`'s opinions about a
/// workflow that is still being drawn.
export function reworkProblems(workflow) {
  const errors = [];
  for (const node of workflow.nodes) {
    const rework = node.rework;
    if (!rework) continue;
    const from = nodeLabel(node);
    if (node.kind === "gate") {
      errors.push({ nodeId: node.id, message: `${from} is a gate and cannot send work back; only a task node can` });
      continue;
    }
    if (!(Number.isInteger(rework.max_rounds) && rework.max_rounds >= 1)) {
      errors.push({ nodeId: node.id, message: `${from} sends work back at most zero times; use at least one round` });
    }
    const target = workflow.nodes.find(item => item.id === rework.to);
    if (!target) {
      errors.push({ nodeId: node.id, message: `${from} sends work back to a node that no longer exists (${rework.to})` });
    } else if (target.kind === "gate") {
      errors.push({ nodeId: node.id, message: `${from} sends work back to the gate ${nodeLabel(target)}; name a task node` });
    } else if (!ancestors(workflow.edges, node.id).has(rework.to)) {
      errors.push({
        nodeId: node.id,
        message: `${from} sends work back to ${nodeLabel(target)}, which does not come before it`,
      });
    }
  }
  return errors;
}

/// "sends work back to implement, at most 5×" -- the summary's wording, and
/// the one the acceptance names.
export function reworkSentence(nodes, node) {
  if (!node.rework) return "";
  const rounds = node.rework.max_rounds;
  return `sends work back to ${nodeTitle(nodes, node.rework.to)}, at most ${rounds}×`;
}

/// "↺ implement ×5" -- the same fact, short enough for a card.
export function reworkBadge(nodes, node) {
  if (!node.rework) return "";
  return `↺ ${nodeTitle(nodes, node.rework.to)} ×${node.rework.max_rounds}`;
}

// ------------------------------------------------------------------- saving

/// What Save sends. Everything the canvas does not edit rides along
/// untouched -- the category (#118) -- and what it does edit goes as it
/// stands: the declared inputs and each node's `rework` (#140), so saving a
/// layout never drops what a file or the CLI put there.
export function saveDraft(workflow) {
  return {
    name: workflow.name, description: workflow.description, scope: workflow.scope,
    category: workflow.category ?? null, inputs: workflow.inputs ?? [],
    nodes: workflow.nodes, edges: workflow.edges,
  };
}

/// A fresh task node with an id nothing else has and Factory's ordinary
/// task defaults -- own worktree, no agent override (the scope's default
/// applies), nothing scheduled (workflow nodes are started by dependencies).
export function newTaskNode(scope, x = 80, y = 80) {
  return {
    id: uid("node"),
    position: { x, y },
    kind: "task",
    task: {
      title: "New task",
      instructions: "",
      scope,
      agent: null,
      worktree: true,
      estimate_seconds: null,
      ack_timeout_seconds: null,
      timeout_seconds: null,
      blocked_timeout_seconds: null,
    },
  };
}

/// A copy of `sourceId` with a new id and a position that does not overlap
/// any existing card (U11) -- `null` if the source node no longer exists.
/// Its `rework` stays behind: the copy has no incoming links, so nothing
/// comes before it for work to go back to.
export function duplicateNode(nodes, sourceId) {
  const source = nodes.find(node => node.id === sourceId);
  if (!source) return null;
  const at = freePosition(nodes, source.position.x + 40, source.position.y + 40);
  return {
    id: uid("node"),
    position: at,
    kind: source.kind,
    task: { ...structuredClone(source.task), title: `${source.task.title} copy` },
  };
}

export function addEdge(edges, id, fromId, toId) {
  return [...edges, { id, from: fromId, to: toId }];
}

/// Client-side validation with the same rules `WorkflowDefinition::validate`
/// checks server-side (U12), so a save is rarely refused by a round trip
/// when it could have been refused right here. Every message is returned
/// with the offending node's id, when there is one, so the caller can
/// highlight it; cross-scope and schedule-on-a-node are not re-checked here
/// since the inspector never lets either happen from the UI in the first
/// place.
export function validate(workflow) {
  const errors = [];
  if (!workflow.name || !workflow.name.trim()) {
    errors.push({ message: "a workflow needs a name" });
  }
  if (!workflow.nodes.length) {
    errors.push({ message: "a workflow needs at least one task node" });
  }

  const seen = new Set();
  for (const node of workflow.nodes) {
    if (seen.has(node.id)) {
      errors.push({ nodeId: node.id, message: `duplicate node id ${JSON.stringify(node.id)}` });
    }
    seen.add(node.id);
    if (!node.task.title || !node.task.title.trim()) {
      // R8: `nodeLabel` itself is not useful here -- the one fact it would
      // report is the very thing missing. The node is already highlighted
      // on the canvas (`renderProblems` keys off `nodeId`), so the message
      // only needs to say what is wrong, not repeat which node by a label
      // it does not have.
      errors.push({ nodeId: node.id, message: "A task node has no title (highlighted on the canvas)." });
    }
    for (const [value, label] of [
      [node.task.estimate_seconds, "estimate"],
      [node.task.ack_timeout_seconds, "acknowledgement timeout"],
      [node.task.timeout_seconds, "run timeout"],
      [node.task.blocked_timeout_seconds, "blocked timeout"],
    ]) {
      if (value !== null && value !== undefined && value !== "" && Number(value) <= 0) {
        errors.push({
          nodeId: node.id,
          message: `${nodeLabel(node)} has a non-positive ${label}; use at least one second`,
        });
      }
    }
  }

  for (const edge of workflow.edges) {
    const hasFrom = workflow.nodes.some(node => node.id === edge.from);
    const hasTo = workflow.nodes.some(node => node.id === edge.to);
    if (!hasFrom || !hasTo) {
      errors.push({ message: `edge ${JSON.stringify(edge.id)} references a node that no longer exists` });
    }
  }

  const summary = topologicalSummary(workflow);
  if (summary.error) errors.push({ message: summary.error });

  errors.push(...inputProblems(workflow), ...reworkProblems(workflow));
  return errors;
}

function nodeLabel(node) {
  const title = node.task.title && node.task.title.trim();
  return title ? `"${title}"` : "a task node";
}

/// What Save/Run/Cancel should look like from state alone (U12: "explicit
/// disabled/loading/error states"). `hasWorkflow` is "there is something
/// open in the editor" -- true even for a brand new, never-saved workflow,
/// since Save has to stay enabled for exactly that; `hasId` is "it has been
/// saved at least once", which Run additionally requires.
export function editorButtons({ hasWorkflow, hasId, dirty, saving, run, mode }) {
  const running = run?.status === "running";
  return {
    save: { disabled: !hasWorkflow || saving, loading: saving },
    run: {
      disabled: !hasWorkflow || !hasId || dirty || saving || running || mode === "run",
      loading: false,
      hint: dirty ? "Save first -- Run always uses the saved revision" : "",
    },
    cancel: { disabled: !running, loading: false },
  };
}

/// A node's "open task" action, or `null` when it has never spawned one
/// (U9) -- a thin, testable seam between a node run and the injected
/// `openTask` the real page calls with the task's id.
export function openTaskAction(nodeRun, openTask) {
  if (!nodeRun?.task_id) return null;
  return () => openTask(nodeRun.task_id);
}

// ------------------------------------------------------------------ events

/// Fold one server event into `{ workflows, current, currentRun, dirty }`.
/// Never mutates its input -- `workflows.js` assigns the result onto its own
/// state, so this can be tested with plain objects and no DOM. Returns the
/// state unchanged for an event about a different workflow, and never lets
/// an in-flight edit (`dirty`) be clobbered by a `workflow_updated` for the
/// workflow currently open -- callers see a `notice` instead (U3).
export function applyWorkflowEvent(state, event) {
  const { workflows, current } = state;
  switch (event.type) {
    case "workflow_created": {
      const definition = event.workflow;
      return { ...state, workflows: upsert(workflows, definition), notice: state.notice };
    }
    case "workflow_updated": {
      const definition = event.workflow;
      const next = upsert(workflows, definition);
      if (current?.id === definition.id && state.dirty) {
        return { ...state, workflows: next, notice: `saved elsewhere as revision ${definition.revision}` };
      }
      const nextCurrent = current?.id === definition.id ? structuredClone(definition) : current;
      return { ...state, workflows: next, current: nextCurrent };
    }
    case "workflow_deleted": {
      const next = workflows.filter(item => item.id !== event.id);
      if (current?.id === event.id) {
        return { ...state, workflows: next, current: null, currentRun: null };
      }
      return { ...state, workflows: next };
    }
    case "workflow_run_updated": {
      if (current?.id !== event.run.workflow_id) return state;
      return { ...state, currentRun: event.run };
    }
    default:
      return state;
  }
}

function upsert(list, definition) {
  const index = list.findIndex(item => item.id === definition.id);
  return index < 0 ? [definition, ...list] : list.map((item, i) => (i === index ? definition : item));
}

// ----------------------------------------------------------------- routing

/// `#<scope>/proc/workflows/<id>`, and with a run selected too,
/// `#<scope>/proc/workflows/<id>/run/<run-id>` -- an explicit `run` marker
/// rather than a bare second segment, since `app.js`'s router already
/// claims a bare `task` segment for the task modal that can open over any
/// view, and a workflow id must never be mistaken for one.
export function workflowRouteTail(id, runId) {
  if (!id) return [];
  return runId ? [id, "run", runId] : [id];
}

/// The inverse: `[id]` or `[id, "run", runId]` back to `{id, runId}`. A tail
/// naming no run (an old link, or one written before runs were selectable)
/// reads as `runId: null`, which is "show the latest" to whoever reads it.
export function readWorkflowRouteTail(tail) {
  const [id, marker, runId] = tail;
  if (!id) return { id: null, runId: null };
  return { id, runId: marker === "run" && runId ? runId : null };
}
