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
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

/// One authored config declaration, before any catalogue is resolved.
/// The legacy configuration path re-exports this exact type.
///
/// A `frameworks` entry is a name or, at the instance root only (#332),
/// `{name, scopes?, except?}`. Either way the name lands in `frameworks`, in
/// the order written, and only a real restriction lands in `applicability`,
/// so a plain string -- or an object with neither key -- still means the
/// whole tree. A nested scope's restriction is refused by the configuration
/// owner, and `into_layer` drops it besides: a child can never exclude.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawDeclaration", into = "RawDeclaration")]
pub struct PolicyDeclaration {
    pub frameworks: Vec<String>,
    /// Where a root-bound framework applies, by framework name. A framework
    /// absent here applies to the whole tree.
    pub applicability: BTreeMap<String, Applicability>,
    pub tighten: BTreeMap<policy::ControlRef, policy::Tighten>,
    pub not_applicable: Vec<policy::NotApplicable>,
}
impl PolicyDeclaration {
    pub fn is_empty(&self) -> bool {
        self.frameworks.is_empty()
            && self.applicability.is_empty()
            && self.tighten.is_empty()
            && self.not_applicable.is_empty()
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

/// The subtrees one root-bound framework applies to (#332): each scope named
/// in `scopes` and everything below it -- the whole tree when `scopes` is
/// absent -- minus each `except` subtree, applied after `scopes`. Names are
/// configured scope names, never a path or a leaf alias. A name no scope
/// carries counts as absent and is a finding (`applicability_findings`),
/// never a load failure: a renamed scope must not stop the daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Applicability {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scopes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub except: Vec<String>,
}
impl Applicability {
    fn is_whole_tree(&self) -> bool {
        self.scopes.is_none() && self.except.is_empty()
    }

    /// The rule that leaves its framework out at `target` (`None`: the root
    /// itself), or `None` when the framework applies there.
    fn excludes<S: ScopeIdentity>(&self, scopes: &[S], target: Option<&S>) -> Option<ExclusionRule> {
        let known = |names: &[String]| -> Vec<String> {
            names.iter().filter(|name| named(scopes, name).is_some()).cloned().collect()
        };
        let covers = |name: &String| named(scopes, name).is_some_and(|top| within(scopes, target, top));
        if let Some(limited) = &self.scopes {
            if !limited.iter().any(covers) {
                return Some(ExclusionRule::Scopes(known(limited)));
            }
        }
        self.except.iter().find(|name| covers(name)).cloned().map(ExclusionRule::Except)
    }
}

/// Why a framework the root binds does not apply at one scope (#332). Only
/// the root's own layer may restrict where a framework applies, so `layer`
/// is always the root's name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exclusion {
    pub framework: String,
    pub layer: String,
    pub rule: ExclusionRule,
}
/// Which half of a root restriction left a scope out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionRule {
    /// Outside every `scopes` subtree -- the known names it was limited to.
    Scopes(Vec<String>),
    /// Inside this `except` subtree.
    Except(String),
}
impl std::fmt::Display for Exclusion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (framework, layer) = (&self.framework, &self.layer);
        match &self.rule {
            ExclusionRule::Except(top) => write!(
                f,
                "the instance root ({layer:?}) excludes {framework} from the {top:?} subtree (except: [{top}])"
            ),
            ExclusionRule::Scopes(limited) => write!(
                f,
                "the instance root ({layer:?}) applies {framework} only to {} (scopes: [{}])",
                if limited.is_empty() {
                    "no configured scope".to_owned()
                } else {
                    format!("the {} subtree(s)", limited.join(", "))
                },
                limited.join(", ")
            ),
        }
    }
}

/// The configured scope a root applicability name means: by its configured
/// name only. Names are unique across the tree (discovery refuses a
/// duplicate), so this never has to choose.
fn named<'a, S: ScopeIdentity>(scopes: &'a [S], name: &str) -> Option<&'a S> {
    scopes.iter().find(|scope| scope.scope_name() == name)
}

/// `target` is `top` or below it -- `scope_subtree`'s own membership rule,
/// by whole path components, never a name prefix. `None` is the root
/// itself, which only a scope configured at the root's own path covers.
fn within<S: ScopeIdentity>(scopes: &[S], target: Option<&S>, top: &S) -> bool {
    match target {
        Some(target) => {
            target.scope_name() == top.scope_name()
                || scope_ancestors(scopes, target)
                    .iter()
                    .any(|ancestor| ancestor.scope_path() == top.scope_path())
        }
        None => top
            .scope_path()
            .components()
            .all(|component| matches!(component, Component::CurDir)),
    }
}

/// Every framework the root binds that its own restriction leaves out at
/// `target` (`None`: the root itself), with the rule that did -- the one
/// resolver the chain, the board's coverage and every refusal share.
pub fn exclusions<S: ScopeIdentity>(
    root: &PolicyDeclaration,
    root_name: &str,
    scopes: &[S],
    target: Option<&S>,
) -> Vec<Exclusion> {
    root.applicability
        .iter()
        .filter_map(|(framework, applicability)| {
            applicability.excludes(scopes, target).map(|rule| Exclusion {
                framework: framework.clone(),
                layer: root_name.to_owned(),
                rule,
            })
        })
        .collect()
}

/// Each root `scopes`/`except` name no configured scope carries, as a
/// finding on the root's own layer. The framework applies as if the name
/// were absent -- which, for a `scopes` list of nothing but unknown names,
/// is nowhere, and the board says so rather than guessing a whole tree.
pub fn applicability_findings<S: ScopeIdentity>(
    root: &PolicyDeclaration,
    root_name: &str,
    scopes: &[S],
) -> Vec<policy::Finding> {
    let mut findings = Vec::new();
    for (framework, applicability) in &root.applicability {
        let written = applicability
            .scopes
            .iter()
            .flatten()
            .map(|name| ("scopes", name))
            .chain(applicability.except.iter().map(|name| ("except", name)));
        for (key, name) in written {
            if named(scopes, name).is_none() {
                findings.push(policy::Finding {
                    kind: policy::FindingKind::UnknownScope,
                    subject: root_name.to_owned(),
                    detail: format!(
                        "{framework}'s `{key}` names {name:?}, which is no configured scope's name; \
                         {framework} applies as if it were absent"
                    ),
                });
            }
        }
    }
    findings
}

/// Plain declarations, not a resolved chain or policy/status cache.
pub trait ScopePolicies: ScopeIdentity {
    fn policy_declaration(&self) -> &PolicyDeclaration;
}

/// The name the root's own layer carries: its configured scope's, or the
/// instance's for a root that never opted into being a scope.
pub fn root_name<'a>(root_name: Option<&'a str>, instance_name: &'a str) -> &'a str {
    root_name.unwrap_or(instance_name)
}

/// The root's own layer as it applies under `excluded`: every excluded
/// framework is gone from it, and so is the root's tightening and `n/a` for
/// one -- unless a layer `below` binds that framework again, which adding
/// always may, and then the root's tightening holds there as it would
/// anywhere the framework applies. Nothing left means no layer at all.
fn narrowed_root(
    root: &PolicyDeclaration,
    root_name: &str,
    excluded: &[Exclusion],
    below: &[policy::PolicyLayer],
) -> Option<policy::PolicyLayer> {
    let gone: BTreeSet<&str> = excluded.iter().map(|e| e.framework.as_str()).collect();
    let rebound = |framework: &str| {
        below
            .iter()
            .any(|layer| layer.frameworks.iter().any(|f| f == framework))
    };
    let mut layer = root.clone().into_layer(root_name);
    layer.frameworks.retain(|f| !gone.contains(f.as_str()));
    layer.tighten.retain(|control, _| {
        !gone.contains(control.framework.as_str()) || rebound(&control.framework)
    });
    layer.not_applicable.retain(|na| {
        !gone.contains(na.control.framework.as_str()) || rebound(&na.control.framework)
    });
    let empty =
        layer.frameworks.is_empty() && layer.tighten.is_empty() && layer.not_applicable.is_empty();
    (!empty).then_some(layer)
}

/// The single policy ancestry algorithm for both configuration compatibility
/// and the physical service. Names never define ancestry; empty layers add
/// nothing and discovery's empty root cannot double the root declaration.
/// The root's layer is narrowed to what applies at `scope` (#332); every
/// layer below is taken whole, since only the root decides applicability.
pub fn chain_for_scope<S: ScopePolicies>(
    root: &PolicyDeclaration,
    root_name: &str,
    scopes: &[S],
    scope: &S,
) -> Vec<policy::PolicyLayer> {
    let below: Vec<_> = scope_ancestors(scopes, scope)
        .into_iter()
        .chain(std::iter::once(scope))
        .filter(|layer| !layer.policy_declaration().is_empty())
        .map(|layer| layer.policy_declaration().clone().into_layer(layer.scope_name()))
        .collect();
    let excluded = exclusions(root, root_name, scopes, Some(scope));
    narrowed_root(root, root_name, &excluded, &below)
        .into_iter()
        .chain(below)
        .collect()
}

/// The chain for a scope that resolves to nothing -- a caller whose scope
/// has since gone: the root's layer alone, narrowed as it applies at the
/// root itself, since a vanished scope has no place in the tree to stand.
pub fn root_chain<S: ScopeIdentity>(
    root: &PolicyDeclaration,
    root_name: &str,
    scopes: &[S],
) -> Vec<policy::PolicyLayer> {
    narrowed_root(root, root_name, &exclusions(root, root_name, scopes, None), &[])
        .into_iter()
        .collect()
}

/// The serialized form: `frameworks` entries as written, a name or
/// `{name, scopes?, except?}`. `deny_unknown_fields`: a typo here --
/// `framework:` for `frameworks:` -- would otherwise commit the company to
/// nothing while parsing clean.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDeclaration {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    frameworks: Vec<FrameworkEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    tighten: BTreeMap<policy::ControlRef, policy::Tighten>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    not_applicable: Vec<policy::NotApplicable>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum FrameworkEntry {
    Name(String),
    Restricted(RestrictedFramework),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestrictedFramework {
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scopes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    except: Vec<String>,
}

/// By hand rather than `untagged`, so a typo inside the object form
/// (`exept:`) is refused by name instead of as "matched no variant".
impl<'de> Deserialize<'de> for FrameworkEntry {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct Entry;
        impl<'de> serde::de::Visitor<'de> for Entry {
            type Value = FrameworkEntry;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a framework name, or {name, scopes, except}")
            }
            fn visit_str<E: serde::de::Error>(self, name: &str) -> std::result::Result<FrameworkEntry, E> {
                Ok(FrameworkEntry::Name(name.to_owned()))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> std::result::Result<FrameworkEntry, A::Error> {
                RestrictedFramework::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(FrameworkEntry::Restricted)
            }
        }
        deserializer.deserialize_any(Entry)
    }
}

impl TryFrom<RawDeclaration> for PolicyDeclaration {
    type Error = String;
    fn try_from(raw: RawDeclaration) -> std::result::Result<Self, String> {
        let mut frameworks: Vec<String> = Vec::new();
        let mut applicability = BTreeMap::new();
        for entry in raw.frameworks {
            let (name, restriction) = match entry {
                FrameworkEntry::Name(name) => (name, None),
                FrameworkEntry::Restricted(entry) => {
                    let restriction = Applicability {
                        scopes: entry.scopes,
                        except: entry.except,
                    };
                    (entry.name, (!restriction.is_whole_tree()).then_some(restriction))
                }
            };
            // Two entries for one framework, either restricted, would leave
            // a person guessing which one decides where it applies.
            let listed = frameworks.contains(&name);
            if listed && (restriction.is_some() || applicability.contains_key(&name)) {
                return Err(format!(
                    "framework {name:?} is listed more than once, with `scopes` or `except` on one of them; \
                     give it a single entry"
                ));
            }
            if let Some(restriction) = restriction {
                applicability.insert(name.clone(), restriction);
            }
            frameworks.push(name);
        }
        Ok(Self {
            frameworks,
            applicability,
            tighten: raw.tighten,
            not_applicable: raw.not_applicable,
        })
    }
}

impl From<PolicyDeclaration> for RawDeclaration {
    fn from(declaration: PolicyDeclaration) -> Self {
        let mut applicability = declaration.applicability;
        Self {
            frameworks: declaration
                .frameworks
                .into_iter()
                .map(|name| match applicability.remove(&name) {
                    Some(restriction) => FrameworkEntry::Restricted(RestrictedFramework {
                        name,
                        scopes: restriction.scopes,
                        except: restriction.except,
                    }),
                    None => FrameworkEntry::Name(name),
                })
                .collect(),
            tighten: declaration.tighten,
            not_applicable: declaration.not_applicable,
        }
    }
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
    pub fn root_name(&self) -> &str {
        root_name(self.root_name.as_deref(), &self.instance_name)
    }
    pub fn chain_for_scope(&self, scope: &Scope) -> Vec<policy::PolicyLayer> {
        chain_for_scope(&self.root_policies, self.root_name(), &self.scopes, scope)
    }
    /// Name/path/leaf resolution keeps the same first-match and vanished-
    /// scope root fallback as the public configuration compatibility path.
    pub fn chain(&self, name: &str) -> Vec<policy::PolicyLayer> {
        match resolve_scope(&self.scopes, name) {
            Ok(scope) => self.chain_for_scope(scope),
            Err(_) => root_chain(&self.root_policies, self.root_name(), &self.scopes),
        }
    }
    /// Every framework the root's own restriction leaves out at `scope`.
    pub fn exclusions(&self, scope: &Scope) -> Vec<Exclusion> {
        exclusions(&self.root_policies, self.root_name(), &self.scopes, Some(scope))
    }
    /// The root's restriction on `framework` with every unknown name left
    /// out -- what the Statement of Applicability prints -- or `None` when
    /// it applies to the whole tree.
    pub fn restriction(&self, framework: &str) -> Option<Applicability> {
        let known = |names: &[String]| -> Vec<String> {
            names
                .iter()
                .filter(|name| named(&self.scopes, name).is_some())
                .cloned()
                .collect()
        };
        self.root_policies
            .applicability
            .get(framework)
            .map(|restriction| Applicability {
                scopes: restriction.scopes.as_deref().map(known),
                except: known(&restriction.except),
            })
    }
    pub fn applicability_findings(&self) -> Vec<policy::Finding> {
        applicability_findings(&self.root_policies, self.root_name(), &self.scopes)
    }
    /// The refusal for `control` not applying at `scope`: it names the
    /// root's exclusion when that is why -- and nothing below binds the
    /// framework back -- rather than leaving a person to guess.
    pub fn not_applying(&self, control: &policy::ControlRef, scope: &Scope) -> FactoryError {
        let bound = policy::frameworks_in_chain(&self.chain_for_scope(scope));
        match self
            .exclusions(scope)
            .into_iter()
            .find(|e| e.framework == control.framework && !bound.contains(&e.framework))
        {
            Some(exclusion) => FactoryError::BadRequest(format!(
                "{control} does not apply at {:?}: {exclusion}",
                scope.name
            )),
            None => FactoryError::BadRequest(format!(
                "{control} does not apply at {:?}, or is not a control any loaded catalogue defines",
                scope.name
            )),
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

    pub async fn catalogues(&self) -> Result<(Vec<policy::Catalogue>, Vec<policy::Finding>)> {
        let directory = policy::policies_dir(&self.root);
        tokio::task::spawn_blocking(move || policy::load_all(&directory))
            .await
            .map_err(|error| FactoryError::Other(anyhow::anyhow!("policy catalogue walk: {error}")))
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
        let (catalogues, _) = self.catalogues().await?;
        let (applied, _) = policy::applicable(&catalogues, &self.config.chain(&scope));
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

    pub async fn catalogues(&self) -> Result<(Vec<policy::Catalogue>, Vec<policy::Finding>)> {
        self.authored().catalogues().await
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
