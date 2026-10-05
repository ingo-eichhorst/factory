//! Live L5 execution-plan compilation. Only authored policy requirements
//! enter from L6; Quality loading/applicability and the sole compiler stay here.
use crate::{
    control_plan::{self, PlanSource},
    quality, quality_inputs,
};
use factory_kernel::{CompiledPlanFact, FactProvider, FactoryError, Provide, Result, L5};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Read {
    pub scope: String,
    pub category: String,
    pub policy: Vec<PlanSource>,
}

pub struct Provider {
    root: PathBuf,
    quality: quality_inputs::Configuration,
}
impl Provider {
    pub fn new(root: PathBuf, quality: quality_inputs::Configuration) -> Self {
        Self { root, quality }
    }
}
impl FactProvider for Provider {
    type Level = L5;
}
#[async_trait::async_trait]
impl Provide<CompiledPlanFact> for Provider {
    type Query = Read;
    type Value = CompiledPlanFact;
    type Error = FactoryError;
    async fn get(&self, read: &Read) -> Result<CompiledPlanFact> {
        factory_kernel::check_category(&read.category).map_err(FactoryError::BadRequest)?;
        let inputs = self
            .quality
            .read(self.root.clone(), Some(&read.scope), true)
            .await?;
        let requirements: Vec<_> = inputs
            .trees
            .iter()
            .flat_map(|(_, tree)| quality::requirements_of(tree))
            .collect();
        Ok(CompiledPlanFact {
            plan: control_plan::resolve(&read.scope, &read.category, &read.policy, &requirements),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        control_plan::{PlanWaiver, Requirement},
        quality::QualityLayer,
    };
    use factory_kernel::{Facts, ScopeNode, L6};

    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let root = Self(
                std::env::temp_dir().join(format!("factory-live-plan-{}", uuid::Uuid::new_v4())),
            );
            std::fs::create_dir_all(quality::quality_dir(&root.0)).unwrap();
            root
        }
        fn profile(&self, seconds: u64) {
            std::fs::write(quality::quality_dir(&self.0).join("base.yaml"), format!("attributes:\n  - id: reliability\n    importance: H\n    difficulty: M\n    requires:\n      - {{applies_to: [feature], step: tests, gate: 'cargo test', timeout_seconds: {seconds}}}\n    scenarios: [{{id: availability}}]\n")).unwrap();
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn config() -> quality_inputs::Configuration {
        quality_inputs::Configuration {
            scopes: vec![quality_inputs::ScopeConfiguration {
                id: "demo-id".into(),
                scope: ScopeNode {
                    name: "demo".into(),
                    path: "projects/demo".into(),
                },
                layers: vec![QualityLayer {
                    scope: "demo".into(),
                    profiles: vec!["base".into()],
                }],
            }],
        }
    }
    fn read() -> Read {
        Read {
            scope: "demo".into(),
            category: "feature".into(),
            policy: vec![PlanSource {
                source: "house/tested".into(),
                requires: vec![Requirement {
                    applies_to: vec!["feature".into()],
                    step: "tests".into(),
                    gate: Some("cargo test".into()),
                    by: None,
                    before: None,
                    after: None,
                    timeout_seconds: Some(90),
                }],
                not_applicable: None,
            }],
        }
    }
    #[tokio::test]
    async fn actual_l5_plan_reads_live_quality_and_compiles_one_tightened_generic_step() {
        let root = Root::new();
        root.profile(30);
        let provider = Provider::new(root.0.clone(), config());
        let facts = Facts::<L6>::new();
        let query = read();
        let first = facts
            .get::<CompiledPlanFact, _>(&provider, &query)
            .await
            .unwrap();
        assert_eq!(first.plan.steps.len(), 1);
        assert_eq!(first.plan.steps[0].timeout_seconds, Some(30));
        assert_eq!(
            first.plan.steps[0].required_by,
            ["house/tested", "quality/reliability"]
        );
        root.profile(10);
        let next = facts
            .get::<CompiledPlanFact, _>(&provider, &query)
            .await
            .unwrap();
        assert_eq!(next.plan.steps[0].timeout_seconds, Some(10));
        let mut waived = read();
        waived.policy[0].not_applicable = Some(PlanWaiver {
            scope: "demo".into(),
            rationale: "authored exemption".into(),
        });
        let plan = facts
            .get::<CompiledPlanFact, _>(&provider, &waived)
            .await
            .unwrap()
            .plan;
        assert_eq!(plan.waived[0].rationale, "authored exemption");
        assert_eq!(plan.steps[0].required_by, ["quality/reliability"]);
        let mut release = read();
        release.category = "release".into();
        assert!(facts
            .get::<CompiledPlanFact, _>(&provider, &release)
            .await
            .unwrap()
            .plan
            .steps
            .is_empty());
    }
    #[tokio::test]
    async fn actual_l5_plan_keeps_validation_and_read_errors_without_cached_success() {
        let root = Root::new();
        root.profile(30);
        let provider = Provider::new(root.0.clone(), config());
        let facts = Facts::<L6>::new();
        let mut bad = read();
        bad.category = "not a category".into();
        bad.scope = "missing".into();
        assert!(facts
            .get::<CompiledPlanFact, _>(&provider, &bad)
            .await
            .unwrap_err()
            .to_string()
            .contains("not a category"));
        bad.category = "feature".into();
        assert_eq!(
            facts
                .get::<CompiledPlanFact, _>(&provider, &bad)
                .await
                .unwrap_err()
                .code(),
            "no_such_scope"
        );
        assert_eq!(
            facts
                .get::<CompiledPlanFact, _>(&provider, &read())
                .await
                .unwrap()
                .plan
                .steps
                .len(),
            1
        );
        std::fs::remove_file(quality::quality_dir(&root.0).join("base.yaml")).unwrap();
        let fresh = facts
            .get::<CompiledPlanFact, _>(&provider, &read())
            .await
            .unwrap();
        assert_eq!(fresh.plan.steps[0].required_by, ["house/tested"]);
    }
}
