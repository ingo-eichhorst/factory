//! Canonical compatibility path for L4-owned process behavior.
pub use factory_process::workflow::*;

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::control_plan::StepKind;
    use crate::{control_plan, task::NewTask};
    use std::collections::BTreeMap;
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
    fn resolved_lexical_plan_still_injects_deterministic_gates_before_review() {
        let requirement =
            |step: &str, gate: Option<&str>, by: Option<&str>| crate::control_plan::Requirement {
                applies_to: vec!["feature".into()],
                step: step.into(),
                gate: gate.map(str::to_string),
                by: by.map(str::to_string),
                before: None,
                after: None,
                timeout_seconds: None,
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
            resolved
                .steps
                .iter()
                .map(|s| s.step.as_str())
                .collect::<Vec<_>>(),
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
        assert!(out
            .edges
            .iter()
            .any(|edge| edge.from == "a.tests" && edge.to == "a.review"));
    }
}
