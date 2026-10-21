//! `#351`: an agent renamed from the roster, as Process sees it.
//!
//! A task names its agent, and that name resolves in the task's own scope to a
//! declaration first and to a bare adapter second. A declaration renamed
//! without its waiting work would leave those tasks failing to dispatch or,
//! where the old name is also an adapter's (`pi`, `codex`), silently running
//! on the bare adapter without the declaration's arguments, role or sandbox.
//! So the tasks still waiting for it move with it; finished ones, runs, usage
//! and the journal keep the name they had -- history is not rewritten. What
//! is authored by a person, a workflow naming the agent, is listed, never
//! rewritten. The rules are here, pure; the daemon carries them out.
use crate::task::Task;
use crate::workflow::WorkflowNode;
use factory_agents::agent::RenameSessions;
use serde::{Deserialize, Serialize};

/// What a rename did (`#351`): the answer to `factory agent rename`, `POST
/// /api/agents/rename` and the roster's Rename.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenameReport {
    /// The scope's canonical name.
    pub scope: String,
    pub from: String,
    /// The new name, trimmed.
    pub to: String,
    /// False when the agent was already called `to`: nothing was written.
    pub changed: bool,
    /// Tasks that were waiting for the agent, now waiting for it under its
    /// new name, by id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<String>,
    /// What became of its standing sessions.
    #[serde(default)]
    pub sessions: RenameSessions,
    /// Workflow files under the scope's directory that still name the old
    /// agent, relative to it. Not rewritten: they are a person's to edit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    /// Stored workflow definitions, and workflow runs still going, that still
    /// name the old agent. Not rewritten either.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workflows: Vec<String>,
    /// Anything else the rename changed that a person would not guess.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl RenameReport {
    /// `"pi in factory is now scratch; 2 waiting tasks moved"`, then what
    /// still names the old one.
    pub fn summary(&self) -> String {
        if !self.changed {
            return format!("{} in {} is already called that; nothing changed", self.from, self.scope);
        }
        let mut out = format!("{} in {} is now {}", self.from, self.scope, self.to);
        match self.tasks.len() {
            0 => {}
            1 => out.push_str("; 1 waiting task moved"),
            n => out.push_str(&format!("; {n} waiting tasks moved")),
        }
        if !self.sessions.started.is_empty() {
            out.push_str(&format!("; restarting {}", self.sessions.started.join(", ")));
        }
        let named: Vec<&str> = self.files.iter().chain(&self.workflows).map(String::as_str).collect();
        if !named.is_empty() {
            out.push_str(&format!("; still naming {}: {}", self.from, named.join(", ")));
        }
        for note in &self.notes {
            out.push_str(&format!("; {note}"));
        }
        out
    }
}

/// Whether `task` is waiting for the agent `name` in the scope `is_here`
/// accepts: not done and not cancelled, in that scope, naming it. A name
/// resolves in its task's own scope, declared first, so in the renamed
/// agent's scope every task naming it reaches the declaration -- the bare
/// adapter of the same name included. `is_here` canonicalizes, so a task
/// still carrying its scope's pre-path bare name counts.
pub fn waits_for(task: &Task, name: &str, is_here: impl Fn(&str) -> bool) -> bool {
    !task.status.is_terminal() && task.agent == name && is_here(&task.scope)
}

/// The ids of the nodes whose task names the agent `name` in the scope
/// `is_here` accepts: a node's own `scope:`, or its workflow's.
pub fn nodes_naming(
    nodes: &[WorkflowNode],
    workflow_scope: &str,
    name: &str,
    is_here: impl Fn(&str) -> bool,
) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.task.agent.as_deref() == Some(name))
        .filter(|node| is_here(node.task.scope.as_deref().unwrap_or(workflow_scope)))
        .map(|node| node.id.clone())
        .collect()
}

/// The same, for a workflow file as a person wrote it: `None` for a file
/// that is not a workflow at all.
pub fn file_nodes_naming(yaml: &str, name: &str, is_here: impl Fn(&str) -> bool) -> Option<Vec<String>> {
    let draft: crate::workflow::WorkflowDraft = serde_yaml_ng::from_str(yaml).ok()?;
    Some(nodes_naming(&draft.nodes, &draft.scope, name, is_here))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::TaskStatus;

    fn task(scope: &str, agent: &str, status: TaskStatus) -> Task {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t", "title": "t", "instructions": "", "scope": scope, "agent": agent,
            "runtime": "herdr", "status": "pending", "created_at": "2026-10-21T00:00:00Z",
            "updated_at": "2026-10-21T00:00:00Z",
        }))
        .unwrap();
        task.status = status;
        task
    }

    #[test]
    fn only_unfinished_tasks_naming_it_in_its_own_scope_wait_for_it() {
        let here = |scope: &str| scope == "projects/factory" || scope == "factory";
        for status in [TaskStatus::Pending, TaskStatus::Blocked, TaskStatus::Intake, TaskStatus::Failed] {
            assert!(waits_for(&task("projects/factory", "pi", status), "pi", here), "{status:?}");
        }
        assert!(waits_for(&task("factory", "pi", TaskStatus::Pending), "pi", here), "a legacy bare scope name");
        assert!(!waits_for(&task("projects/factory", "pi", TaskStatus::Done), "pi", here), "history keeps its name");
        assert!(!waits_for(&task("projects/factory", "pi", TaskStatus::Cancelled), "pi", here));
        assert!(!waits_for(&task("projects/other", "pi", TaskStatus::Pending), "pi", here), "another scope's pi");
        assert!(!waits_for(&task("projects/factory", "codex", TaskStatus::Pending), "pi", here));
    }

    #[test]
    fn a_workflow_names_the_agent_where_a_node_asks_for_it_in_this_scope() {
        let yaml = "name: github-issue\nscope: factory\nnodes:\n  - id: triage\n    task: { title: Triage, agent: builder }\n  - id: elsewhere\n    task: { title: X, agent: builder, scope: other }\n  - id: check\n    task: { title: Check, agent: shell }\n  - id: default\n    task: { title: Default }\nedges: []\n";
        let here = |scope: &str| scope == "factory";
        assert_eq!(file_nodes_naming(yaml, "builder", here), Some(vec!["triage".to_string()]));
        assert_eq!(file_nodes_naming(yaml, "reviewer", here), Some(Vec::new()));
        assert_eq!(file_nodes_naming("just: some yaml\n", "builder", here), None, "not a workflow");
        let in_other = |scope: &str| scope == "other";
        assert_eq!(file_nodes_naming(yaml, "builder", in_other), Some(vec!["elsewhere".to_string()]));
    }

    #[test]
    fn the_summary_says_what_moved_and_what_still_names_the_old_agent() {
        let report = RenameReport {
            scope: "factory".into(),
            from: "builder".into(),
            to: "implementer".into(),
            changed: true,
            tasks: vec!["a".into(), "b".into()],
            sessions: RenameSessions { moved: Vec::new(), started: vec!["factory/implementer".into()] },
            files: vec!["workflows/github-issue.yaml".into()],
            workflows: vec!["workflow github-issue".into()],
            notes: Vec::new(),
        };
        assert_eq!(
            report.summary(),
            "builder in factory is now implementer; 2 waiting tasks moved; restarting factory/implementer; \
             still naming builder: workflows/github-issue.yaml, workflow github-issue"
        );
        let same = RenameReport { changed: false, ..report };
        assert_eq!(same.summary(), "builder in factory is already called that; nothing changed");
    }
}
