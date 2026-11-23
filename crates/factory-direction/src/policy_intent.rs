//! Live L6 authored policy inputs. This owner reads its own templates and
//! every scope's own policies (#390), receipts and monthly limits, then
//! projects raw declarations to L5. It never supplies a compliance report,
//! spend or precomputed verdict.
use crate::{budget, policy, policy_store::PolicyStore};
use factory_assurance::{
    checks::CheckSource,
    evidence::{needs_budget_facts, BudgetIntent},
    metrics_service::{PolicyInputs, PolicyScope, QualityBudgets},
};
use factory_kernel::{
    scope_ancestors, scope_subtree, FactoryError, Facts, KnowledgeTags, Provide, Result,
    ScopeIdentity, ScopeNode, L6,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

/// A `policies:` block -- the instance root's top-level one, or a nested
/// scope's `scope.policies` -- in the shape Factory no longer reads (#390):
/// `frameworks` with `scopes`/`except`, `tighten` and `not_applicable`. A
/// policy now belongs to one scope, in that scope's own
/// `.factory/policies/`. The block is kept verbatim, whatever its shape, so
/// the daemon still starts and a roster rewrite never deletes it, and every
/// read reports it as a [`policy::FindingKind::LegacyPolicies`] finding
/// naming the migration -- the same rule as an unknown scope name: a config
/// mistake is a finding, never a daemon that refuses to start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PolicyDeclaration(pub Option<serde_yaml_ng::Value>);
impl PolicyDeclaration {
    /// No block at all (or an empty `policies:`).
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

/// The finding a legacy `policies:` block raises at `subject` (a scope's
/// name), naming where its frameworks now belong.
pub fn legacy_finding(subject: &str, block: &str, policies_dir: &str) -> policy::Finding {
    policy::Finding {
        kind: policy::FindingKind::LegacyPolicies,
        subject: subject.to_owned(),
        detail: format!(
            "has a `{block}` block, which Factory no longer reads: a policy now belongs to one scope (#390). \
             Move each framework this scope commits to into {policies_dir}<framework>.yaml -- picking \
             controls from a template in .factory/policies/templates/ and giving each one this scope's own \
             evidence -- and delete the block. See the README's \"Policies\" section"
        ),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Only ever reported: see [`PolicyDeclaration`].
    pub policies: PolicyDeclaration,
}
impl ScopeIdentity for Scope {
    fn scope_name(&self) -> &str {
        &self.name
    }
    fn scope_path(&self) -> &Path {
        &self.path
    }
}

/// The name the root carries in a finding: its configured scope's, or the
/// instance's for a root that never opted into being a scope.
pub fn root_name<'a>(root_name: Option<&'a str>, instance_name: &'a str) -> &'a str {
    root_name.unwrap_or(instance_name)
}

#[derive(Debug, Clone)]
pub struct Configuration {
    pub scopes: Vec<Scope>,
    /// The root's top-level `policies:`, only ever reported.
    pub root_policies: PolicyDeclaration,
    pub root_name: Option<String>,
    pub instance_name: String,
}
impl Configuration {
    pub fn root_name(&self) -> &str {
        root_name(self.root_name.as_deref(), &self.instance_name)
    }

    /// One finding per legacy `policies:` block still in the config.
    pub fn legacy_findings(&self) -> Vec<policy::Finding> {
        let dir = |path: &Path| {
            let label = path
                .components()
                .filter(|c| !matches!(c, Component::CurDir))
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            if label.is_empty() {
                ".factory/policies/".to_owned()
            } else {
                format!("{label}/.factory/policies/")
            }
        };
        let mut findings = Vec::new();
        if !self.root_policies.is_empty() {
            findings.push(legacy_finding(
                self.root_name(),
                "policies:",
                ".factory/policies/",
            ));
        }
        for scope in self.scopes.iter().filter(|s| !s.policies.is_empty()) {
            findings.push(legacy_finding(
                &scope.name,
                "scope.policies",
                &dir(&scope.path),
            ));
        }
        findings
    }

    /// Every template and every scope's own policies, read fresh off disk,
    /// with the legacy-block findings folded in. Blocking: call it off the
    /// executor.
    pub fn load_policies(&self, root: &Path) -> policy::Policies {
        let scopes: Vec<(String, PathBuf)> = self
            .scopes
            .iter()
            .map(|scope| (scope.name.clone(), scope.path.clone()))
            .collect();
        let mut policies = policy::load(root, &scopes);
        policies.findings.extend(self.legacy_findings());
        policies.findings.sort_by(|a, b| {
            a.subject
                .cmp(&b.subject)
                .then(a.kind.cmp(&b.kind))
                .then(a.detail.cmp(&b.detail))
        });
        policies
    }

    /// The refusal for `control` when `scope` does not own it (#390): it
    /// says whether the scope has no policy for the framework -- and which
    /// scopes below it do own the control -- or whether its own policy left
    /// the control unselected, rather than leaving a person to guess.
    pub fn not_owned(
        &self,
        policies: &policy::Policies,
        control: &policy::ControlRef,
        scope: &Scope,
    ) -> FactoryError {
        let own = policies
            .of(&scope.name)
            .find(|p| p.framework == control.framework);
        let detail = match own {
            Some(policy) => match policy.not_selected.iter().find(|c| &c.control == control) {
                Some(_) => format!(
                    "{control} is not selected at {:?}: its {} policy ({}) does not pick it from template {:?}",
                    scope.name,
                    policy.framework,
                    policy.file,
                    policy.template.as_deref().unwrap_or_default()
                ),
                None => format!(
                    "{control} is not a control of {:?}'s {} policy ({})",
                    scope.name, policy.framework, policy.file
                ),
            },
            None => {
                let owners: Vec<&str> = match scope_subtree(&self.scopes, Some(&scope.name)) {
                    Ok((_, targets)) => targets
                        .into_iter()
                        .filter(|target| {
                            policies
                                .of(&target.name)
                                .any(|p| p.controls.iter().any(|c| &c.control == control))
                        })
                        .map(|target| target.name.as_str())
                        .collect(),
                    Err(_) => Vec::new(),
                };
                if owners.is_empty() {
                    format!(
                        "{:?} has no {} policy of its own, so {control} does not apply there",
                        scope.name, control.framework
                    )
                } else {
                    format!(
                        "{:?} has no {} policy of its own; {control} belongs to {} -- name the scope that owns it",
                        scope.name,
                        control.framework,
                        owners.join(", ")
                    )
                }
            }
        };
        FactoryError::BadRequest(detail)
    }

    pub fn budget_intent(&self, scope: &Scope, config: &budget::PolicyConfig) -> BudgetIntent {
        budget::check_intent(
            config,
            scope_ancestors(&self.scopes, scope)
                .into_iter()
                .chain(std::iter::once(scope))
                .map(|s| (s.id.clone(), s.name.clone())),
        )
    }
}

/// Authored budget intent as a pure read: the budget catalogue under the root plus the scope configuration. It holds no
/// receipt store or other L6 state, so any level may be handed one (S9b: `Intent`); `Service` reads the same data through it.
pub struct Authored {
    pub(crate) root: PathBuf,
    pub(crate) config: Configuration,
}
impl Authored {
    pub fn new(root: PathBuf, config: Configuration) -> Self {
        Self { root, config }
    }

    /// Preserve invalid authored limits as raw configuration error data.
    /// Gathering/assessment of spend belongs to L5, never this method.
    pub async fn budget_config(&self) -> Result<budget::PolicyConfig> {
        let root = self.root.clone();
        let loaded = tokio::task::spawn_blocking(move || budget::load(&root))
            .await
            .map_err(|error| FactoryError::Other(anyhow::anyhow!("budget intent read: {error}")))?;
        Ok(match loaded {
            Ok(catalogue) => budget::PolicyConfig {
                catalogue: Some(catalogue),
                error: None,
            },
            Err(error) => budget::PolicyConfig {
                catalogue: None,
                error: Some(error),
            },
        })
    }

    /// Every template and scope policy, read fresh -- see
    /// [`Configuration::load_policies`].
    pub async fn policies(&self) -> Result<policy::Policies> {
        let root = self.root.clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || config.load_policies(&root))
            .await
            .map_err(|error| FactoryError::Other(anyhow::anyhow!("policy directory walk: {error}")))
    }

    /// Select current authored requirements in L6, then read the actual L5
    /// plan compiler. Preserve category/scope/catalogue/Quality failure order.
    pub async fn control_plan<P>(
        &self,
        scope: &str,
        category: &str,
        provider: &P,
    ) -> Result<factory_kernel::ControlPlan>
    where
        P: Provide<factory_kernel::CompiledPlanFact,
            Query = factory_assurance::plan_service::Read,
            Value = factory_kernel::CompiledPlanFact, Error = FactoryError>,
    {
        let read = self.plan_input(scope, category).await?;
        Ok(Facts::<L6>::new()
            .get::<factory_kernel::CompiledPlanFact, _>(provider, &read).await?.plan)
    }

    /// Own authored sources only; L5 still loads Quality and compiles live.
    pub async fn plan_input(&self, scope: &str, category: &str) -> Result<factory_assurance::plan_service::Read> {
        factory_kernel::check_category(category).map_err(FactoryError::BadRequest)?;
        let scope = factory_kernel::resolve_scope(&self.config.scopes, scope)?.name.clone();
        // The run's own scope's policies only: nothing is inherited (#390).
        let applied = self.policies().await?.applied(&scope);
        Ok(factory_assurance::plan_service::Read {
            scope,
            category: category.to_owned(),
            policy: policy::plan_sources(&applied),
        })
    }

    pub async fn budget_for<S: CheckSource>(
        &self,
        applied: &[&[S]],
    ) -> Result<Option<budget::PolicyConfig>> {
        if !applied.iter().any(|subjects| needs_budget_facts(subjects)) {
            return Ok(None);
        }
        self.budget_config().await.map(Some)
    }

    pub async fn quality_budgets(&self, needed: &[&str]) -> QualityBudgets {
        if needed.is_empty() {
            return Ok(BTreeMap::new());
        }
        let config = self
            .budget_config()
            .await
            .map_err(|error| error.to_string())?;
        Ok(self
            .config
            .scopes
            .iter()
            .filter(|scope| needed.contains(&scope.id.as_str()))
            .map(|scope| (scope.id.clone(), self.config.budget_intent(scope, &config)))
            .collect())
    }
}

pub struct Service<'a> {
    pub(crate) root: PathBuf,
    pub(crate) config: Configuration,
    pub(crate) receipts: &'a PolicyStore,
}
impl<'a> Service<'a> {
    pub fn new(root: PathBuf, config: Configuration, receipts: &'a PolicyStore) -> Self {
        Self {
            root,
            config,
            receipts,
        }
    }

    fn authored(&self) -> Authored {
        Authored::new(self.root.clone(), self.config.clone())
    }

    pub async fn policies(&self) -> Result<policy::Policies> {
        self.authored().policies().await
    }

    /// Select current authored requirements in L6, then read the actual L5
    /// plan compiler. Preserve category/scope/catalogue/Quality failure order.
    pub async fn control_plan<P>(
        &self,
        scope: &str,
        category: &str,
        provider: &P,
    ) -> Result<factory_kernel::ControlPlan>
    where
        P: Provide<factory_kernel::CompiledPlanFact,
            Query = factory_assurance::plan_service::Read,
            Value = factory_kernel::CompiledPlanFact, Error = FactoryError>,
    {
        self.authored().control_plan(scope, category, provider).await
    }

    pub async fn plan_input(&self, scope: &str, category: &str) -> Result<factory_assurance::plan_service::Read> {
        self.authored().plan_input(scope, category).await
    }

    /// Same-level authored read followed by the actual L5 fact capability.
    /// A failed fact read remains a failure; nothing is cached or published.
    pub async fn policies_with_tags<P>(
        &self,
        provider: &P,
    ) -> Result<(policy::Policies, BTreeSet<String>)>
    where
        P: Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>,
    {
        let policies = self.policies().await?;
        let tags = Facts::<L6>::new()
            .get::<KnowledgeTags, _>(provider, &())
            .await?
            .tags;
        Ok((policies, tags))
    }

    /// Preserve invalid authored limits as raw configuration error data.
    /// Gathering/assessment of spend belongs to L5, never this method.
    pub async fn budget_config(&self) -> Result<budget::PolicyConfig> {
        Authored::new(self.root.clone(), self.config.clone()).budget_config().await
    }

    pub async fn budget_for<S: CheckSource>(
        &self,
        applied: &[&[S]],
    ) -> Result<Option<budget::PolicyConfig>> {
        self.authored().budget_for(applied).await
    }

    /// Fresh raw policy subjects, receipts and optional caps for the L5
    /// metric service. Whole-instance store receipt order is unchanged;
    /// L5's evidence service filters applicability at the live read.
    pub async fn metric_inputs(&self, scope: Option<&str>) -> Result<PolicyInputs> {
        let policies = self.policies().await?;
        let (_, targets) = scope_subtree(&self.config.scopes, scope)?;
        let attestations = self.receipts.all().await?;
        let mut applied = Vec::new();
        // Every scope in the subtree with a policy of its own (#390): the
        // metric is the aggregate over every (scope, control) pair.
        for target in targets {
            let subjects = policies.applied(&target.name);
            if !subjects.is_empty() {
                applied.push((target, subjects));
            }
        }
        let config = self
            .budget_for(
                &applied
                    .iter()
                    .map(|(_, subjects)| subjects.as_slice())
                    .collect::<Vec<_>>(),
            )
            .await?;
        let scopes = applied
            .into_iter()
            .map(|(target, subjects)| PolicyScope {
                scope: ScopeNode {
                    name: target.name.clone(),
                    path: target.path.clone(),
                },
                subjects: subjects.iter().map(policy::metric_subject).collect(),
                budget: config
                    .as_ref()
                    .map(|config| self.config.budget_intent(target, config)),
            })
            .collect();
        Ok(PolicyInputs {
            scopes,
            attestations,
        })
    }

    /// L5 asks only for ids its own Quality plan needs. Selected raw limits
    /// remain independently inherited, in the configured order. No metric
    /// result, Quality judgement or spend ever enters/leaves this method.
    pub async fn quality_budgets(&self, needed: &[&str]) -> QualityBudgets {
        Authored::new(self.root.clone(), self.config.clone()).quality_budgets(needed).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use factory_kernel::{Attestation, FactProvider, Withdrawal, L5};
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    };

    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("factory-policy-intent-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(policy::policies_dir(&root)).unwrap();
            std::fs::create_dir_all(budget::budgets_dir(&root)).unwrap();
            Self(root)
        }
        /// `cra.yaml` in the `child` scope's own policies directory.
        fn catalogue(&self, raw: &str) {
            self.policy("projects/work/demo", "cra", raw);
        }
        fn policy(&self, scope_path: &str, framework: &str, raw: &str) {
            let dir = policy::scope_policies_dir(&self.0, Path::new(scope_path));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{framework}.yaml")), raw).unwrap();
        }
        fn template(&self, name: &str, raw: &str) {
            let dir = policy::templates_dir(&self.0);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("{name}.yaml")), raw).unwrap();
        }
        fn limits(&self, raw: &str) {
            std::fs::write(budget::catalogue_path(&self.0), raw).unwrap();
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn declaration(raw: &str) -> PolicyDeclaration {
        serde_yaml_ng::from_str(raw).unwrap()
    }
    fn configuration() -> Configuration {
        Configuration {
            scopes: [
                ("root-id", "company", "."),
                ("parent-id", "work", "projects/work"),
                ("child-id", "child", "projects/work/demo"),
                ("side-id", "work/side", "projects/work-other"),
            ]
            .into_iter()
            .map(|(id, name, path)| Scope {
                id: id.into(),
                name: name.into(),
                path: path.into(),
                policies: PolicyDeclaration::default(),
            })
            .collect(),
            root_policies: PolicyDeclaration::default(),
            root_name: Some("company".into()),
            instance_name: "instance".into(),
        }
    }
    const CATALOGUE: &str = "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - id: a\n    title: A\n    max_age: 7d\n    evidence: [{check: attestation}]\n";
    fn receipt(id: &str, scope: &str) -> Attestation {
        let now = Utc::now();
        Attestation {
            id: id.into(),
            control: "cra/a".parse().unwrap(),
            scope: scope.into(),
            evidence: "recorded evidence".into(),
            note: None,
            attested_by: "owner".into(),
            attested_at: now,
            expires_at: now + Duration::days(3),
            withdrawn: None,
            clock: None,
            corrective: None,
        }
    }

    // -- legacy blocks (#390) --------------------------------------------

    #[test]
    fn a_legacy_block_of_any_shape_parses_round_trips_and_is_a_finding_naming_the_migration() {
        for raw in [
            "frameworks: [{name: cra, except: [finance]}, dsgvo]\ntighten: {cra/a: {max_age: 7d}}\n",
            // Refused at load before #390; now only reported.
            "frameworks: [{name: cra, exept: [finance]}, cra, {name: cra, scopes: [x]}]\n",
            "frameworks: [7]\n",
            "not even a mapping",
        ] {
            let parsed = declaration(raw);
            assert!(!parsed.is_empty(), "{raw}");
            let yaml = serde_yaml_ng::to_string(&parsed).unwrap();
            assert_eq!(declaration(&yaml), parsed, "a rewrite keeps the block verbatim");
            let json = serde_json::to_value(&parsed).unwrap();
            assert_eq!(serde_json::from_value::<PolicyDeclaration>(json).unwrap(), parsed);
        }
        assert!(declaration("~").is_empty());
        assert_eq!(
            serde_json::to_value(PolicyDeclaration::default()).unwrap(),
            serde_json::Value::Null
        );

        let mut config = configuration();
        config.root_policies = declaration("frameworks: [cra]");
        config.scopes[2].policies = declaration("tighten: {cra/a: {max_age: 7d}}");
        let findings = config.legacy_findings();
        assert_eq!(
            findings
                .iter()
                .map(|f| (f.kind, f.subject.as_str()))
                .collect::<Vec<_>>(),
            [
                (policy::FindingKind::LegacyPolicies, "company"),
                (policy::FindingKind::LegacyPolicies, "child"),
            ]
        );
        assert!(findings[0].detail.contains("`policies:`"), "{findings:?}");
        assert!(
            findings[0]
                .detail
                .contains(".factory/policies/<framework>.yaml"),
            "{findings:?}"
        );
        assert!(
            findings[1]
                .detail
                .contains("projects/work/demo/.factory/policies/<framework>.yaml"),
            "{findings:?}"
        );
        let mut unnamed = config.clone();
        unnamed.root_name = None;
        assert_eq!(unnamed.legacy_findings()[0].subject, "instance");
    }

    // -- ownership (#390) ------------------------------------------------

    #[test]
    fn a_policy_applies_to_the_scope_that_owns_it_only() {
        let root = Root::new();
        std::fs::write(policy::policies_dir(&root.0).join("house.yaml"), "framework: house\ntitle: House\nkind: best-practice\ncontrols: [{id: h, title: H, evidence: [{check: attestation}]}]\n").unwrap();
        root.policy("projects/work", "cra", CATALOGUE);
        // A lookalike path beside `work` is not below it.
        root.policy(
            "projects/work-other",
            "cra",
            &CATALOGUE.replace("title: A", "title: Side"),
        );
        // `templates/` and `drafts/` are never policies.
        root.template(
            "cra",
            "template: cra\ntitle: CRA\nkind: regulation\ncontrols: [{id: a, title: A}]\n",
        );
        std::fs::create_dir_all(policy::policies_dir(&root.0).join("drafts")).unwrap();
        std::fs::write(
            policy::policies_dir(&root.0).join("drafts/next.yaml"),
            "framework: next\ntitle: Next\nkind: regulation\n",
        )
        .unwrap();
        let config = configuration();
        let policies = config.load_policies(&root.0);
        assert!(policies.findings.is_empty(), "{:?}", policies.findings);
        assert_eq!(policies.frameworks("company"), ["house"]);
        assert_eq!(policies.frameworks("work"), ["cra"]);
        assert!(
            policies.frameworks("child").is_empty(),
            "nothing is inherited down the tree"
        );
        assert_eq!(policies.applied("work/side")[0].title, "Side");
        assert_eq!(policies.templates.len(), 1);
        assert_eq!(
            policies.of("work").next().unwrap().file,
            "projects/work/.factory/policies/cra.yaml"
        );
        assert_eq!(
            policies.of("company").next().unwrap().file,
            ".factory/policies/house.yaml"
        );
        assert_eq!(
            policy::policy_frameworks(&root.0, Path::new("projects/work")),
            ["cra"]
        );

        let cra_a: policy::ControlRef = "cra/a".parse().unwrap();
        let refused = config
            .not_owned(&policies, &cra_a, &config.scopes[0])
            .to_string();
        assert!(
            refused.contains("\"company\" has no cra policy of its own"),
            "{refused}"
        );
        assert!(refused.contains("belongs to work, work/side"), "{refused}");
        let below = config
            .not_owned(&policies, &cra_a, &config.scopes[2])
            .to_string();
        assert!(below.contains("does not apply there"), "{below}");
        let unknown = config
            .not_owned(&policies, &"cra/zz".parse().unwrap(), &config.scopes[1])
            .to_string();
        assert!(
            unknown.contains("is not a control of \"work\"'s cra policy"),
            "{unknown}"
        );
    }

    #[tokio::test]
    async fn a_control_plan_reads_the_runs_own_scope_and_never_an_ancestors_requires() {
        let root = Root::new();
        root.policy(
            "projects/work",
            "house",
            "framework: house\ntitle: House\nkind: best-practice\ncontrols:\n  - id: tested\n    title: Tested\n    evidence: [{check: attestation}]\n    requires:\n      - {applies_to: [feature], step: tests, gate: cargo test}\n",
        );
        let authored = Authored::new(root.0.clone(), configuration());
        let parent = authored.plan_input("work", "feature").await.unwrap();
        assert_eq!(parent.policy.len(), 1);
        assert_eq!(parent.policy[0].source, "house/tested");
        let child = authored.plan_input("child", "feature").await.unwrap();
        assert!(
            child.policy.is_empty(),
            "an ancestor's policy injects nothing into a child's plan"
        );
    }

    #[test]
    fn a_root_policy_with_no_root_scope_is_owned_by_nobody_and_said_so() {
        let root = Root::new();
        std::fs::write(policy::policies_dir(&root.0).join("cra.yaml"), CATALOGUE).unwrap();
        let mut config = configuration();
        config.scopes.remove(0);
        let policies = config.load_policies(&root.0);
        assert!(policies.owned.is_empty());
        assert_eq!(policies.findings.len(), 1, "{:?}", policies.findings);
        assert_eq!(
            policies.findings[0].kind,
            policy::FindingKind::NoOwningScope
        );
        assert_eq!(policies.findings[0].subject, ".factory/policies/cra.yaml");
    }

    #[test]
    fn an_unselected_template_control_is_named_in_the_refusal() {
        let root = Root::new();
        root.template("cra", "template: cra\ntitle: CRA\nkind: regulation\ncontrols: [{id: a, title: A}, {id: b, title: B}]\n");
        root.policy("projects/work", "cra", "framework: cra\ntemplate: cra\ncontrols: [{id: a, evidence: [{check: attestation}]}]\n");
        let config = configuration();
        let policies = config.load_policies(&root.0);
        assert!(policies.findings.is_empty(), "{:?}", policies.findings);
        let refused = config
            .not_owned(&policies, &"cra/b".parse().unwrap(), &config.scopes[1])
            .to_string();
        assert!(
            refused.contains("cra/b is not selected at \"work\""),
            "{refused}"
        );
        assert!(refused.contains("template \"cra\""), "{refused}");
    }

    #[tokio::test]
    async fn metric_inputs_reread_catalogues_receipts_and_withdrawals_without_a_verdict() {
        let root = Root::new();
        root.catalogue(CATALOGUE);
        root.policy("projects/work/demo", "practices", "framework: practices\ntitle: Practices\nkind: best-practice\ncontrols: [{id: b, title: B}]\n");
        root.policy(
            ".",
            "house",
            "framework: house\ntitle: House\nkind: regulation\ncontrols: [{id: h, title: H}]\n",
        );
        let store = PolicyStore::in_memory().unwrap();
        let owner = Service::new(root.0.clone(), configuration(), &store);
        let first = owner.metric_inputs(Some("demo")).await.unwrap();
        assert_eq!(first.scopes.len(), 1);
        assert_eq!(first.scopes[0].scope.name, "child");
        assert_eq!(
            first.scopes[0].subjects[0].max_age,
            Some("7d".parse().unwrap())
        );
        assert!(first.scopes[0].subjects[0].kind);
        assert!(
            !first.scopes[0].subjects[1].kind,
            "only L6 declares which classifications count"
        );
        assert!(first.scopes[0].budget.is_none());
        assert!(first.attestations.is_empty());
        let parent = receipt("parent", "work");
        let sibling = receipt("side", "work/side");
        store.append_attestation(&parent).await.unwrap();
        store.append_attestation(&sibling).await.unwrap();
        let withdrawal = Withdrawal {
            at: Utc::now(),
            by: "owner".into(),
            reason: Some("superseded".into()),
        };
        store
            .append_withdrawal(&parent.id, &parent.control, &parent.scope, &withdrawal)
            .await
            .unwrap();
        root.catalogue(&CATALOGUE.replace("title: A", "title: Updated"));
        let next = owner.metric_inputs(Some("child")).await.unwrap();
        assert_eq!(next.scopes[0].subjects[0].title, "Updated");
        // Filtering receipts for the current scope is L5's evidence work,
        // so even a sibling's raw receipt remains in this input as before.
        assert_eq!(next.attestations.len(), 2);
        assert_eq!(
            next.attestations
                .iter()
                .find(|a| a.id == "parent")
                .unwrap()
                .withdrawn
                .as_ref(),
            Some(&withdrawal)
        );
        assert_eq!(
            next.attestations.iter().find(|a| a.id == "side").unwrap(),
            &sibling
        );
        let whole = owner.metric_inputs(None).await.unwrap();
        assert_eq!(
            whole
                .scopes
                .iter()
                .map(|s| s.scope.name.as_str())
                .collect::<Vec<_>>(),
            ["company", "child"],
            "only the scopes that own a policy, each with its own"
        );
        assert_eq!(
            whole.scopes[0].subjects.len(),
            1,
            "the root's own policy only"
        );
    }

    #[tokio::test]
    async fn authored_budgets_need_no_store_and_carry_a_bad_file_as_data_for_only_the_scopes_asked() {
        let root = Root::new();
        root.limits("invalid: configuration\n");
        let authored = Authored::new(root.0.clone(), configuration());
        assert!(authored.quality_budgets(&[]).await.unwrap().is_empty(), "nothing asked, nothing read");
        let asked = authored.quality_budgets(&["child-id"]).await.unwrap();
        assert_eq!(asked.keys().map(String::as_str).collect::<Vec<_>>(), ["child-id"], "only the scope asked for");
        assert!(authored.budget_config().await.unwrap().error.is_some());
    }

    #[tokio::test]
    async fn budget_intent_is_lazy_live_independent_and_never_a_spend_assessment() {
        let root = Root::new();
        root.catalogue(CATALOGUE);
        root.limits("invalid: configuration\n");
        let store = PolicyStore::in_memory().unwrap();
        let owner = Service::new(root.0.clone(), configuration(), &store);
        assert!(owner
            .metric_inputs(None)
            .await
            .unwrap()
            .scopes
            .iter()
            .all(|s| s.budget.is_none()));
        assert!(owner.quality_budgets(&[]).await.unwrap().is_empty());
        root.catalogue(&CATALOGUE.replace("{check: attestation}", "{check: budget_within}"));
        let invalid = owner.metric_inputs(Some("child")).await.unwrap();
        let intent = invalid.scopes[0].budget.as_ref().unwrap();
        assert!(intent.error.as_ref().unwrap().contains("limits.yaml"));
        assert!(intent.caps.is_empty());
        let quality_error = owner.quality_budgets(&["child-id"]).await.unwrap();
        assert!(quality_error["child-id"]
            .error
            .as_ref()
            .unwrap()
            .contains("limits.yaml"));
        assert!(quality_error["child-id"].caps.is_empty());
        root.limits("version: 1\nscopes:\n  parent-id: {monthly_usd: 50}\n  child-id: {monthly_usd: 20}\n  side-id: {monthly_usd: 900}\n");
        let valid = owner.metric_inputs(Some("child")).await.unwrap();
        assert_eq!(
            valid.scopes[0].budget.as_ref().unwrap().caps,
            [("work".into(), 50.0), ("child".into(), 20.0)]
        );
        let quality = owner
            .quality_budgets(&["child-id", "absent-id"])
            .await
            .unwrap();
        assert_eq!(quality.len(), 1);
        assert_eq!(
            quality["child-id"].caps,
            [("work".into(), 50.0), ("child".into(), 20.0)]
        );
        root.limits("version: 1\nscopes: {child-id: {monthly_usd: 12}}\n");
        assert_eq!(
            owner.quality_budgets(&["child-id"]).await.unwrap()["child-id"].caps,
            [("child".into(), 12.0)]
        );
        assert!(
            owner.quality_budgets(&["child-id"]).await.unwrap()["child-id"]
                .error
                .is_none()
        );
    }

    #[tokio::test]
    async fn invalid_scopes_precede_receipt_failures_and_an_actual_failed_store_read_retries() {
        let root = Root::new();
        root.catalogue(CATALOGUE);
        let path = root.0.join("receipts.sqlite");
        let store = PolicyStore::open(&path).unwrap();
        let owner = Service::new(root.0.clone(), configuration(), &store);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "ALTER TABLE policy_attestations RENAME TO qa_hidden_receipts",
            [],
        )
        .unwrap();
        let unknown = match owner.metric_inputs(Some("missing")).await {
            Err(e) => e,
            Ok(_) => panic!("unknown scope accepted"),
        };
        assert_eq!(unknown.code(), "no_such_scope");
        let failure = match owner.metric_inputs(None).await {
            Err(e) => e,
            Ok(_) => panic!("missing store table ignored"),
        };
        assert!(failure.to_string().contains("policy_attestations"));
        conn.execute(
            "ALTER TABLE qa_hidden_receipts RENAME TO policy_attestations",
            [],
        )
        .unwrap();
        assert!(owner.metric_inputs(None).await.is_ok());
    }

    #[derive(Default)]
    struct Tags {
        calls: AtomicUsize,
        fail: AtomicBool,
        tags: Mutex<BTreeSet<String>>,
    }
    impl FactProvider for Tags {
        type Level = L5;
    }
    #[async_trait::async_trait]
    impl Provide<KnowledgeTags> for Tags {
        type Query = ();
        type Value = KnowledgeTags;
        type Error = FactoryError;
        async fn get(&self, _: &()) -> Result<KnowledgeTags> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(FactoryError::adapter("knowledge", "unavailable"));
            }
            Ok(KnowledgeTags {
                tags: self.tags.lock().unwrap().clone(),
            })
        }
    }
    #[tokio::test]
    async fn knowledge_uses_a_real_typed_read_and_catalogue_findings_and_failure_retry_stay_live() {
        let root = Root::new();
        root.catalogue(CATALOGUE);
        let store = PolicyStore::in_memory().unwrap();
        let owner = Service::new(root.0.clone(), configuration(), &store);
        let tags = Tags::default();
        tags.tags.lock().unwrap().insert("control/cra/a".into());
        let (policies, first) = owner.policies_with_tags(&tags).await.unwrap();
        assert_eq!(policies.applied("child")[0].title, "A");
        assert!(policies.findings.is_empty());
        assert!(first.contains("control/cra/a"));
        root.catalogue(&CATALOGUE.replace("title: A", "title: Updated"));
        std::fs::write(
            policy::policies_dir(&root.0).join("broken.yaml"),
            "framework: [bad]\n",
        )
        .unwrap();
        tags.tags.lock().unwrap().clear();
        let (policies, next) = owner.policies_with_tags(&tags).await.unwrap();
        assert_eq!(policies.applied("child")[0].title, "Updated");
        assert!(!policies.findings.is_empty());
        assert!(next.is_empty());
        tags.fail.store(true, Ordering::SeqCst);
        assert!(owner
            .policies_with_tags(&tags)
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable"));
        tags.fail.store(false, Ordering::SeqCst);
        assert!(owner.policies_with_tags(&tags).await.is_ok());
        assert_eq!(tags.calls.load(Ordering::SeqCst), 4);
    }
}
