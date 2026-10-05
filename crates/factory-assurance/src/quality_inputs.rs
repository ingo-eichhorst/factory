//! Live authored Quality loading and applicability, owned by L5. The
//! composition layer supplies current plain declaration chains, not trees,
//! metric values, judgements or callbacks into its configuration/router.
use crate::{
    metrics::{self, MetricError, MetricId},
    quality::{self, QualityCatalogue, QualityLayer, QualityTree},
};
use factory_kernel::{FactoryError, Result, ScopeIdentity, ScopeNode};
use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone)]
pub struct ScopeConfiguration {
    pub id: String,
    pub scope: ScopeNode,
    pub layers: Vec<QualityLayer>,
}
impl ScopeIdentity for ScopeConfiguration {
    fn scope_name(&self) -> &str {
        &self.scope.name
    }
    fn scope_path(&self) -> &Path {
        &self.scope.path
    }
}
#[derive(Clone, Default)]
pub struct Configuration {
    pub scopes: Vec<ScopeConfiguration>,
}

pub struct Inputs {
    pub catalogue: QualityCatalogue,
    pub fingerprint: u64,
    pub loaded_at: Instant,
    pub asked: Option<ScopeConfiguration>,
    pub trees: Vec<(ScopeConfiguration, QualityTree)>,
    pub findings: Vec<quality::Finding>,
}
impl Inputs {
    pub fn metric_ids(&self) -> Vec<MetricId> {
        metric_ids(self.trees.iter().map(|(_, t)| t))
    }
}
impl Configuration {
    pub async fn read(&self, root: PathBuf, scope: Option<&str>, only: bool) -> Result<Inputs> {
        let loaded_at = Instant::now();
        let dir = quality::quality_dir(&root);
        let catalogue = tokio::task::spawn_blocking(move || quality::load(&dir))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("quality profile walk: {e}")))?;
        let (asked, mut targets) = factory_kernel::scope_subtree(&self.scopes, scope)?;
        if only {
            targets.retain(|s| Some(s.scope.name.as_str()) == asked.map(|a| a.scope.name.as_str()));
        }
        let mut findings = catalogue.findings.clone();
        let mut trees = Vec::new();
        for target in targets {
            if target.layers.is_empty() {
                continue;
            }
            let (tree, chain_findings) =
                quality::applicable(&catalogue, &target.scope.name, &target.layers);
            findings.extend(chain_findings);
            trees.push((target.clone(), tree));
        }
        Ok(Inputs {
            fingerprint: self.fingerprint(&catalogue),
            catalogue,
            loaded_at,
            asked: asked.cloned(),
            trees,
            findings,
        })
    }
    /// Shape only: profiles/findings and every declaration chain. Evidence
    /// and the selected subtree are deliberately excluded, as before.
    pub fn fingerprint(&self, catalogue: &QualityCatalogue) -> u64 {
        let chains: Vec<_> = self
            .scopes
            .iter()
            .map(|s| (s.scope.name.clone(), s.layers.clone()))
            .collect();
        let text = serde_json::to_string(&(&catalogue.profiles, &catalogue.findings, &chains))
            .unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

/// First-seen known metric ids; unavailable entries remain visible. The
/// recursion guard excludes quality.* before any metric is computed.
pub fn metric_ids<'a>(trees: impl IntoIterator<Item = &'a QualityTree>) -> Vec<MetricId> {
    let mut ids = Vec::new();
    for tree in trees {
        for attribute in &tree.attributes {
            for scenario in &attribute.scenarios {
                if let Some(quality::Measure::Metric(measure)) = &scenario.scenario.measure {
                    if !quality::is_quality_metric(&measure.metric)
                        && !ids.contains(&measure.metric)
                        && !matches!(
                            metrics::resolve(&measure.metric),
                            Err(MetricError::Unknown(_))
                        )
                    {
                        ids.push(measure.metric.clone());
                    }
                }
            }
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let root = Self(
                std::env::temp_dir()
                    .join(format!("factory-quality-inputs-{}", uuid::Uuid::new_v4())),
            );
            std::fs::create_dir_all(quality::quality_dir(&root.0)).unwrap();
            root
        }
        fn write(&self, measure: &str) {
            let profile = format!("attributes:\n  - id: reliability.availability\n    importance: H\n    difficulty: M\n    scenarios:\n      - id: up\n        measure: {{ metric: {measure}, above: 0.8 }}\n");
            std::fs::write(quality::quality_dir(&self.0).join("base.yaml"), profile).unwrap();
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn config() -> Configuration {
        Configuration {
            scopes: [
                ("one", "parent", "projects/work"),
                ("two", "child", "projects/work/deep"),
                ("three", "parent/fake", "projects/work-other"),
            ]
            .into_iter()
            .map(|(id, name, path)| ScopeConfiguration {
                id: id.into(),
                scope: ScopeNode {
                    name: name.into(),
                    path: path.into(),
                },
                layers: vec![QualityLayer {
                    scope: "root".into(),
                    profiles: vec!["base".into()],
                }],
            })
            .collect(),
        }
    }
    #[tokio::test]
    async fn authored_profiles_are_reread_subtrees_use_paths_and_fingerprint_is_global_shape() {
        let root = Root::new();
        root.write("fail_rate");
        let config = config();
        let inputs = config
            .read(root.0.clone(), Some("projects/work"), false)
            .await
            .unwrap();
        assert_eq!(inputs.asked.as_ref().unwrap().id, "one");
        assert_eq!(
            inputs
                .trees
                .iter()
                .map(|(scope, _)| scope.id.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert_eq!(
            inputs
                .metric_ids()
                .iter()
                .map(MetricId::as_str)
                .collect::<Vec<_>>(),
            ["fail_rate"]
        );
        assert_eq!(
            inputs.fingerprint,
            config
                .read(root.0.clone(), None, false)
                .await
                .unwrap()
                .fingerprint
        );
        let only = config
            .read(root.0.clone(), Some("parent"), true)
            .await
            .unwrap();
        assert_eq!(only.trees.len(), 1);
        assert_eq!(inputs.fingerprint, only.fingerprint);
        root.write("cost_week");
        let changed = config
            .read(root.0.clone(), Some("parent"), false)
            .await
            .unwrap();
        assert_ne!(changed.fingerprint, inputs.fingerprint);
        assert_eq!(
            changed
                .metric_ids()
                .iter()
                .map(MetricId::as_str)
                .collect::<Vec<_>>(),
            ["cost_week"]
        );
        assert!(changed.loaded_at >= inputs.loaded_at);
    }
    #[tokio::test]
    async fn recursive_and_unknown_quality_metrics_never_enter_dependencies() {
        let root = Root::new();
        for name in ["quality.reliability", "not_a_metric"] {
            root.write(name);
            let inputs = config().read(root.0.clone(), None, false).await.unwrap();
            assert!(inputs.metric_ids().is_empty());
        }
    }
}
