//! Live L6 authored policy inputs. This owner reads its own catalogues,
//! receipts and monthly limits, then projects raw declarations to L5.
//! It never supplies a compliance report, spend or precomputed verdict.
use crate::{budget, policy, policy_store::PolicyStore};
use factory_assurance::{
    checks::CheckSource,
    evidence::{needs_budget_facts, BudgetIntent},
    metrics_service::{PolicyInputs, PolicyScope, QualityBudgets},
};
use factory_kernel::{
    resolve_scope, scope_ancestors, scope_subtree, FactoryError, Facts, KnowledgeTags, Provide,
    Result, ScopeIdentity, ScopeNode, L6,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// One authored config declaration, before any catalogue is resolved.
/// The legacy configuration path re-exports this exact type.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PolicyDeclaration {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frameworks: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tighten: BTreeMap<policy::ControlRef, policy::Tighten>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_applicable: Vec<policy::NotApplicable>,
}
impl PolicyDeclaration {
    pub fn is_empty(&self) -> bool {
        self.frameworks.is_empty() && self.tighten.is_empty() && self.not_applicable.is_empty()
    }
    pub fn into_layer(self, scope: impl Into<String>) -> policy::PolicyLayer {
        policy::PolicyLayer {
            scope: scope.into(),
            frameworks: self.frameworks,
            tighten: self.tighten,
            not_applicable: self.not_applicable,
        }
    }
}

/// Plain declarations, not a resolved chain or policy/status cache.
pub trait ScopePolicies: ScopeIdentity {
    fn policy_declaration(&self) -> &PolicyDeclaration;
}

pub fn root_layer(
    declaration: &PolicyDeclaration,
    root_name: Option<&str>,
    instance_name: &str,
) -> Option<policy::PolicyLayer> {
    (!declaration.is_empty()).then(|| {
        declaration
            .clone()
            .into_layer(root_name.unwrap_or(instance_name))
    })
}

/// The single policy ancestry algorithm for both configuration compatibility
/// and the physical service. Names never define ancestry; empty layers add
/// nothing and discovery's empty root cannot double the root declaration.
pub fn chain_for_scope<S: ScopePolicies>(
    root: Option<policy::PolicyLayer>,
    scopes: &[S],
    scope: &S,
) -> Vec<policy::PolicyLayer> {
    let mut chain: Vec<_> = root.into_iter().collect();
    for layer in scope_ancestors(scopes, scope)
        .into_iter()
        .chain(std::iter::once(scope))
    {
        let declaration = layer.policy_declaration();
        if !declaration.is_empty() {
            chain.push(declaration.clone().into_layer(layer.scope_name()));
        }
    }
    chain
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
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
impl ScopePolicies for Scope {
    fn policy_declaration(&self) -> &PolicyDeclaration {
        &self.policies
    }
}
#[derive(Debug, Clone)]
pub struct Configuration {
    pub scopes: Vec<Scope>,
    pub root_policies: PolicyDeclaration,
    pub root_name: Option<String>,
    pub instance_name: String,
}
impl Configuration {
    fn root_layer(&self) -> Option<policy::PolicyLayer> {
        root_layer(
            &self.root_policies,
            self.root_name.as_deref(),
            &self.instance_name,
        )
    }
    pub fn chain_for_scope(&self, scope: &Scope) -> Vec<policy::PolicyLayer> {
        chain_for_scope(self.root_layer(), &self.scopes, scope)
    }
    /// Name/path/leaf resolution keeps the same first-match and vanished-
    /// scope root fallback as the public configuration compatibility path.
    pub fn chain(&self, name: &str) -> Vec<policy::PolicyLayer> {
        match resolve_scope(&self.scopes, name) {
            Ok(scope) => self.chain_for_scope(scope),
            Err(_) => self.root_layer().into_iter().collect(),
        }
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

pub struct Service<'a> {
    root: PathBuf,
    config: Configuration,
    receipts: &'a PolicyStore,
}
impl<'a> Service<'a> {
    pub fn new(root: PathBuf, config: Configuration, receipts: &'a PolicyStore) -> Self {
        Self {
            root,
            config,
            receipts,
        }
    }

    pub async fn catalogues(&self) -> Result<(Vec<policy::Catalogue>, Vec<policy::Finding>)> {
        let directory = policy::policies_dir(&self.root);
        tokio::task::spawn_blocking(move || policy::load_all(&directory))
            .await
            .map_err(|error| FactoryError::Other(anyhow::anyhow!("policy catalogue walk: {error}")))
    }

    /// Same-level authored read followed by the actual L5 fact capability.
    /// A failed fact read remains a failure; nothing is cached or published.
    pub async fn catalogues_with_tags<P>(
        &self,
        provider: &P,
    ) -> Result<(
        Vec<policy::Catalogue>,
        Vec<policy::Finding>,
        BTreeSet<String>,
    )>
    where
        P: Provide<KnowledgeTags, Query = (), Value = KnowledgeTags, Error = FactoryError>,
    {
        let (catalogues, findings) = self.catalogues().await?;
        let tags = Facts::<L6>::new()
            .get::<KnowledgeTags, _>(provider, &())
            .await?
            .tags;
        Ok((catalogues, findings, tags))
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

    pub async fn budget_for<S: CheckSource>(
        &self,
        applied: &[&[S]],
    ) -> Result<Option<budget::PolicyConfig>> {
        if !applied.iter().any(|subjects| needs_budget_facts(subjects)) {
            return Ok(None);
        }
        self.budget_config().await.map(Some)
    }

    /// Fresh raw policy subjects, receipts and optional caps for the L5
    /// metric service. Whole-instance store receipt order is unchanged;
    /// L5's evidence service filters applicability at the live read.
    pub async fn metric_inputs(&self, scope: Option<&str>) -> Result<PolicyInputs> {
        let (catalogues, _) = self.catalogues().await?;
        let (_, targets) = scope_subtree(&self.config.scopes, scope)?;
        let attestations = self.receipts.all().await?;
        let mut applied = Vec::new();
        for target in targets {
            let (subjects, _) = policy::applicable(&catalogues, &self.config.chain(&target.name));
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
        fn catalogue(&self, raw: &str) {
            std::fs::write(policy::policies_dir(&self.0).join("cra.yaml"), raw).unwrap();
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
                ("root-id", "company", ".", "{}"),
                (
                    "parent-id",
                    "work",
                    "projects/work",
                    "frameworks: [practices]",
                ),
                (
                    "child-id",
                    "child",
                    "projects/work/demo",
                    "tighten: {cra/a: {max_age: 7d}}",
                ),
                ("side-id", "work/side", "projects/work-other", "{}"),
            ]
            .into_iter()
            .map(|(id, name, path, raw)| Scope {
                id: id.into(),
                name: name.into(),
                path: path.into(),
                policies: declaration(raw),
            })
            .collect(),
            root_policies: declaration("frameworks: [cra]"),
            root_name: Some("company".into()),
            instance_name: "instance".into(),
        }
    }
    const CATALOGUE: &str = "framework: cra\ntitle: CRA\nkind: regulation\ncontrols:\n  - id: a\n    title: A\n    max_age: 30d\n    evidence: [{check: attestation}]\n";
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

    #[test]
    fn declaration_is_plain_strict_serde_and_the_chain_uses_paths_once() {
        let config = configuration();
        let chain = config.chain("projects/work/demo");
        assert_eq!(
            chain.iter().map(|l| l.scope.as_str()).collect::<Vec<_>>(),
            ["company", "work", "child"]
        );
        assert_eq!(
            chain[2].tighten[&"cra/a".parse().unwrap()].max_age,
            Some("7d".parse().unwrap())
        );
        assert_eq!(config.chain("work/side").len(), 1);
        assert_eq!(config.chain("vanished").len(), 1);
        let mut duplicate = configuration();
        let mut second = duplicate.scopes[1].clone();
        second.path = "elsewhere/work".into();
        second.policies = declaration("frameworks: [other]");
        duplicate.scopes.push(second);
        assert_eq!(
            duplicate.chain("work")[1].frameworks,
            ["practices"],
            "configured-name lookup keeps the first declaration, even in a duplicated fixture"
        );
        let mut ambiguous = configuration();
        let mut second = ambiguous.scopes[2].clone();
        second.name = "other-demo".into();
        second.path = "elsewhere/demo".into();
        ambiguous.scopes.push(second);
        assert_eq!(
            ambiguous.chain("demo").len(),
            1,
            "ambiguous legacy leaf keeps the compatibility root fallback"
        );
        let mut unnamed = configuration();
        unnamed.root_name = None;
        assert_eq!(unnamed.chain("child")[0].scope, "instance");
        let original = declaration("frameworks: [cra]\ntighten: {cra/a: {max_age: 7d}}\nnot_applicable: [{control: cra/b, rationale: unused}]\n");
        let json = serde_json::to_value(&original).unwrap();
        assert_eq!(
            serde_json::from_value::<PolicyDeclaration>(json.clone()).unwrap(),
            original
        );
        assert_eq!(json["tighten"]["cra/a"]["max_age"], "1w");
        assert!(serde_yaml_ng::from_str::<PolicyDeclaration>("framework: [cra]").is_err());
        assert_eq!(
            serde_json::to_value(PolicyDeclaration::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[tokio::test]
    async fn metric_inputs_reread_catalogues_receipts_and_withdrawals_without_a_verdict() {
        let root = Root::new();
        root.catalogue(CATALOGUE);
        std::fs::write(policy::policies_dir(&root.0).join("practices.yaml"), "framework: practices\ntitle: Practices\nkind: best-practice\ncontrols: [{id: b, title: B}]\n").unwrap();
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
            ["company", "work", "child", "work/side"]
        );
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
        let (catalogues, findings, first) = owner.catalogues_with_tags(&tags).await.unwrap();
        assert_eq!(catalogues[0].controls[0].title, "A");
        assert!(findings.is_empty());
        assert!(first.contains("control/cra/a"));
        root.catalogue(&CATALOGUE.replace("title: A", "title: Updated"));
        std::fs::write(
            policy::policies_dir(&root.0).join("broken.yaml"),
            "framework: [bad]\n",
        )
        .unwrap();
        tags.tags.lock().unwrap().clear();
        let (catalogues, findings, next) = owner.catalogues_with_tags(&tags).await.unwrap();
        assert_eq!(catalogues[0].controls[0].title, "Updated");
        assert!(!findings.is_empty());
        assert!(next.is_empty());
        tags.fail.store(true, Ordering::SeqCst);
        assert!(owner
            .catalogues_with_tags(&tags)
            .await
            .unwrap_err()
            .to_string()
            .contains("unavailable"));
        tags.fail.store(false, Ordering::SeqCst);
        assert!(owner.catalogues_with_tags(&tags).await.is_ok());
        assert_eq!(tags.calls.load(Ordering::SeqCst), 4);
    }
}
