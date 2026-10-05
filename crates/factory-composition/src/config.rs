use crate::dashboard::DashboardConfig;
use factory_agents::agent::Lifetime;
use factory_agents::role::{RoleSpec, Roles};
#[cfg(test)]
use factory_agents::role::{Role, RoleOrigin};
use factory_assurance::quality::QualityLayer;
use factory_direction::policy::PolicyLayer;
#[cfg(test)]
use factory_direction::policy::ControlRef;
use factory_kernel::{FactoryError, Result};
use factory_process::ready::IntakeLayer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The directory that marks a Factory instance or a configured scope. Runtime
/// state belongs to the instance root's directory; nested copies contain only
/// the configuration for the scope whose directory they sit in.
pub const FACTORY_DIR: &str = ".factory";
pub const CONFIG_FILE: &str = "config.yaml";
pub const PLUGINS_DIR: &str = "plugins";

pub use factory_environment::declarations::{
    DependenciesConfig, DependencyDirection, DependencyEffect, DependencyService,
    DependencyTransport,
};

/// What one config file -- the instance root's top-level `policies:`, or a
/// scope's own `scope.policies` -- declares about which frameworks apply.
/// The same shape as `policy::PolicyLayer` minus `scope`: that field names
/// which layer in a resolved chain a declaration came from, and is filled in
/// only when a declaration becomes a layer (`into_layer`, the one place this
/// conversion happens -- see `Config::policy_chain_for_scope`).
///
/// Unlike `RoleSpec`, this is additive only: a scope may name more
/// frameworks and tighten or mark a control `n/a`, never drop or loosen what
/// an ancestor already committed the company to. That rule lives in
/// `policy::applicable`, which folds a chain of these; nothing here enforces
/// it on its own.
///
/// `deny_unknown_fields`: a typo here -- `framework:` for `frameworks:` --
/// would otherwise commit the company to nothing while parsing clean. For a
/// regulatory declaration that is worth refusing loudly rather than quietly
/// applying zero frameworks.
pub use factory_direction::policy_intent::PolicyDeclaration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    pub instance: Instance,
    #[serde(default)]
    pub daemon: DaemonConfig,
    /// The scope rooted in the same directory as this config file. On the
    /// instance root this sits alongside the instance-wide settings above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    /// Scopes loaded from their own `.factory/config.yaml` files at startup.
    /// This runtime list is never written back into the instance config. The
    /// old central list remains parse-compatible, but discovery replaces it.
    #[serde(default, skip_serializing)]
    pub scopes: Vec<Scope>,
    /// Roles this instance names for itself, on top of `worker` and `foreman`.
    /// Keyed by the name an agent is given in its `role:`. They hold in every
    /// scope; a nested scope adds its own under `scope.roles`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, RoleSpec>,
    /// This instance's own dashboard layout (`#159`), the top of every chain
    /// `dashboard_for_scope` walks -- written here exactly where `roles`
    /// above is, and refused on `self.scope` the same way
    /// (`refuse_root_scope_dashboard`). `None` means the instance names no
    /// layout of its own, which resolves to the built-in default unless a
    /// scope below overrides it; `Some` replaces that default with at least
    /// one tile -- an empty `tiles` is refused at validation, not a
    /// deliberate "show nothing" -- see `DashboardConfig`'s own doc comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<DashboardConfig>,
    /// Which frameworks this instance commits to, plus any tightening or
    /// `n/a` declared at the root -- the top of every chain
    /// `policy_chain_for_scope` builds, mirroring `roles` above except
    /// additive-only. A nested scope adds its own under `scope.policies`;
    /// the root refuses that block the same way it refuses `scope.roles`
    /// (`refuse_root_scope_policies`).
    #[serde(default, skip_serializing_if = "PolicyDeclaration::is_empty")]
    pub policies: PolicyDeclaration,
    /// Which quality profiles (`.factory/quality/<profile>.yaml`) hold
    /// everywhere -- the top of every chain `quality_chain_for_scope`
    /// builds, exactly as `policies` above is for policy layers. Just a list
    /// of profile ids: a profile is itself the utility tree, so there is
    /// nothing else for a declaration to say. A nested scope adds its own
    /// under `scope.quality`; the root refuses that block
    /// (`refuse_root_scope_quality`), the same as `scope.policies`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quality: Vec<String>,
    /// What everything runs on that Factory does not run itself: today, the
    /// AI accounts behind the agents. Only the instance root declares these
    /// -- a nested scope's file refuses the block
    /// (`refuse_misplaced_scope_infrastructure`) -- and nothing here is ever
    /// discovered by reading a credential. See `Infrastructure`.
    #[serde(default, skip_serializing_if = "Infrastructure::is_empty")]
    pub infrastructure: Infrastructure,
    /// The declared secrets catalogue (`#244`): where each secret lives and
    /// what it is for, never its value. Only the instance root declares it
    /// -- a nested scope's file refuses the block
    /// (`refuse_misplaced_scope_secrets`) -- and an OpenShell provider names
    /// an entry with `credential: { secret: <name> }`. See `secrets`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<factory_environment::secrets::SecretDecl>,
    /// Where the daemon looks for out-of-process adapters, relative to
    /// `.factory/`. Defaults to `plugins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugins_dir: Option<PathBuf>,
    /// Important-date metadata for the whole instance. Never credential values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renewals: Vec<factory_infrastructure::renewals::RenewalDecl>,
    /// Optional standing owner opt-in for rare (one-day/overdue) push alerts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewals_notify: Option<factory_infrastructure::renewals::RenewalsNotify>,
}

impl Config {
    /// The roles every scope starts from: the two built in, plus the instance
    /// root's own. Not the whole answer for any nested scope -- see
    /// `roles_for_scope`.
    pub fn roles(&self) -> Result<Roles> {
        Roles::resolve(&self.roles)
    }

    /// The configured scopes above `scope`, top of the tree first.
    ///
    /// Ancestry is `Scope.path` and nothing else. Discovery produces a flat,
    /// path-sorted list and directory nesting is the only nesting it records;
    /// names may contain `/` and are matched loosely on purpose
    /// (`names_scope`, `last_segment`), so a name prefix is not a parent.
    /// Directories without a scope config are not in the list, so they are
    /// passed over without being asked.
    pub fn ancestors_of(&self, scope: &Scope) -> Vec<&Scope> {
        factory_kernel::scope_ancestors(&self.scopes, scope)
    }

    /// Every role in effect in `scope`: the built-in presets, the instance
    /// root's `roles:`, then each scope's `scope.roles` from the top of the
    /// tree down to `scope` itself. The nearest definition wins, whole.
    ///
    /// Roles never flow up or sideways: only ancestors and the scope itself
    /// are layered here, so a role `projects/a` defines does not exist in
    /// `projects/b`. And a definition inherited from above carries no
    /// authority from above -- reach is still the agent's own scope.
    pub fn roles_for_scope(&self, scope: &Scope) -> Result<Roles> {
        factory_agents::role_chain::roles_for_scope(&self.roles, &self.scopes, scope)
    }

    /// This instance's own resolved dashboard, with nothing below the root
    /// to ask about: the top-level `dashboard:`, if it named one, else the
    /// built-in default. The second element says where it came from: `None`
    /// for the built-in default, or `Some` naming the scope whose block
    /// answered -- the root's own configured scope name, or the instance
    /// name when the root never opted itself into being a scope at all
    /// (`root_policy_layer`'s own naming rule, mirrored here). Never a
    /// magic string like `"root"`: a scope can be named anything, including
    /// `root` or `default`, and the source is always either `None` or an
    /// actual name, never a word that could collide with one.
    pub fn dashboard(&self) -> (Option<&DashboardConfig>, Option<String>) {
        match &self.dashboard {
            Some(dashboard) => {
                let root_name = self
                    .scope
                    .as_ref()
                    .map(|s| s.name.clone())
                    .unwrap_or_else(|| self.instance.name.clone());
                (Some(dashboard), Some(root_name))
            }
            None => (None, None),
        }
    }

    /// The dashboard in effect in `scope`: the nearest `dashboard:` block
    /// walking from `scope` itself back up to the root -- `ancestors_of`,
    /// by `Scope.path`, the same chain `roles_for_scope` walks. Unlike
    /// roles, a dashboard layer is never merged with what came before it:
    /// the whole point of "nearest wins" here is that a layout stays one
    /// readable list, so the first `Some` found walking upward is the
    /// answer outright, with no folding step at all. The second element of
    /// the pair names the scope whose block won, or `None` when nothing
    /// anywhere in the chain names one (see `dashboard`'s own doc comment
    /// for why this is never a magic string).
    ///
    /// A scope with no `dashboard:` of its own is skipped exactly like an
    /// ancestor with no `roles:` is in `roles_for_scope` -- including the
    /// root's own `Scope` entry that discovery may place in `self.scopes`
    /// for path `.`, whose `dashboard` is always `None`
    /// (`refuse_root_scope_dashboard` keeps the top-level block the only
    /// way in for the root), so it is never mistaken for an override.
    pub fn dashboard_for_scope<'a>(
        &'a self,
        scope: &'a Scope,
    ) -> (Option<&'a DashboardConfig>, Option<String>) {
        for layer in self
            .ancestors_of(scope)
            .into_iter()
            .chain(std::iter::once(scope))
            .rev()
        {
            if let Some(dashboard) = &layer.dashboard {
                return (Some(dashboard), Some(layer.name.clone()));
            }
        }
        self.dashboard()
    }

    /// Every dashboard block this config declares, checked against the
    /// metric registry with the offending block and tile named -- the root's
    /// own `dashboard:` (path `"dashboard"`), then each scope's
    /// `scope.dashboard` (path `scope "{name}" dashboard`). Called from both
    /// `validate` (every scope) and `validate_instance` (the root only, the
    /// one layer that instance-only file can see) -- the same split those
    /// two already draw for roles.
    fn validate_dashboards<'a>(&self, scopes: impl Iterator<Item = &'a Scope>) -> Result<()> {
        if let Some(dashboard) = &self.dashboard {
            dashboard
                .validate("dashboard")
                .map_err(FactoryError::BadRequest)?;
        }
        for scope in scopes {
            if let Some(dashboard) = &scope.dashboard {
                dashboard
                    .validate(&format!("scope {:?} dashboard", scope.name))
                    .map_err(FactoryError::BadRequest)?;
            }
        }
        Ok(())
    }

    /// The instance root's own policy layer, if it declared any -- the top
    /// of every chain `policy_chain_for_scope` builds, and what
    /// `Factory::policy_chain` falls back to for a scope that no longer
    /// exists. Named from the root's own configured scope when it has one --
    /// the root refuses to also write `scope.policies`
    /// (`refuse_root_scope_policies`), so `self.policies` *is* that scope's
    /// layer -- or from the instance name, for an instance that never opted
    /// its root into being a scope at all.
    fn root_policy_layer(&self) -> Option<PolicyLayer> {
        factory_direction::policy_intent::root_layer(
            &self.policies,
            self.scope.as_ref().map(|scope| scope.name.as_str()),
            &self.instance.name,
        )
    }

    /// Every policy layer that applies to `scope`, root first: the instance
    /// root's own top-level `policies:`, then each ancestor's own
    /// `scope.policies` down to `scope` itself -- the same chain
    /// `roles_for_scope` walks (`ancestors_of`, by `Scope.path`, never by
    /// name). Unlike roles, nothing here *replaces* what came before: a
    /// policy layer only ever adds a framework or tightens a control, and
    /// folding that is `policy::applicable`'s job, not this method's -- this
    /// just resolves which layers exist, root to leaf, from the live config,
    /// keeping no second copy of the chain anywhere.
    ///
    /// Discovery may also have placed the root's own `Scope` (path `.`) in
    /// `self.scopes`, which then surfaces as an "ancestor" of everything;
    /// its `policies` is always empty (the same guard that keeps
    /// `root_policy_layer` the sole way the root's commitments enter the
    /// chain), so skipping empty layers below -- exactly as
    /// `roles_for_scope` already does -- keeps it from ever appearing twice.
    pub fn policy_chain_for_scope(&self, scope: &Scope) -> Vec<PolicyLayer> {
        factory_direction::policy_intent::chain_for_scope(
            self.root_policy_layer(),
            &self.scopes,
            scope,
        )
    }

    /// The instance root's own quality layer, if it binds any profile --
    /// named exactly as `root_policy_layer` names the root's policy layer,
    /// and for the same reason (`refuse_root_scope_quality` keeps
    /// `self.quality` the root scope's only layer).
    fn root_quality_layer(&self) -> Option<QualityLayer> {
        if self.quality.is_empty() {
            return None;
        }
        let root_name = self
            .scope
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| self.instance.name.clone());
        Some(QualityLayer {
            scope: root_name,
            profiles: self.quality.clone(),
        })
    }

    /// The instance root's own implicit definition-of-ready layer (`#169`)
    /// -- always present, unlike `root_quality_layer`: `ready.yaml` needs
    /// no list to bind it, so there is no "root binds nothing" case to
    /// return `None` for. Named the same way `root_quality_layer` is.
    fn root_intake_layer(&self) -> IntakeLayer {
        let root_name = self
            .scope
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_else(|| self.instance.name.clone());
        IntakeLayer {
            scope: root_name,
            files: vec!["ready".to_string()],
            required: false,
        }
    }

    /// Every quality layer that applies to `scope`, root first: the root's
    /// top-level `quality:`, then each ancestor's own `scope.quality` down to
    /// `scope` itself -- `policy_chain_for_scope`'s walk exactly (ancestry by
    /// `Scope.path`, never by name; empty layers skipped, so the root's own
    /// `Scope` entry in `self.scopes` never appears twice). Folding the
    /// chain -- add or tighten only -- is `quality::applicable`'s job, not
    /// this method's.
    pub fn quality_chain_for_scope(&self, scope: &Scope) -> Vec<QualityLayer> {
        let mut chain: Vec<QualityLayer> = self.root_quality_layer().into_iter().collect();
        for layer in self
            .ancestors_of(scope)
            .into_iter()
            .chain(std::iter::once(scope))
        {
            if layer.quality.is_empty() {
                continue;
            }
            chain.push(QualityLayer {
                scope: layer.name.clone(),
                profiles: layer.quality.clone(),
            });
        }
        chain
    }

    /// Every definition-of-ready layer that applies to `scope` (`#169`),
    /// root first through `ancestors_of` -- by `Scope.path`, never by name,
    /// so a sibling never inherits, exactly `quality_chain_for_scope`'s
    /// walk. Unlike that walk, though, the first layer is not conditional
    /// on any list: it is the instance root's own implicit `"ready"` layer,
    /// always present (`required: false` -- see `ready::IntakeLayer` and
    /// the module doc's "Fail closed" section for what that flag does), and
    /// named from the same root scope `root_quality_layer` uses. Every
    /// further layer comes from an ancestor's own `scope.intake`, each
    /// `required: true`: nothing silently drops a name a scope bound on
    /// purpose. Folding the chain -- add or tighten only, fail closed on an
    /// unreadable file -- is `ready::effective`'s job, not this method's.
    pub fn intake_chain_for_scope(&self, scope: &Scope) -> Vec<IntakeLayer> {
        let mut chain = vec![self.root_intake_layer()];
        for layer in self
            .ancestors_of(scope)
            .into_iter()
            .chain(std::iter::once(scope))
        {
            if layer.intake.is_empty() {
                continue;
            }
            chain.push(IntakeLayer {
                scope: layer.name.clone(),
                files: layer.intake.clone(),
                required: true,
            });
        }
        chain
    }

    /// Refuse a config that gives an agent a role nothing in its scope's chain
    /// defines, and say which agent it was and what that scope does have.
    /// Falling back to the default instead would demote an agent on a typo
    /// and never mention it.
    pub fn validate(&self) -> Result<()> {
        factory_infrastructure::renewals::validate(&self.renewals, self.renewals_notify.as_ref())?;
        self.roles()?;
        self.refuse_root_scope_roles()?;
        self.refuse_root_scope_policies()?;
        self.refuse_root_scope_quality()?;
        self.refuse_root_scope_intake()?;
        self.refuse_root_scope_dashboard()?;
        self.validate_dashboards(self.scopes.iter())?;
        self.infrastructure.validate()?;
        factory_environment::secrets::validate(&self.secrets)?;
        self.validate_environments()?;
        for scope in self.scope.iter().chain(&self.scopes) {
            factory_infrastructure::renewals::validate(&scope.renewals, None)?;
            scope.validate_dependencies()?;
            refuse_zero_max_sessions(scope)?;
            let roles = self.roles_for_scope(scope)?;
            for agent in scope.declared_agents() {
                refuse_shell_args(scope, &agent)?;
                refuse_bad_openshell(scope, &agent)?;
                refuse_bad_secret_refs(&self.secrets, scope, &agent)?;
                self.infrastructure.refuse_unknown_provider(scope, &agent)?;
                if !roles.contains(&agent.role) {
                    return Err(FactoryError::BadRequest(format!(
                        "scope {:?} gives {:?} the role {:?}, which is not defined in that scope. \
                         The roles available in {:?} are: {}",
                        scope.name,
                        agent.name(),
                        agent.role.as_str(),
                        scope.name,
                        roles.names().join(", ")
                    )));
                }
            }
        }
        Ok(())
    }

    /// Every scope's environments together: a name is unique across the
    /// instance, and `promotes_to` may name one in another scope.
    pub fn validate_environments(&self) -> Result<()> {
        // The root scope is `scope:` and, once discovered, one of `scopes:`
        // too; the same scope twice is not two declarations.
        let mut seen = std::collections::BTreeSet::new();
        factory_infrastructure::environments::validate(
            self.scope
                .iter()
                .chain(&self.scopes)
                .filter(|s| seen.insert(s.name.as_str()))
                .map(|s| (s.name.as_str(), s.environments.as_slice())),
        )
    }

    /// Every declared environment, with the name of the scope declaring it.
    pub fn environments(
        &self,
    ) -> Vec<(
        String,
        factory_infrastructure::environments::EnvironmentDecl,
    )> {
        let mut out: Vec<(
            String,
            factory_infrastructure::environments::EnvironmentDecl,
        )> = Vec::new();
        for scope in self.scope.iter().chain(&self.scopes) {
            for env in &scope.environments {
                // The root scope can also be listed in `scopes:`; one copy.
                if !out.iter().any(|(_, e)| e.name == env.name) {
                    out.push((scope.name.clone(), env.clone()));
                }
            }
        }
        out
    }

    /// Validate the instance file before discovery replaces its legacy scope
    /// list. Local scope files are checked by `validate` after discovery.
    ///
    /// This runs before discovery, so the only roles it can know are the
    /// instance root's. That is the whole answer for the root scope, which is
    /// the only scope checked here, and its message says it is the root's
    /// list -- never that it is every role a nested scope might have.
    pub fn validate_instance(&self) -> Result<()> {
        factory_infrastructure::renewals::validate(&self.renewals, self.renewals_notify.as_ref())?;
        let roles = self.roles()?;
        self.refuse_root_scope_roles()?;
        self.refuse_root_scope_policies()?;
        self.refuse_root_scope_quality()?;
        self.refuse_root_scope_intake()?;
        self.refuse_root_scope_dashboard()?;
        self.validate_dashboards(std::iter::empty())?;
        self.infrastructure.validate()?;
        factory_environment::secrets::validate(&self.secrets)?;
        if let Some(scope) = &self.scope {
            factory_infrastructure::renewals::validate(&scope.renewals, None)?;
            scope.validate_dependencies()?;
            refuse_zero_max_sessions(scope)?;
            for agent in scope.declared_agents() {
                refuse_shell_args(scope, &agent)?;
                refuse_bad_openshell(scope, &agent)?;
                refuse_bad_secret_refs(&self.secrets, scope, &agent)?;
                self.infrastructure.refuse_unknown_provider(scope, &agent)?;
                if !roles.contains(&agent.role) {
                    return Err(FactoryError::BadRequest(format!(
                        "scope {:?} gives {:?} the role {:?}, which the instance root does not define. \
                         The roles defined at the instance root are: {}",
                        scope.name,
                        agent.name(),
                        agent.role.as_str(),
                        roles.names().join(", ")
                    )));
                }
            }
        }
        Ok(())
    }

    /// The instance root already has a role layer, its top-level `roles:`.
    /// One file with two would leave a person guessing which one wins.
    fn refuse_root_scope_roles(&self) -> Result<()> {
        match &self.scope {
            Some(scope) if !scope.roles.is_empty() => Err(FactoryError::BadRequest(format!(
                "the instance root's config gives its scope {:?} a `scope.roles` block. \
                 The root's roles belong in its top-level `roles:`; move {} there",
                scope.name,
                scope.roles.keys().cloned().collect::<Vec<_>>().join(", ")
            ))),
            _ => Ok(()),
        }
    }

    /// The instance root already has a policy layer, its top-level
    /// `policies:`. One file with two would leave a person guessing which
    /// one wins -- the same reasoning as `refuse_root_scope_roles`, for
    /// policies instead.
    fn refuse_root_scope_policies(&self) -> Result<()> {
        match &self.scope {
            Some(scope) if !scope.policies.is_empty() => Err(FactoryError::BadRequest(format!(
                "the instance root's config gives its scope {:?} a `scope.policies` block. \
                 The root's policies belong in its top-level `policies:`; move them there",
                scope.name,
            ))),
            _ => Ok(()),
        }
    }

    /// The instance root already has a quality layer, its top-level
    /// `quality:` -- `refuse_root_scope_policies`'s reasoning, for quality
    /// profiles instead.
    fn refuse_root_scope_quality(&self) -> Result<()> {
        match &self.scope {
            Some(scope) if !scope.quality.is_empty() => Err(FactoryError::BadRequest(format!(
                "the instance root's config gives its scope {:?} a `scope.quality` block. \
                 The root's quality profiles belong in its top-level `quality:`; move {} there",
                scope.name,
                scope.quality.join(", ")
            ))),
            _ => Ok(()),
        }
    }

    /// The instance root already has a dashboard layer, its top-level
    /// `dashboard:` -- `refuse_root_scope_policies`'s reasoning, for the
    /// dashboard layout instead (`#159`).
    fn refuse_root_scope_dashboard(&self) -> Result<()> {
        match &self.scope {
            Some(scope) if scope.dashboard.is_some() => Err(FactoryError::BadRequest(format!(
                "the instance root's config gives its scope {:?} a `scope.dashboard` block. \
                 The root's dashboard belongs in its top-level `dashboard:`; move it there",
                scope.name,
            ))),
            _ => Ok(()),
        }
    }

    /// Unlike `roles`, `policies` and `quality` above, the root's own
    /// definition of ready (`#169`) is not a list to move a `scope.intake`
    /// binding to -- `.factory/intake/ready.yaml` is the root's whole layer
    /// automatically, if it exists, and nothing else. So the root's own
    /// scope entry may not bind named definitions at all.
    fn refuse_root_scope_intake(&self) -> Result<()> {
        match &self.scope {
            Some(scope) if !scope.intake.is_empty() => Err(FactoryError::BadRequest(format!(
                "the instance root's config gives its scope {:?} a `scope.intake` block. \
                 The root already applies .factory/intake/ready.yaml to every scope automatically; \
                 a nested scope binds further definitions of ready with its own `scope.intake`, but \
                 the root has no list of its own to move {} to",
                scope.name,
                scope.intake.join(", ")
            ))),
            _ => Ok(()),
        }
    }
}

fn refuse_shell_args(scope: &Scope, agent: &ScopeAgent) -> Result<()> {
    if agent.harness == "shell" && !agent.args.is_empty() {
        return Err(FactoryError::BadRequest(format!(
            "scope {:?} gives shell agent {:?} arguments, but the shell agent runs the task's instructions directly and cannot use them",
            scope.name,
            agent.name(),
        )));
    }
    Ok(())
}

/// `sandbox: openshell` and its `openshell:` block come together or not at
/// all, and the block itself has to be one a sandbox can be made from
/// (`OpenshellConfig::problems`). Refused at load, naming the scope's path,
/// like `refuse_zero_max_sessions` -- a run that only found out at dispatch
/// would fail every week until somebody read the journal.
pub fn refuse_bad_openshell(scope: &Scope, agent: &ScopeAgent) -> Result<()> {
    let refuse = |what: String| {
        Err(FactoryError::BadRequest(format!(
            "scope {:?} at {} gives {:?} {what}",
            scope.name,
            scope.path.display(),
            agent.name(),
        )))
    };
    match (agent.sandbox, &agent.openshell) {
        (Sandbox::Openshell, None) => refuse(
            "sandbox: openshell without an openshell: block; it needs at least a policy".to_string(),
        ),
        (other, Some(_)) if other != Sandbox::Openshell => refuse(format!(
            "an openshell: block but sandbox: {}; set sandbox: openshell or remove the block",
            other.as_str()
        )),
        (_, Some(_)) if agent.lifetime != Lifetime::Task => refuse(
            "sandbox: openshell for a standing agent; this version supports only lifetime: task and will not start a standing harness on the host instead".to_string(),
        ),
        (_, Some(block)) => match block.problems().first() {
            Some(problem) => refuse(format!("an openshell: block that {problem}")),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Every `credential: { secret: <name> }` an agent's OpenShell providers
/// give names a declared secret, and a provider's own `expires:` agrees with
/// that secret's -- the catalogue's is the one that holds (`#244`), so two
/// dates that disagree are refused with both named rather than one quietly
/// winning.
pub fn refuse_bad_secret_refs(
    catalogue: &[factory_environment::secrets::SecretDecl],
    scope: &Scope,
    agent: &ScopeAgent,
) -> Result<()> {
    let Some(block) = &agent.openshell else {
        return Ok(());
    };
    for provider in &block.providers {
        let factory_environment::openshell::ProviderDecl::Managed(managed) = provider else {
            continue;
        };
        let Some(name) = managed.credential.secret() else {
            continue;
        };
        let refuse = |what: String| {
            Err(FactoryError::BadRequest(format!(
                "scope {:?} gives {:?} the OpenShell provider {:?} {what}",
                scope.name,
                agent.name(),
                managed.name,
            )))
        };
        let Some(secret) = factory_environment::secrets::find(catalogue, name) else {
            let declared: Vec<&str> = catalogue.iter().map(|s| s.name.as_str()).collect();
            return refuse(format!(
                "the credential {{ secret: {name} }}, which the instance root's secrets: does not declare (it declares: {})",
                if declared.is_empty() { "nothing".to_string() } else { declared.join(", ") }
            ));
        };
        let Some(own) = managed.expires else { continue };
        let own = factory_environment::secrets::Expiry::On(own);
        match secret.expires {
            Some(catalogue) if catalogue == own => {}
            Some(catalogue) => {
                return refuse(format!(
                    "expires: {own}, and the secret {name} it names says expires: {catalogue}; \
                     the secret's date is the one that holds, so remove expires: from the provider"
                ))
            }
            None => {
                return refuse(format!(
                    "expires: {own}, and the secret {name} it names gives no expires:; \
                     move the date to the secret in the instance root's secrets:"
                ))
            }
        }
    }
    Ok(())
}

/// A `secrets:` block in a nested scope's file: only the instance root's is
/// read, so one written anywhere else would quietly declare nothing.
pub fn refuse_misplaced_scope_secrets(document: &serde_yaml_ng::Value, path: &Path) -> Result<()> {
    let misplaced = document
        .as_mapping()
        .is_some_and(|root| root.contains_key(serde_yaml_ng::Value::String("secrets".into())));
    if misplaced {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has a `secrets:` block, which only the instance root's config reads. \
             Move its entries into the root .factory/config.yaml",
            path.display()
        )));
    }
    Ok(())
}

/// A `max_sessions: 0` would never run anything -- which is never what
/// somebody meant by it, only ever a stand-in for "unlimited" that should
/// have been left out (`#179`). Refused at load, naming the scope's path so
/// there is no ambiguity about which of two same-named scopes it was.
fn refuse_zero_max_sessions(scope: &Scope) -> Result<()> {
    if scope.max_sessions == Some(0) {
        return Err(FactoryError::BadRequest(format!(
            "scope {:?} at {} sets max_sessions: 0, which would never run anything; leave it out for no limit",
            scope.name,
            scope.path.display(),
        )));
    }
    for agent in scope.declared_agents() {
        if agent.max_sessions == Some(0) {
            return Err(FactoryError::BadRequest(format!(
                "scope {:?} at {} gives {:?} max_sessions: 0, which would never run anything; leave it out for no limit",
                scope.name,
                scope.path.display(),
                agent.name(),
            )));
        }
    }
    Ok(())
}

/// Refuse a `roles:` block written at the top of a nested scope's own config.
/// Only the `scope:` block of that file is Factory's, so serde would drop it
/// without a word -- and a role that silently does not exist is found only
/// when an agent is refused for holding it.
pub fn refuse_misplaced_scope_roles(document: &serde_yaml_ng::Value, path: &Path) -> Result<()> {
    let misplaced = document
        .as_mapping()
        .is_some_and(|root| root.contains_key(serde_yaml_ng::Value::String("roles".into())));
    if misplaced {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has a top-level `roles:` block, which a scope's own file does not read. \
             Move it under `scope.roles`",
            path.display()
        )));
    }
    Ok(())
}

/// Refuse a `policies:` block written at the top of a nested scope's own
/// config, the same mistake `refuse_misplaced_scope_roles` guards against:
/// only the `scope:` block of that file is Factory's, so serde would drop a
/// top-level `policies:` without a word -- and a commitment that silently
/// does not apply is far worse to find late than a role.
pub fn refuse_misplaced_scope_policies(document: &serde_yaml_ng::Value, path: &Path) -> Result<()> {
    let misplaced = document
        .as_mapping()
        .is_some_and(|root| root.contains_key(serde_yaml_ng::Value::String("policies".into())));
    if misplaced {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has a top-level `policies:` block, which a scope's own file does not read. \
             Move it under `scope.policies`",
            path.display()
        )));
    }
    Ok(())
}

/// Refuse a `quality:` block written at the top of a nested scope's own
/// config -- `refuse_misplaced_scope_policies`'s mistake, for quality
/// profiles: serde would drop it without a word, and a scope would then be
/// held to nothing while its author believes otherwise.
pub fn refuse_misplaced_scope_quality(document: &serde_yaml_ng::Value, path: &Path) -> Result<()> {
    let misplaced = document
        .as_mapping()
        .is_some_and(|root| root.contains_key(serde_yaml_ng::Value::String("quality".into())));
    if misplaced {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has a top-level `quality:` block, which a scope's own file does not read. \
             Move it under `scope.quality`",
            path.display()
        )));
    }
    Ok(())
}

/// Refuse a `dashboard:` block written at the top of a nested scope's own
/// config -- `refuse_misplaced_scope_quality`'s mistake, for the dashboard
/// layout instead (`#159`): serde would drop it without a word, and a scope
/// would keep inheriting whatever it stood to override while its author
/// believes otherwise. Called only from `discovery.rs`'s `read_scope`, the
/// same as `refuse_misplaced_scope_quality` -- `configuration.rs`'s
/// `read_document` also reads the instance root's own file, where a
/// top-level `dashboard:` is exactly right.
pub fn refuse_misplaced_scope_dashboard(
    document: &serde_yaml_ng::Value,
    path: &Path,
) -> Result<()> {
    let misplaced = document
        .as_mapping()
        .is_some_and(|root| root.contains_key(serde_yaml_ng::Value::String("dashboard".into())));
    if misplaced {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has a top-level `dashboard:` block, which a scope's own file does not read. \
             Move it under `scope.dashboard`",
            path.display()
        )));
    }
    Ok(())
}

/// Refuse an `infrastructure:` block written at the top of a nested scope's
/// own config, the same mistake `refuse_misplaced_scope_roles` guards
/// against: the AI accounts are declared once, for the whole instance, in
/// the root config, and serde would otherwise drop this block without a word
/// -- leaving every agent it meant to bind showing up as unassigned.
pub fn refuse_misplaced_scope_infrastructure(
    document: &serde_yaml_ng::Value,
    path: &Path,
) -> Result<()> {
    let misplaced = document.as_mapping().is_some_and(|root| {
        root.contains_key(serde_yaml_ng::Value::String("infrastructure".into()))
    });
    if misplaced {
        return Err(FactoryError::BadRequest(format!(
            "scope config {} has an `infrastructure:` block, which only the instance root's config reads. \
             Move its providers into the root .factory/config.yaml",
            path.display()
        )));
    }
    Ok(())
}

fn default_version() -> u32 {
    1
}

/// Common scope-tree resolution, independent of any policy service.
pub fn subtree_scopes(
    snapshot: &Factory,
    scope: Option<&str>,
) -> Result<(Option<Scope>, Vec<Scope>)> {
    snapshot.subtree_scopes(scope)
}

/// Nested files read only `scope:`; never silently drop renewal metadata.
pub fn refuse_misplaced_scope_renewals(document: &serde_yaml_ng::Value, path: &Path) -> Result<()> {
    for key in ["renewals", "renewals_notify"] {
        if document
            .as_mapping()
            .is_some_and(|root| root.contains_key(serde_yaml_ng::Value::String(key.into())))
        {
            let destination = if key == "renewals" {
                "under scope.renewals"
            } else {
                "in the instance root"
            };
            return Err(FactoryError::BadRequest(format!(
                "scope config {} has top-level {key}; put it {destination}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instance {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// Which task-store adapter backs the CRUD contract.
    #[serde(default = "default_store")]
    pub task_store: String,
    /// Which knowledge provider answers `factory knowledge search`. One for
    /// the whole instance, never per scope: the vault is company-wide, and
    /// so is the search over it.
    #[serde(default = "default_knowledge_provider")]
    pub knowledge_provider: String,
    /// Interfaces the daemon mounts at startup.
    #[serde(default = "default_interfaces")]
    pub interfaces: Vec<InterfaceConfig>,
    /// How often the scheduler looks for due tasks.
    #[serde(default = "default_tick")]
    pub tick_seconds: u64,
    /// A cap on how long a dispatched task's run may take in total, counted
    /// from when it started -- not a check on how long it has gone without
    /// reporting, so a run that reports constantly is still failed once this
    /// is reached.
    #[serde(default = "default_task_timeout")]
    pub task_timeout_seconds: u64,
    /// How long an agent has to say it has started. An agent that is up but
    /// sitting on a prompt nobody will answer -- a first-run trust dialog, a
    /// login -- never acknowledges, and there is no reason to hold the task
    /// open for the full run timeout to find that out.
    #[serde(default = "default_ack_timeout")]
    pub ack_timeout_seconds: u64,
    /// How long a run reported `Blocked` by a runtime hook may sit waiting
    /// for a human before the daemon gives up on it too. Deliberately its own
    /// clock rather than a reuse of `task_timeout_seconds`: a person needs a
    /// real chance to notice and answer, but a wedged run still cannot wait
    /// forever with nobody told. A day by default.
    #[serde(default = "default_blocked_timeout")]
    pub blocked_timeout_seconds: u64,
    /// Defaults for tasks that do not name their own.
    #[serde(default = "default_agent")]
    pub default_agent: String,
    #[serde(default = "default_runtime")]
    pub default_runtime: String,
    /// How a scheduled task's failed run is retried, for a task that does
    /// not set `Task::retry` itself. On by default: a scheduled task exists
    /// because someone needs it to actually happen on a cadence, and a
    /// transient failure silently costing a whole cycle -- a week, for a
    /// weekly task -- is the more surprising default of the two. A task for
    /// which a re-run would be actively harmful sets `retry: none` itself.
    #[serde(default = "default_retry")]
    pub default_retry: factory_process::task::RetryPolicy,
    /// Give every scope a foreman without writing one into each of them.
    #[serde(default)]
    pub foreman: ForemanConfig,
    /// Hold an OS-level "do not sleep" assertion for as long as any run is
    /// active -- see issue #61. `task_timeout_seconds` is spent by wall
    /// clock, which keeps ticking while the host cannot execute anything, so
    /// a laptop that drops into a DarkWake-and-back-to-sleep cycle burns a
    /// run's whole budget on time nobody could use. On by default; the
    /// platform seam in `factory-daemon::power` no-ops on anything but
    /// macOS, so this is safe to leave on everywhere.
    #[serde(default = "default_power_assertion")]
    pub power_assertion: bool,
    /// Check a harness starts before handing it a task (`#131`).
    #[serde(default)]
    pub harness_health: HarnessHealthConfig,
}

/// How the daemon checks that a harness starts before dispatching to it.
///
/// ```yaml
/// daemon:
///   harness_health:
///     enabled: true            # probe before dispatch (the default)
///     timeout_seconds: 10      # how long `<harness> --version` may take
///     cache_seconds: 180       # how long a healthy answer is trusted
///     retry_seconds: 60        # how often an unhealthy one is probed again
///     repair_script: /path/to/factory/scripts/repair-harness
///     auto_repair: false       # opt-in: run repair_script on its own
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessHealthConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_probe_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "default_probe_cache")]
    pub cache_seconds: u64,
    #[serde(default = "default_probe_retry")]
    pub retry_seconds: u64,
    /// Where `scripts/repair-harness` is on this host. Named in a blocked
    /// task's reason, and what `auto_repair` runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repair_script: Option<String>,
    /// Run `repair_script <harness>` on its own when a harness stops
    /// answering, once per unhealthy stretch. Off unless the owner turns it
    /// on: the repair overrides a decision macOS is holding, so by default a
    /// person runs it. Does nothing without `repair_script`, and the script's
    /// own signature check applies either way.
    #[serde(default)]
    pub auto_repair: bool,
}

fn default_true() -> bool {
    true
}
fn default_probe_timeout() -> u64 {
    10
}
fn default_probe_cache() -> u64 {
    180
}
fn default_probe_retry() -> u64 {
    60
}

impl Default for HarnessHealthConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_seconds: default_probe_timeout(),
            cache_seconds: default_probe_cache(),
            retry_seconds: default_probe_retry(),
            repair_script: None,
            auto_repair: false,
        }
    }
}

pub use factory_agents::roster::ForemanConfig;

fn default_store() -> String {
    "sqlite".into()
}
fn default_knowledge_provider() -> String {
    "keyword".into()
}
fn default_tick() -> u64 {
    5
}
fn default_task_timeout() -> u64 {
    3600
}
fn default_ack_timeout() -> u64 {
    180
}
fn default_blocked_timeout() -> u64 {
    86400
}
fn default_agent() -> String {
    factory_agents::roster::default_agent()
}
fn default_runtime() -> String {
    "herdr".into()
}
fn default_power_assertion() -> bool {
    true
}
/// Three attempts, five minutes apart. Enough to ride out the transient
/// failures a retry is for -- a runtime hiccup, a moment's network trouble --
/// without turning into a slow-motion version of the very cadence the task's
/// own schedule already provides.
fn default_retry() -> factory_process::task::RetryPolicy {
    factory_process::task::RetryPolicy::Backoff {
        max_attempts: 3,
        backoff_seconds: 300,
    }
}
fn default_interfaces() -> Vec<InterfaceConfig> {
    vec![
        InterfaceConfig {
            kind: "cli".into(),
            settings: BTreeMap::new(),
        },
        InterfaceConfig {
            kind: "http".into(),
            settings: BTreeMap::new(),
        },
    ]
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            task_store: default_store(),
            knowledge_provider: default_knowledge_provider(),
            interfaces: default_interfaces(),
            tick_seconds: default_tick(),
            task_timeout_seconds: default_task_timeout(),
            ack_timeout_seconds: default_ack_timeout(),
            blocked_timeout_seconds: default_blocked_timeout(),
            default_agent: default_agent(),
            default_runtime: default_runtime(),
            default_retry: default_retry(),
            foreman: ForemanConfig::default(),
            power_assertion: default_power_assertion(),
            harness_health: HarnessHealthConfig::default(),
        }
    }
}

pub use factory_infrastructure::interfaces::{InterfaceConfig, DEFAULT_HTTP_BIND};

pub use factory_environment::sandbox::Sandbox;

/// The root config's `infrastructure:` block: what the agents run on that
/// Factory does not run itself: `providers`, the AI accounts that pay for
/// the agents' model calls, and `backup`, where the instance's own state is
/// copied to.
///
/// **Declared, never discovered.** Nothing in Factory opens a credential
/// file, the Keychain or an `.env` to find out which accounts exist, and
/// nothing here holds a secret: an api-key provider may name the environment
/// variable its key lives in, and that *name* is all Factory ever keeps or
/// shows -- the variable is never read.
///
/// `deny_unknown_fields`, on this and on `Provider`: `harness:` for
/// `harnesses:` would otherwise bind nothing and parse clean, and every agent
/// it meant to cover would quietly show up as unassigned.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Infrastructure {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<Provider>,
    /// Where and when the instance's own state is backed up (`#116`). See
    /// `backup::BackupConfig`; absent means no backup is configured, which
    /// the L1 Backup page says in red rather than leaving blank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<factory_infrastructure::backup::BackupConfig>,
}

/// One AI account, as the root config declares it.
///
/// ```yaml
/// infrastructure:
///   providers:
///     - name: claude-max
///       vendor: anthropic
///       kind: subscription
///       plan: Max 20x
///       harnesses: [claude-code]
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    /// Unique across the instance. What an agent's `provider:` names.
    pub name: String,
    /// Who sells the account: `anthropic`, `openrouter`, ... Free text.
    pub vendor: String,
    pub kind: ProviderKind,
    /// Free text, shown as written: `Max 20x`, `pay as you go`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    /// The *name* of the environment variable an api-key provider's key
    /// lives in. Shown, never read. Refused on a subscription, which has no
    /// key to point at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// The default binding: every agent on one of these harnesses uses this
    /// provider unless its own `provider:` says otherwise. A harness may be
    /// claimed by at most one provider.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub harnesses: Vec<String>,
}

/// How an account is paid for. An unknown kind is refused when the config is
/// parsed, naming the two there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    Subscription,
    ApiKey,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Subscription => "subscription",
            Self::ApiKey => "api-key",
        }
    }
}

/// Which rule bound an agent to its provider: its own `provider:`, or the
/// provider whose `harnesses:` lists its harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderVia {
    Agent,
    Harness,
}

impl ProviderVia {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Harness => "harness",
        }
    }
}

/// `shell` runs the task's instructions as a command. It makes no model
/// call, so it never has a provider -- and is never listed as missing one.
pub const SHELL_HARNESS: &str = "shell";

impl Infrastructure {
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty() && self.backup.is_none()
    }

    pub fn provider(&self, name: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.name == name)
    }

    /// The provider `agent` uses, and which rule chose it. An agent's own
    /// `provider:` wins; otherwise the one provider whose `harnesses:` lists
    /// its harness. `shell` never has one, and neither does a model agent
    /// nothing claims -- that is not an error, it is what the view lists as
    /// unassigned. A `provider:` naming nothing declared also answers `None`
    /// here; `validate` is what refuses it, at load.
    pub fn provider_for(&self, agent: &ScopeAgent) -> Option<(&Provider, ProviderVia)> {
        if agent.harness == SHELL_HARNESS {
            return None;
        }
        if let Some(name) = &agent.provider {
            return self.provider(name).map(|p| (p, ProviderVia::Agent));
        }
        self.providers
            .iter()
            .find(|p| p.harnesses.iter().any(|h| h == &agent.harness))
            .map(|p| (p, ProviderVia::Harness))
    }

    /// The load-time refusals that concern the provider list alone:
    /// duplicate names, one harness claimed twice, an `env:` on a
    /// subscription, and a provider claiming `shell`. An unknown `kind` never
    /// gets this far -- serde refuses it while parsing.
    pub fn validate(&self) -> Result<()> {
        if let Some(backup) = &self.backup {
            backup.validate()?;
        }
        let mut names = std::collections::BTreeSet::new();
        let mut claimed: BTreeMap<&str, &str> = BTreeMap::new();
        for provider in &self.providers {
            if provider.name.trim().is_empty() {
                return Err(FactoryError::BadRequest(
                    "infrastructure.providers has a provider with no name".into(),
                ));
            }
            if !names.insert(provider.name.as_str()) {
                return Err(FactoryError::BadRequest(format!(
                    "infrastructure.providers declares {:?} twice. Provider names must be unique",
                    provider.name
                )));
            }
            if provider.kind == ProviderKind::Subscription {
                if let Some(env) = &provider.env {
                    return Err(FactoryError::BadRequest(format!(
                        "provider {:?} is a subscription but names the environment variable {:?}. \
                         Only an api-key provider has a key to point at; remove `env:` or make it `kind: api-key`",
                        provider.name, env
                    )));
                }
            }
            for harness in &provider.harnesses {
                if harness == SHELL_HARNESS {
                    return Err(FactoryError::BadRequest(format!(
                        "provider {:?} claims the shell harness, which makes no model call and never has a provider",
                        provider.name
                    )));
                }
                if let Some(first) = claimed.insert(harness.as_str(), provider.name.as_str()) {
                    return Err(FactoryError::BadRequest(format!(
                        "providers {:?} and {:?} both claim the harness {:?}. A harness may default to one provider; \
                         give the agents that should use the other an explicit `provider:` instead",
                        first, provider.name, harness
                    )));
                }
            }
        }
        Ok(())
    }

    /// Refuse an agent whose `provider:` names nothing declared, or that
    /// gives the shell harness a provider at all. Falling back to the harness
    /// default instead would put a typo's model calls on a different account
    /// and never mention it. `configure_agent` asks the same question before
    /// it writes a declaration, so the roster cannot write a file the next
    /// start would refuse.
    pub fn refuse_unknown_provider(&self, scope: &Scope, agent: &ScopeAgent) -> Result<()> {
        let Some(name) = &agent.provider else {
            return Ok(());
        };
        if agent.harness == SHELL_HARNESS {
            return Err(FactoryError::BadRequest(format!(
                "scope {:?} gives shell agent {:?} the provider {:?}, but the shell agent makes no model call and never has one",
                scope.name,
                agent.name(),
                name
            )));
        }
        if self.provider(name).is_none() {
            return Err(FactoryError::BadRequest(format!(
                "scope {:?} gives {:?} the provider {:?}, which infrastructure.providers in the root config does not declare. {}",
                scope.name,
                agent.name(),
                name,
                self.declared_list()
            )));
        }
        Ok(())
    }

    /// What there is instead, for a refusal to say.
    fn declared_list(&self) -> String {
        if self.providers.is_empty() {
            "No providers are declared".into()
        } else {
            format!(
                "The declared providers are: {}",
                self.providers
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

pub use factory_agents::roster::{AgentRef, ScopeAgent};

/// A directory Factory can run agents in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scope {
    /// Stable identity stored with the scope rather than inferred from its
    /// location. Discovery requires this to be non-empty.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// Derived from the directory containing `.factory/config.yaml`.
    #[serde(default, skip_serializing)]
    pub path: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentRef>,
    /// Further agents in this scope. A scope can have as many as it likes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<ScopeAgent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<String>,
    /// Which task-store adapter holds this scope's tasks. Absent means the
    /// instance default, `daemon.task_store`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_store: Option<String>,
    /// How many sessions this scope may hold open at once, across every
    /// agent in it -- counted the same way an agent's own `max_sessions` is
    /// (`#179`). Absent is unlimited. `0` is refused at load. The root
    /// scope's own `scope:` block may set this too, for an instance-wide cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_sessions: Option<u32>,
    /// Roles this scope names for itself and for every scope below it by
    /// path. A same-named role here replaces the inherited one whole. Only a
    /// nested scope writes these: the instance root uses its top-level
    /// `roles:` instead, and refuses this block.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub roles: BTreeMap<String, RoleSpec>,
    /// This scope's own dashboard layout, replacing whatever it inherited
    /// whole -- see `Config::dashboard_for_scope`. Only a nested scope
    /// writes this: the instance root uses its top-level `dashboard:`
    /// instead, and refuses this block, exactly like `roles` above. `None`
    /// inherits; `Some` replaces, with at least one tile -- an empty
    /// `tiles: []` is refused at validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard: Option<DashboardConfig>,
    /// This scope's own policy declarations, layered under the root's
    /// `policies:` and every ancestor's own `scope.policies` -- see
    /// `Config::policy_chain_for_scope`. Only a nested scope writes these:
    /// the instance root uses its top-level `policies:` instead, and
    /// refuses this block, exactly like `roles` above.
    #[serde(default, skip_serializing_if = "PolicyDeclaration::is_empty")]
    pub policies: PolicyDeclaration,
    /// Quality profiles this scope binds for itself and every scope below it
    /// by path, layered under the root's `quality:` and every ancestor's own
    /// `scope.quality` -- see `Config::quality_chain_for_scope`. Only a
    /// nested scope writes these; the instance root refuses this block, the
    /// same as `policies` above.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quality: Vec<String>,
    /// Named definitions of ready (`#169`) this scope binds for itself and
    /// every scope below it by path, on top of the instance root's own
    /// implicit `.factory/intake/ready.yaml` layer -- see
    /// `Config::intake_chain_for_scope`. Each name is a file stem,
    /// `.factory/intake/<name>.yaml`. Unlike `quality` above there is no
    /// top-level list this binds instead of: the root layer is always
    /// `ready.yaml` if it exists, nothing else, so the instance root's own
    /// scope entry refuses this block outright
    /// (`Config::refuse_root_scope_intake`) rather than being told where
    /// else to put it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intake: Vec<String>,
    /// Declared product components and external services for L2 Dependencies.
    #[serde(default, skip_serializing_if = "DependenciesConfig::is_empty")]
    pub dependencies: DependenciesConfig,
    /// Where what this scope builds is deployed and kept running (`#185`):
    /// each environment's tier, URL, health checks and SLO. See
    /// `environments::EnvironmentDecl`; names are unique across the
    /// instance, checked by `Config::validate`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environments: Vec<factory_infrastructure::environments::EnvironmentDecl>,
    /// This scope's authored important-date metadata, kept in its own config.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renewals: Vec<factory_infrastructure::renewals::RenewalDecl>,
    /// `#278`: this scope's own reported-metrics source -- a file the
    /// scope's own tooling writes, and the metrics it declares through it
    /// (`reported.<source.id>.<declare[].id>`). Own-scope data, not a
    /// chained declaration like `policies`/`quality` above: nothing here
    /// inherits down, and nothing is validated at parse time -- an id
    /// shape, a duplicate source id or a path that escapes the instance is
    /// a finding L5's `reported::validate` raises on read, never a reason
    /// this config fails to load. See `reported.rs`'s own doc comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<ScopeMetricsDeclaration>,
}

/// `scope.metrics`, exactly as written -- plain strings, no validation.
/// `source` is the one file this scope reports through; `declare` names
/// every metric id that file may report, with the title and unit L5's
/// registry listing shows for it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeMetricsDeclaration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<ScopeMetricsSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub declare: Vec<ScopeMetricsDeclared>,
}

/// Every field is optional here on purpose: a *missing* `id`/`file` is an
/// authoring mistake the same as an invalid one (a bad slug, a path that
/// escapes the instance), and L5's `reported::validate` turns either into
/// the same kind of finding -- never a parse failure that would abort
/// `discovery::apply` and take the whole daemon down with it on the next
/// restart. Unknown keys are still refused (`deny_unknown_fields`), the
/// same convention every neighbouring scope block already follows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeMetricsSource {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

/// See [`ScopeMetricsSource`]'s doc comment: every field is optional for
/// the same reason. A missing `better` is handled exactly like an unknown
/// spelling of it -- the same `UnknownBetter` finding, this one declared
/// metric left out -- never a reason the scope's whole config file fails
/// to parse.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeMetricsDeclared {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// `higher` or `lower`. `metrics::resolve` is pure and has no access
    /// to this declaration, so a missing direction can never be guessed at
    /// `higher` without risking exactly the wrong-direction Goals finding
    /// this field exists to prevent for a metric that is actually
    /// lower-is-better -- a missing value here is a finding
    /// (`UnknownBetter`), not a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub better: Option<String>,
}


impl Scope {
    fn validate_dependencies(&self) -> Result<()> {
        let declared: std::collections::BTreeSet<String> = self
            .declared_agents()
            .into_iter()
            .map(|agent| agent.name())
            .collect();
        self.dependencies.validate_services(&self.name, &declared)
    }

    /// The adapter a task in this scope runs on unless it says otherwise.
    pub fn agent_adapter(&self) -> Option<&str> {
        self.agent.as_ref().map(AgentRef::adapter)
    }

    /// Every agent this scope declares, plus the foreman the instance adds.
    /// Discovery admits only explicitly configured scope directories.
    pub fn agents_with(&self, foreman: &ForemanConfig) -> Vec<ScopeAgent> {
        factory_agents::roster::agents_with(&self.name, self.agent.as_ref(), &self.agents, foreman)
    }

    pub fn standing_agents_with(&self, foreman: &ForemanConfig) -> Vec<ScopeAgent> {
        self.agents_with(foreman)
            .into_iter()
            .filter(|a| a.lifetime.is_standing())
            .collect()
    }

    /// Every agent this scope declares, from either spelling, in the order a
    /// person wrote them.
    pub fn declared_agents(&self) -> Vec<ScopeAgent> {
        factory_agents::roster::declared_agents(self.agent.as_ref(), &self.agents)
    }

    /// Only the ones meant to exist between tasks.
    pub fn standing_agents(&self) -> Vec<ScopeAgent> {
        self.declared_agents()
            .into_iter()
            .filter(|a| a.lifetime.is_standing())
            .collect()
    }
}

/// A loaded instance: the config plus the root it was found under.
#[derive(Debug, Clone)]
pub struct Factory {
    pub root: PathBuf,
    pub config: Config,
}

impl Factory {
    /// Walk up from `start` looking for the config that owns an instance. A
    /// nested scope has the same marker path, so scope-only files are passed
    /// over until a document with `instance:` is found.
    pub fn discover(start: &Path) -> Result<Option<PathBuf>> {
        let mut dir = if start.is_absolute() {
            start.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|e| FactoryError::Other(e.into()))?
                .join(start)
        };
        loop {
            let config = dir.join(FACTORY_DIR).join(CONFIG_FILE);
            if config.is_file() {
                let text = std::fs::read_to_string(&config).map_err(|e| {
                    FactoryError::Other(anyhow::anyhow!("reading {}: {e}", config.display()))
                })?;
                match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&text) {
                    Ok(serde_yaml_ng::Value::Mapping(values))
                        if values.contains_key(serde_yaml_ng::Value::String("instance".into())) =>
                    {
                        return Ok(Some(dir));
                    }
                    // Let `load` report a malformed nearby marker rather than
                    // silently hiding it behind an instance farther up.
                    Err(_) => return Ok(Some(dir)),
                    _ => {}
                }
            }
            if !dir.pop() {
                return Ok(None);
            }
        }
    }

    pub fn load(root: &Path) -> Result<Self> {
        let path = root.join(FACTORY_DIR).join(CONFIG_FILE);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("reading {}: {e}", path.display())))?;
        let config: Config = serde_yaml_ng::from_str(&text)
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("parsing {}: {e}", path.display())))?;
        config.validate_instance()?;
        Ok(Self {
            root: root.to_path_buf(),
            config,
        })
    }

    pub fn factory_dir(&self) -> PathBuf {
        self.root.join(FACTORY_DIR)
    }

    pub fn plugins_dir(&self) -> PathBuf {
        match &self.config.plugins_dir {
            Some(p) if p.is_absolute() => p.clone(),
            Some(p) => self.factory_dir().join(p),
            None => self.factory_dir().join(PLUGINS_DIR),
        }
    }

    pub fn database_path(&self) -> PathBuf {
        self.factory_dir().join("factory.sqlite")
    }

    /// Where a run's own worktree lives, when its task asked for one. Under
    /// `.factory/`, never inside a scope -- one subdirectory per run, named
    /// after the run once it exists, so a retry never inherits the dirty tree
    /// a failed attempt left behind.
    pub fn worktrees_dir(&self) -> PathBuf {
        self.factory_dir().join("worktrees")
    }

    /// The knowledge vault: `.factory/knowledge/`. Unlike everything else
    /// under `.factory/`, this one is not daemon-owned -- it holds authored
    /// pages and documents nothing in Factory regenerates, and is the one
    /// directory here worth backing up. See `knowledge::vault_root`, which
    /// this agrees with.
    pub fn knowledge_dir(&self) -> PathBuf {
        self.factory_dir().join("knowledge")
    }

    /// Where dataset files live: `<root>/.factory/datasets/<name>.yaml`.
    /// Authored content, like the knowledge vault -- not daemon-owned state,
    /// even though it sits under `.factory/` -- so nothing here ever deletes
    /// the directory itself, only the one file a `dataset rm` names.
    pub fn datasets_dir(&self) -> PathBuf {
        self.factory_dir().join("datasets")
    }

    /// Where policy catalogues live: `<root>/.factory/policies/<framework>.yaml`,
    /// the third authored-content directory alongside `knowledge_dir` and
    /// `datasets_dir`. Delegates to `policy::policies_dir` rather than
    /// duplicating the path it already defines.
    pub fn policies_dir(&self) -> PathBuf {
        factory_direction::policy::policies_dir(&self.root)
    }

    /// Where the guide to Factory itself lives, for a harness whose
    /// system-prompt mechanism wants a file rather than inline text. Under
    /// `.factory/`, never inside a scope -- a scope owns only its own
    /// `config.yaml`, and this is daemon state like everything else here.
    pub fn guides_dir(&self) -> PathBuf {
        self.factory_dir().join("guides")
    }

    /// The control socket.
    ///
    /// It belongs in `.factory/`, but a unix socket path has to fit in
    /// `sockaddr_un.sun_path` -- 104 bytes on macOS, 108 on Linux -- and a
    /// deep instance root blows that limit. When it will not fit, fall back to
    /// a short name in the temporary directory derived from the root, so the
    /// daemon and the CLI compute the same path without either reading state
    /// the other wrote.
    pub fn socket_path(&self) -> PathBuf {
        let natural = self.factory_dir().join("factory.sock");
        if natural.as_os_str().len() < 100 {
            return natural;
        }
        let tmp = std::env::temp_dir();
        tmp.join(format!("factory-{}.sock", short_hash(&self.root)))
    }

    /// Resolve a whole authored scope subtree by paths, never name prefixes.
    /// Shared by every level; policy is a consumer, not the owner of ancestry.
    pub fn subtree_scopes(&self, scope: Option<&str>) -> Result<(Option<Scope>, Vec<Scope>)> {
        let (asked, scopes) = factory_kernel::scope_subtree(&self.config.scopes, scope)?;
        Ok((asked.cloned(), scopes.into_iter().cloned().collect()))
    }

    /// Every scope discovered from its own local configuration.
    pub fn scope_names(&self) -> Vec<String> {
        self.config.scopes.iter().map(|s| s.name.clone()).collect()
    }

    /// Look a scope up by its configured name, then by the path-shaped names
    /// older tasks may still carry. Discovery makes configured names unique.
    ///
    /// Older tasks may carry either the relative path used by directory
    /// discovery or the leaf name used before that. Exact relative paths are
    /// accepted; a leaf resolves only when one scope matches. More than one is
    /// refused rather than guessed at.
    pub fn scope(&self, name: &str) -> Result<&Scope> {
        factory_kernel::resolve_scope(&self.config.scopes, name)
    }

    /// Plain identities for an isolated level's live fact read.
    pub fn scope_tree(&self) -> factory_kernel::ScopeTree {
        factory_kernel::ScopeTree {
            scopes: self
                .config
                .scopes
                .iter()
                .map(|scope| factory_kernel::ScopeNode {
                    name: scope.name.clone(),
                    path: scope.path.clone(),
                })
                .collect(),
        }
    }

    /// `name`'s canonical identity, for joining data written under the name a
    /// scope used to have -- a task's `scope` field, a run's liveness --
    /// against `Scope.name` as it reads today. Falls back to `name` itself
    /// when nothing resolves it at all, so a scope that is genuinely gone
    /// still groups its old data under the name it was last known by instead
    /// of losing it to a join that silently matches nothing.
    pub fn canonical_scope_name(&self, name: &str) -> String {
        self.scope(name)
            .map(|s| s.name.clone())
            .unwrap_or_else(|_| name.to_string())
    }

    /// Every role in effect in the scope a name resolves to. A name that
    /// resolves to no scope -- a caller whose scope has since gone -- gets
    /// only what holds everywhere, never a guess at some other scope's roles.
    pub fn roles_for(&self, scope: &str) -> Result<Roles> {
        match self.scope(scope) {
            Ok(found) => self.config.roles_for_scope(found),
            Err(_) => self.config.roles(),
        }
    }

    /// The resolved dashboard for `scope` -- its tiles (`None` for "use the
    /// built-in default") and the name of the scope whose block answered
    /// (`None` for the same built-in-default case, never a magic string --
    /// see `Config::dashboard`'s own doc comment) -- or for the instance
    /// root itself when `scope` is `None`.
    ///
    /// Deliberately **not** `roles_for`'s fallback: a caller asking for a
    /// role is asking "what may I do", and a scope that has since vanished
    /// still needs an answer (the built-in roles) rather than a hard
    /// failure. A caller asking for a dashboard is asking "show me this
    /// named place", and a name that resolves to nothing is refused outright
    /// -- `Factory::scope`'s own `NoSuchScope`, propagated by `?` -- the same
    /// way any other scope-addressed read (`Request::PolicyControl`,
    /// `Request::TaskCreate`'s scope check) refuses a scope nothing
    /// configures, rather than silently substituting the root's own.
    pub fn dashboard_for(
        &self,
        scope: Option<&str>,
    ) -> Result<(Option<DashboardConfig>, Option<String>)> {
        match scope {
            None => {
                let (dashboard, source) = self.config.dashboard();
                Ok((dashboard.cloned(), source))
            }
            Some(name) => {
                let found = self.scope(name)?;
                let (dashboard, source) = self.config.dashboard_for_scope(found);
                Ok((dashboard.cloned(), source))
            }
        }
    }

    /// Every policy layer that applies to the scope a name resolves to, root
    /// first. A name that resolves to no scope -- a caller whose scope has
    /// since gone -- gets only the root layer, the same fallback `roles_for`
    /// makes for roles. Nothing here can fail the way `Roles::resolve` can
    /// (a policy layer is only ever folded by `policy::applicable`, never
    /// validated against a catalogue here), so unlike `roles_for` this
    /// returns the chain directly rather than a `Result`.
    pub fn policy_chain(&self, scope: &str) -> Vec<PolicyLayer> {
        match self.scope(scope) {
            Ok(found) => self.config.policy_chain_for_scope(found),
            Err(_) => self.config.root_policy_layer().into_iter().collect(),
        }
    }

    /// Every quality layer that applies to the scope a name resolves to,
    /// root first -- `policy_chain`'s fallback exactly: a name that resolves
    /// to no scope gets only the root's layer.
    pub fn quality_chain(&self, scope: &str) -> Vec<QualityLayer> {
        match self.scope(scope) {
            Ok(found) => self.config.quality_chain_for_scope(found),
            Err(_) => self.config.root_quality_layer().into_iter().collect(),
        }
    }

    /// Where quality profiles live: `<root>/.factory/quality/<profile>.yaml`
    /// -- authored content like `policies_dir`, delegating to
    /// `quality::quality_dir` for the same reason.
    pub fn quality_dir(&self) -> PathBuf {
        factory_assurance::quality::quality_dir(&self.root)
    }

    /// Every definition-of-ready layer that applies to the scope a name
    /// resolves to, root first -- `quality_chain`'s fallback exactly: a
    /// name that resolves to no scope gets only the instance root's own
    /// implicit layer.
    pub fn intake_chain(&self, scope: &str) -> Vec<IntakeLayer> {
        match self.scope(scope) {
            Ok(found) => self.config.intake_chain_for_scope(found),
            Err(_) => vec![self.config.root_intake_layer()],
        }
    }

    /// Where definitions of ready live: `<root>/.factory/intake/<name>.yaml`
    /// -- authored content like `quality_dir`, delegating to
    /// `ready::ready_dir` for the same reason.
    pub fn intake_dir(&self) -> PathBuf {
        factory_process::ready::ready_dir(&self.root)
    }

    /// The absolute working directory for a scope.
    pub fn scope_path(&self, name: &str) -> Result<PathBuf> {
        let scope = self.scope(name)?;
        Ok(if scope.path.is_absolute() {
            scope.path.clone()
        } else {
            self.root.join(&scope.path)
        })
    }

    /// The task-store adapter a scope's tasks live in. A scope that does not
    /// name one uses the instance default.
    pub fn task_store_for(&self, scope: &str) -> &str {
        self.config
            .scopes
            .iter()
            .find(|s| s.name == scope)
            .and_then(|s| s.task_store.as_deref())
            .unwrap_or(&self.config.daemon.task_store)
    }

    /// Every distinct task-store adapter this instance uses: the default,
    /// plus whatever the scopes name. In a stable order, default first.
    pub fn task_stores(&self) -> Vec<String> {
        let mut out = vec![self.config.daemon.task_store.clone()];
        for scope in &self.config.scopes {
            if let Some(store) = &scope.task_store {
                if !out.contains(store) {
                    out.push(store.clone());
                }
            }
        }
        out
    }
}

/// FNV-1a. Small, and -- unlike `DefaultHasher` -- defined to stay the same
/// from one release to the next, which is what makes both sides agree.
fn short_hash(path: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in path.as_os_str().as_encoded_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn factory(root: &str) -> Factory {
        Factory {
            root: PathBuf::from(root),
            config: Config {
                version: 1,
                instance: Instance {
                    id: "i".into(),
                    name: "n".into(),
                },
                daemon: DaemonConfig::default(),
                scope: None,
                scopes: vec![],
                roles: BTreeMap::new(),
                dashboard: None,
                policies: PolicyDeclaration::default(),
                quality: Vec::new(),
                infrastructure: Infrastructure::default(),
                secrets: Vec::new(),
                plugins_dir: None,
                renewals: Vec::new(),
                renewals_notify: None,
            },
        }
    }

    /// A factory rooted at `/inst`, carrying exactly the scopes given --
    /// for the resolution tests below, where the root itself never matters.
    fn factory_with(scopes: Vec<Scope>) -> Factory {
        let mut f = factory("/inst");
        f.config.scopes = scopes;
        f
    }

    fn scope_at(name: &str, path: &str) -> Scope {
        let mut scope: Scope =
            serde_yaml_ng::from_str(&format!("id: {name}-id\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// Locks `Duration`'s wire form inside a real config struct, not just
    /// the type on its own: moving it into `factory-kernel` must not change
    /// so much as a byte of what `max_age: 30d` looks like on disk (#193,
    /// phase 1, F7). See `factory_kernel::duration::tests` for the JSON side
    /// of the same lock.
    #[test]
    fn a_dependencies_max_age_round_trips_through_yaml_byte_identically() {
        let cfg = DependenciesConfig {
            scan_workflow: None,
            max_age: Some("30d".parse().unwrap()),
            services: Vec::new(),
        };
        let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
        assert_eq!(
            yaml, "max_age: 30d\n",
            "serialized form must stay byte-identical"
        );
        let back: DependenciesConfig = serde_yaml_ng::from_str(&yaml).unwrap();
        assert_eq!(back.max_age, cfg.max_age);
    }

    #[test]
    fn a_foreman_is_added_to_every_scope_but_the_excluded_ones() {
        let s: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        let root: Scope = serde_yaml_ng::from_str("name: root\npath: .\n").unwrap();
        let cfg = ForemanConfig {
            enabled: true,
            ..Default::default()
        };
        let names: Vec<String> = s.agents_with(&cfg).iter().map(|a| a.name()).collect();
        assert_eq!(names, vec!["foreman"]);
        assert_eq!(s.agents_with(&cfg)[0].role, Role::foreman());
        assert!(s.agents_with(&cfg)[0].lifetime.is_standing());
        assert!(
            root.agents_with(&cfg).is_empty(),
            "root is excluded by default"
        );
        assert!(
            s.agents_with(&ForemanConfig::default()).is_empty(),
            "off by default"
        );
    }

    #[test]
    fn a_scope_that_names_its_own_foreman_keeps_it() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: demo\npath: .\nagents:\n  - name: chef\n    harness: pi\n    lifetime: permanent\n    role: foreman\n",
        )
        .unwrap();
        let cfg = ForemanConfig {
            enabled: true,
            ..Default::default()
        };
        let names: Vec<String> = s.agents_with(&cfg).iter().map(|a| a.name()).collect();
        assert_eq!(names, vec!["chef"], "no second foreman is bolted on");
    }

    #[test]
    fn foreman_exclude_matches_a_bare_name_by_its_last_segment() {
        // A pre-migration `exclude: ["demo"]` must keep meaning the same
        // directory once that scope's identity becomes `projects/demo`.
        let s: Scope =
            serde_yaml_ng::from_str("name: projects/demo\npath: projects/demo\n").unwrap();
        let cfg = ForemanConfig {
            enabled: true,
            exclude: vec!["demo".into()],
            ..Default::default()
        };
        assert!(
            s.agents_with(&cfg).is_empty(),
            "the bare exclude entry still reaches it"
        );

        // A full path in `exclude` is not loosened the same way -- it means
        // exactly the scope it names.
        let other: Scope =
            serde_yaml_ng::from_str("name: projects/other\npath: projects/other\n").unwrap();
        let cfg = ForemanConfig {
            enabled: true,
            exclude: vec!["projects/demo".into()],
            ..Default::default()
        };
        assert!(
            !other.agents_with(&cfg).is_empty(),
            "a path-shaped exclude does not match a sibling"
        );
    }

    #[test]
    fn a_bare_name_resolves_to_the_scope_whose_last_segment_matches() {
        let f = factory_with(vec![
            scope_at("projects/factory", "projects/factory"),
            scope_at("projects/other", "projects/other"),
        ]);
        // No scope is literally named "factory" any more -- only the last
        // segment of one matches -- and the fallback still finds it.
        assert_eq!(f.scope("factory").unwrap().name, "projects/factory");
        // The full identity keeps working too.
        assert_eq!(f.scope("projects/other").unwrap().name, "projects/other");
    }

    #[test]
    fn an_old_path_name_resolves_after_the_scope_gets_a_configured_name() {
        let f = factory_with(vec![scope_at("demo-project", "projects/demo")]);
        assert_eq!(f.scope("projects/demo").unwrap().name, "demo-project");
        assert_eq!(f.scope("demo").unwrap().name, "demo-project");
    }

    #[test]
    fn an_ambiguous_bare_name_is_refused_with_the_candidates_named() {
        let f = factory_with(vec![
            scope_at("projects/a/src", "projects/a/src"),
            scope_at("projects/b/src", "projects/b/src"),
        ]);
        let e = f.scope("src").unwrap_err().to_string();
        assert!(e.contains("projects/a/src"), "{e}");
        assert!(e.contains("projects/b/src"), "{e}");
    }

    #[test]
    fn canonical_scope_name_normalizes_a_legacy_bare_name_and_keeps_an_unknown_one() {
        let f = factory_with(vec![scope_at("projects/factory", "projects/factory")]);
        assert_eq!(f.canonical_scope_name("factory"), "projects/factory");
        assert_eq!(
            f.canonical_scope_name("gone"),
            "gone",
            "a scope that resolves to nothing keeps its old data grouped under the name it had"
        );
    }

    fn config_with(yaml: &str) -> Config {
        serde_yaml_ng::from_str(&format!("instance:\n  id: i\n  name: n\n{yaml}")).unwrap()
    }

    #[test]
    fn a_role_nothing_defines_is_refused_with_the_agent_named() {
        let c = config_with(
            "scopes:\n  - name: demo\n    path: .\n    agents:\n      - name: watcher\n        harness: pi\n        role: reviewer\n",
        );
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("watcher"), "{e}");
        assert!(e.contains("reviewer"), "{e}");
        assert!(e.contains("worker"), "it says what there is instead: {e}");
    }

    #[test]
    fn a_role_the_instance_named_is_accepted() {
        let c = config_with(
            "roles:\n  reviewer:\n    grants: [task.report]\nscopes:\n  - name: demo\n    path: .\n    agents:\n      - name: watcher\n        harness: pi\n        role: reviewer\n",
        );
        c.validate().unwrap();
        assert!(c.roles().unwrap().contains(&Role::new("reviewer")));
    }

    /// A scope at `path` named `name`, declaring `roles` and one agent holding
    /// `holds`, if given. Names and paths deliberately differ in the tests
    /// below: inheritance must follow the path, never the name.
    fn scope_with_roles(name: &str, path: &str, roles: &str, holds: Option<&str>) -> Scope {
        let mut yaml = format!("id: {name}-id\nname: {name}\n");
        if !roles.is_empty() {
            yaml.push_str(&format!("roles:\n{roles}"));
        }
        if let Some(role) = holds {
            yaml.push_str(&format!(
                "agents:\n  - name: critic\n    harness: pi\n    role: {role}\n"
            ));
        }
        let mut scope: Scope = serde_yaml_ng::from_str(&yaml).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    fn tree() -> Config {
        let mut c = config_with(
            "roles:\n  runner:\n    grants: [task.run, task.cancel]\n    reach: scope\n",
        );
        c.scopes = vec![
            scope_with_roles("company", ".", "", None),
            // Named nothing like its path, and nothing like its children's.
            scope_with_roles(
                "engineering",
                "projects",
                "  reviewer:\n    describe: works its own tasks\n    grants: [task.edit, task.report]\n    reach: own\n",
                None,
            ),
            scope_with_roles(
                "demo-app",
                "projects/demo",
                "  reviewer:\n    describe: reviews, and may open follow-ups\n    grants: [task.create, task.edit, task.report]\n    reach: own\n",
                None,
            ),
            scope_with_roles("engineering/tools", "projects/tools", "", None),
            // A name that reads as a child of `engineering` but is not below
            // `projects` on disk, and a path that shares a prefix string.
            scope_with_roles("engineering/other", "elsewhere", "", None),
            scope_with_roles("lookalike", "projects-x", "", None),
        ];
        c
    }

    fn scope_named<'a>(c: &'a Config, name: &str) -> &'a Scope {
        c.scopes.iter().find(|s| s.name == name).unwrap()
    }

    #[test]
    fn roles_inherit_down_the_path_tree_and_never_up_or_sideways() {
        let c = tree();
        let reviewer = Role::new("reviewer");

        let tools = c
            .roles_for_scope(scope_named(&c, "engineering/tools"))
            .unwrap();
        let inherited = tools
            .entry(&reviewer)
            .expect("projects/tools is below projects");
        assert_eq!(
            inherited.origin,
            RoleOrigin::Scope {
                scope: "engineering".into()
            }
        );
        assert!(
            tools.contains(&Role::new("runner")),
            "instance roles hold everywhere"
        );

        for elsewhere in ["company", "engineering/other", "lookalike"] {
            let roles = c.roles_for_scope(scope_named(&c, elsewhere)).unwrap();
            assert!(
                !roles.contains(&reviewer),
                "{elsewhere} is not below projects by path, whatever its name says"
            );
        }
    }

    #[test]
    fn a_child_override_replaces_the_inherited_definition_whole() {
        let c = tree();
        let roles = c.roles_for_scope(scope_named(&c, "demo-app")).unwrap();
        let entry = roles.entry(&Role::new("reviewer")).unwrap();
        assert_eq!(
            entry.origin,
            RoleOrigin::Scope {
                scope: "demo-app".into()
            }
        );
        assert_eq!(
            entry.overrides,
            Some(RoleOrigin::Scope {
                scope: "engineering".into()
            })
        );
        assert_eq!(entry.def.describe, "reviews, and may open follow-ups");
        assert!(entry.def.allows(factory_agents::role::Grant::TaskCreate));
    }

    #[test]
    fn a_scope_may_not_redefine_a_preset_and_the_error_names_it() {
        let mut c = tree();
        c.scopes.push(scope_with_roles(
            "sneaky",
            "projects/sneaky",
            "  worker:\n    grants: ['*']\n",
            None,
        ));
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("worker"), "{e}");
        assert!(e.contains("sneaky"), "{e}");
    }

    #[test]
    fn an_agent_may_hold_an_inherited_role_but_not_one_from_elsewhere() {
        let mut c = tree();
        c.scopes.push(scope_with_roles(
            "inner",
            "projects/demo/inner",
            "",
            Some("reviewer"),
        ));
        c.validate().unwrap();

        c.scopes.push(scope_with_roles(
            "outsider",
            "elsewhere/team",
            "",
            Some("reviewer"),
        ));
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("outsider"), "{e}");
        assert!(e.contains("reviewer"), "{e}");
        assert!(
            e.contains("runner"),
            "it lists what that scope does have: {e}"
        );
    }

    #[test]
    fn the_root_config_refuses_scope_roles_and_points_at_its_own_roles() {
        let c = config_with(
            "scope:\n  name: company\n  roles:\n    runner:\n      grants: [task.run]\n",
        );
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(e.contains("scope.roles"), "{e}");
        assert!(e.contains("top-level `roles:`"), "{e}");
    }

    #[test]
    fn a_top_level_roles_block_in_a_nested_scope_file_is_refused() {
        let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "scope:\n  id: s\n  name: demo\nroles:\n  reviewer:\n    grants: [task.report]\n",
        )
        .unwrap();
        let e = refuse_misplaced_scope_roles(
            &document,
            Path::new("/x/projects/demo/.factory/config.yaml"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("scope.roles"), "{e}");
        assert!(e.contains("projects/demo"), "{e}");

        let fine: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("scope:\n  id: s\n  name: demo\n  roles: {}\n").unwrap();
        refuse_misplaced_scope_roles(&fine, Path::new("x")).unwrap();
    }

    // -- dashboard_for_scope --------------------------------------------------

    /// A scope at `path` named `name`, declaring a `dashboard:` block inline
    /// -- mirrors `scope_with_roles` exactly, same reason: inheritance must
    /// follow the path, never the name.
    fn scope_with_dashboard(name: &str, path: &str, dashboard: &str) -> Scope {
        let mut yaml = format!("id: {name}-id\nname: {name}\n");
        if !dashboard.is_empty() {
            yaml.push_str(&format!("dashboard:\n{dashboard}"));
        }
        let mut scope: Scope = serde_yaml_ng::from_str(&yaml).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// The same topology `tree()` uses for roles, with dashboards instead:
    /// `company` at the root, `engineering` (path `projects`) overriding,
    /// `demo-app` (path `projects/demo`, below `engineering`) overriding
    /// again, `engineering/tools` (path `projects/tools`, below
    /// `engineering`, inheriting), `engineering/other` (path `elsewhere`,
    /// named like a child of `engineering` but not below it on disk) and
    /// `lookalike` (path `projects-x`, a path that shares a string prefix
    /// with `projects` but is not the same or a child segment) both outside
    /// every override.
    fn dashboard_tree() -> Config {
        let mut c =
            config_with("dashboard:\n  tiles:\n    - { metric: throughput_week, size: s }\n");
        c.scopes = vec![
            scope_with_dashboard("company", ".", ""),
            scope_with_dashboard(
                "engineering",
                "projects",
                "  tiles:\n    - { metric: first_pass_yield, size: m }\n",
            ),
            scope_with_dashboard(
                "demo-app",
                "projects/demo",
                "  tiles:\n    - { view: throughput, size: l }\n    - { view: kpis, size: s }\n",
            ),
            scope_with_dashboard("engineering/tools", "projects/tools", ""),
            scope_with_dashboard("engineering/other", "elsewhere", ""),
            scope_with_dashboard("lookalike", "projects-x", ""),
        ];
        c
    }

    #[test]
    fn dashboard_parses_at_root_and_scope_level() {
        let c = dashboard_tree();
        let root_tiles = &c
            .dashboard
            .as_ref()
            .expect("root declared a dashboard")
            .tiles;
        assert_eq!(root_tiles.len(), 1);
        assert_eq!(
            root_tiles[0].metric.as_ref().unwrap().as_str(),
            "throughput_week"
        );

        let engineering = scope_named(&c, "engineering");
        let tiles = &engineering
            .dashboard
            .as_ref()
            .expect("engineering overrides")
            .tiles;
        assert_eq!(tiles.len(), 1);
        assert_eq!(
            tiles[0].metric.as_ref().unwrap().as_str(),
            "first_pass_yield"
        );
    }

    #[test]
    fn a_dashboard_override_reaches_descendants_but_not_siblings_or_ancestors() {
        let c = dashboard_tree();

        let (tiles, source) = c.dashboard_for_scope(scope_named(&c, "engineering/tools"));
        assert_eq!(
            source,
            Some("engineering".to_string()),
            "projects/tools is below projects on disk"
        );
        assert_eq!(
            tiles.unwrap().tiles[0].metric.as_ref().unwrap().as_str(),
            "first_pass_yield"
        );

        for elsewhere in ["company", "engineering/other", "lookalike"] {
            let (tiles, source) = c.dashboard_for_scope(scope_named(&c, elsewhere));
            assert_eq!(
                source,
                Some("n".to_string()),
                "{elsewhere} is not below projects by path, whatever its name says -- falls \
                 through to the instance root's own block instead of engineering's; \"n\" is \
                 config_with's own instance name, the root's fallback when it names no scope"
            );
            assert_eq!(
                tiles.unwrap().tiles[0].metric.as_ref().unwrap().as_str(),
                "throughput_week"
            );
        }
    }

    #[test]
    fn the_nearest_dashboard_wins_whole_never_merged_with_an_ancestors() {
        let c = dashboard_tree();
        let (tiles, source) = c.dashboard_for_scope(scope_named(&c, "demo-app"));
        assert_eq!(source, Some("demo-app".to_string()));
        let tiles = &tiles.unwrap().tiles;
        assert_eq!(
            tiles.len(),
            2,
            "demo-app's own two tiles, not folded with engineering's one"
        );
        assert!(
            tiles.iter().all(|t| t.metric.is_none()),
            "demo-app names only view tiles"
        );
    }

    #[test]
    fn an_empty_tile_list_override_fails_to_load_naming_the_scope() {
        let mut c = tree();
        c.scopes.push(scope_with_dashboard(
            "empty-override",
            "projects/empty",
            "  tiles: []\n",
        ));
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("empty-override"), "{e}");
        assert!(e.contains("at least one tile"), "{e}");
        assert!(
            e.contains("remove the `dashboard:` block to inherit"),
            "{e}"
        );
    }

    #[test]
    fn absent_dashboard_everywhere_resolves_to_the_built_in_default() {
        let c = tree(); // the roles fixture: no `dashboard:` block anywhere
        let (tiles, source) = c.dashboard_for_scope(scope_named(&c, "demo-app"));
        assert!(tiles.is_none());
        assert_eq!(source, None);

        let (tiles, source) = c.dashboard();
        assert!(tiles.is_none());
        assert_eq!(source, None);
    }

    #[test]
    fn an_unknown_metric_fails_load_naming_the_root_dashboard_path_and_reason() {
        let c = config_with("dashboard:\n  tiles:\n    - { metric: not_a_real_metric, size: s }\n");
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("dashboard.tiles[0].metric"), "{e}");
        assert!(e.contains("not_a_real_metric"), "{e}");
    }

    #[test]
    fn a_tile_naming_both_metric_and_view_fails_load() {
        let c = config_with(
            "dashboard:\n  tiles:\n    - { metric: throughput_week, view: kpis, size: s }\n",
        );
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("dashboard.tiles[0]"), "{e}");
        assert!(e.contains("exactly one"), "{e}");
    }

    #[test]
    fn a_tile_naming_neither_metric_nor_view_fails_load() {
        let c = config_with("dashboard:\n  tiles:\n    - { size: s }\n");
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("dashboard.tiles[0]"), "{e}");
        assert!(e.contains("exactly one"), "{e}");
    }

    #[test]
    fn a_bound_metric_family_loads_cleanly_but_an_empty_tile_list_does_not() {
        config_with("dashboard:\n  tiles:\n    - { metric: compliance.cra, size: s }\n")
            .validate()
            .unwrap();
        let e = config_with("dashboard:\n  tiles: []\n")
            .validate()
            .unwrap_err()
            .to_string();
        assert!(e.contains("dashboard needs at least one tile"), "{e}");
    }

    #[test]
    fn an_invalid_tile_in_a_nested_scope_names_that_scope_and_fails_load() {
        let mut c = tree();
        c.scopes.push(scope_with_dashboard(
            "bad-scope",
            "projects/bad",
            "  tiles:\n    - { metric: nope, size: s }\n",
        ));
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("bad-scope"), "{e}");
        assert!(e.contains("dashboard.tiles[0].metric"), "{e}");
        assert!(e.contains("nope"), "{e}");
    }

    #[test]
    fn an_unknown_size_or_view_is_refused_at_parse_time_as_a_closed_enum() {
        let e = serde_yaml_ng::from_str::<Config>(
            "instance:\n  id: i\n  name: n\ndashboard:\n  tiles:\n    - { metric: throughput_week, size: xxl }\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("xxl"), "{e}");

        let e = serde_yaml_ng::from_str::<Config>(
            "instance:\n  id: i\n  name: n\ndashboard:\n  tiles:\n    - { view: nonexistent_view, size: s }\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("nonexistent_view"), "{e}");
    }

    #[test]
    fn the_root_config_refuses_scope_dashboard_and_points_at_its_own_dashboard() {
        let c = config_with(
            "scope:\n  name: company\n  dashboard:\n    tiles:\n      - { view: kpis, size: s }\n",
        );
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(e.contains("scope.dashboard"), "{e}");
        assert!(e.contains("top-level `dashboard:`"), "{e}");
    }

    #[test]
    fn a_top_level_dashboard_block_in_a_nested_scope_file_is_refused() {
        let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "scope:\n  id: s\n  name: demo\ndashboard:\n  tiles:\n    - { view: kpis, size: s }\n",
        )
        .unwrap();
        let e = refuse_misplaced_scope_dashboard(
            &document,
            Path::new("/x/projects/demo/.factory/config.yaml"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("scope.dashboard"), "{e}");
        assert!(e.contains("projects/demo"), "{e}");

        let fine: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "scope:\n  id: s\n  name: demo\n  dashboard:\n    tiles:\n      - { view: kpis, size: s }\n",
        )
        .unwrap();
        refuse_misplaced_scope_dashboard(&fine, Path::new("x")).unwrap();
    }

    #[test]
    fn factory_dashboard_for_an_unknown_scope_is_refused_unlike_roles_for() {
        let f = factory_with(vec![scope_at("demo", ".")]);
        let e = f.dashboard_for(Some("nope")).unwrap_err().to_string();
        assert!(e.contains("nope"), "{e}");
        assert!(
            f.roles_for("nope").is_ok(),
            "roles_for falls back to the built-in roles for a scope that has since gone; dashboard_for does not"
        );
    }

    #[test]
    fn factory_dashboard_for_none_resolves_the_instance_roots_own() {
        let mut f = factory("/inst");
        f.config.dashboard =
            Some(serde_yaml_ng::from_str("tiles:\n  - { view: kpis, size: s }\n").unwrap());
        let (tiles, source) = f.dashboard_for(None).unwrap();
        // "n" is `factory`'s own instance name -- the root's fallback name
        // when it declares no `scope:` of its own (`Config::dashboard`).
        assert_eq!(source, Some("n".to_string()));
        assert_eq!(tiles.unwrap().tiles.len(), 1);
    }

    // -- policy_chain_for_scope ---------------------------------------------

    /// A scope at `path` named `name`, declaring `policies` inline, mirroring
    /// `scope_with_roles` above -- names and paths deliberately differ, since
    /// inheritance must follow the path, never the name.
    fn scope_with_policies(name: &str, path: &str, policies: &str) -> Scope {
        let mut yaml = format!("id: {name}-id\nname: {name}\n");
        if !policies.is_empty() {
            yaml.push_str(&format!("policies:\n{policies}"));
        }
        let mut scope: Scope = serde_yaml_ng::from_str(&yaml).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// The same shape as `tree()`, for policies instead of roles: the root
    /// commits to `cra`, `engineering` (`projects`) adds `gdpr`, and
    /// `demo-app` (`projects/demo`) tightens one of `cra`'s controls -- plus
    /// the same lookalike and sideways scopes `tree()` uses to prove
    /// inheritance follows the path and nothing else.
    fn policy_tree() -> Config {
        let mut c = config_with("policies:\n  frameworks: [cra]\n");
        c.scopes = vec![
            scope_with_policies("company", ".", ""),
            scope_with_policies("engineering", "projects", "  frameworks: [gdpr]\n"),
            scope_with_policies(
                "demo-app",
                "projects/demo",
                "  tighten:\n    cra/annex-i-2-1:\n      max_age: 7d\n",
            ),
            scope_with_policies("engineering/tools", "projects/tools", ""),
            // A name that reads as a child of `engineering` but is not below
            // `projects` on disk, and a path that shares a prefix string.
            scope_with_policies("engineering/other", "elsewhere", ""),
            scope_with_policies("lookalike", "projects-x", ""),
        ];
        c
    }

    #[test]
    fn the_policy_chain_is_root_first_then_each_ancestor_down_to_the_scope_itself() {
        let c = policy_tree();
        let chain = c.policy_chain_for_scope(scope_named(&c, "demo-app"));
        let scopes: Vec<&str> = chain.iter().map(|l| l.scope.as_str()).collect();
        // The root layer is named from the instance (`config_with` gives it
        // no `scope:` of its own), then each ancestor by its own scope name.
        assert_eq!(scopes, vec!["n", "engineering", "demo-app"], "{scopes:?}");
        assert_eq!(chain[0].frameworks, vec!["cra".to_string()]);
        assert_eq!(chain[1].frameworks, vec!["gdpr".to_string()]);
        assert!(chain[2]
            .tighten
            .contains_key(&ControlRef::new("cra", "annex-i-2-1")));
    }

    #[test]
    fn a_nested_scope_inherits_ancestors_layers_down_the_path_tree_and_never_up_or_sideways() {
        let c = policy_tree();

        let tools = c.policy_chain_for_scope(scope_named(&c, "engineering/tools"));
        let scopes: Vec<&str> = tools.iter().map(|l| l.scope.as_str()).collect();
        assert_eq!(
            scopes,
            vec!["n", "engineering"],
            "projects/tools is below projects, but not below projects/demo"
        );

        for elsewhere in ["company", "engineering/other", "lookalike"] {
            let chain = c.policy_chain_for_scope(scope_named(&c, elsewhere));
            let scopes: Vec<&str> = chain.iter().map(|l| l.scope.as_str()).collect();
            assert_eq!(
                scopes,
                vec!["n"],
                "{elsewhere} is not below projects by path, whatever its name says"
            );
        }
    }

    #[test]
    fn the_root_layer_is_named_from_its_own_registered_scope_and_never_appears_twice() {
        // The common shape on a real instance: the root opts into being a
        // scope, and discovery then places that same `Scope` (path `.`) in
        // `self.scopes`, where it would otherwise surface as a second,
        // empty "root" ancestor of everything below it.
        let mut c = config_with("policies:\n  frameworks: [cra]\n");
        let root_scope: Scope =
            serde_yaml_ng::from_str("id: root-id\nname: company\npath: .\n").unwrap();
        c.scope = Some(root_scope.clone());
        c.scopes = vec![
            root_scope,
            scope_with_policies("demo", "projects/demo", "  frameworks: [gdpr]\n"),
        ];

        let chain = c.policy_chain_for_scope(scope_named(&c, "demo"));
        let scopes: Vec<&str> = chain.iter().map(|l| l.scope.as_str()).collect();
        assert_eq!(
            chain.len(),
            2,
            "the root's own Scope entry in `self.scopes` contributes nothing beyond the root layer: {scopes:?}"
        );
        assert_eq!(scopes, vec!["company", "demo"]);
        assert_eq!(
            chain[0].frameworks,
            vec!["cra".to_string()],
            "the root layer is named from its configured scope, not the instance name"
        );
    }

    #[test]
    fn factory_policy_chain_matches_config_policy_chain_for_scope_and_falls_back_for_an_unknown_name(
    ) {
        let mut f = factory_with(vec![scope_with_policies(
            "demo",
            "demo",
            "  frameworks: [gdpr]\n",
        )]);
        f.config.policies = PolicyDeclaration {
            frameworks: vec!["cra".to_string()],
            ..Default::default()
        };

        let expected = f.config.policy_chain_for_scope(f.scope("demo").unwrap());
        assert_eq!(f.policy_chain("demo"), expected);

        // A name that resolves to no scope -- a caller whose scope has since
        // gone -- gets only the root layer, the same fallback `roles_for`
        // makes for roles.
        let root_only = f.policy_chain("gone");
        assert_eq!(root_only.len(), 1);
        assert_eq!(root_only[0].frameworks, vec!["cra".to_string()]);
    }

    #[test]
    fn a_populated_policy_declaration_round_trips_through_yaml() {
        // `tighten` is keyed by `ControlRef`, which #75 only ever exercised
        // through JSON and in-code construction -- confirm it also works as
        // a YAML mapping key, exactly as the issue's own example writes it.
        let yaml = "instance:\n  id: i\n  name: n\n\
                     policies:\n  frameworks: [cra, dsgvo]\n  tighten:\n    cra/annex-i-2-1:\n      max_age: 14d\n  \
                     not_applicable:\n    - control: dsgvo/art-37\n      rationale: fewer than 20 people process personal data\n";
        let c: Config = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(
            c.policies.frameworks,
            vec!["cra".to_string(), "dsgvo".to_string()]
        );
        let tighten = c
            .policies
            .tighten
            .get(&ControlRef::new("cra", "annex-i-2-1"))
            .unwrap();
        assert_eq!(tighten.max_age, Some("14d".parse().unwrap()));
        assert_eq!(
            c.policies.not_applicable[0].control,
            ControlRef::new("dsgvo", "art-37")
        );

        let rendered = serde_yaml_ng::to_string(&c).unwrap();
        let reparsed: Config = serde_yaml_ng::from_str(&rendered).unwrap();
        assert_eq!(
            reparsed.policies, c.policies,
            "round-trips through its own rendered YAML"
        );
    }

    #[test]
    fn a_config_and_scope_naming_no_policies_round_trip_with_no_policies_key() {
        let c: Config = serde_yaml_ng::from_str("instance:\n  id: i\n  name: n\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&c).unwrap().contains("policies"));

        let s: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&s).unwrap().contains("policies"));
    }

    #[test]
    fn an_unknown_field_in_a_policy_declaration_is_refused_not_silently_dropped() {
        let yaml = "instance:\n  id: i\n  name: n\npolicies:\n  framework: [cra]\n";
        let err = serde_yaml_ng::from_str::<Config>(yaml).unwrap_err();
        assert!(err.to_string().contains("framework"), "{err}");
    }

    #[test]
    fn the_root_config_refuses_scope_policies_and_points_at_its_own_policies() {
        let c = config_with("scope:\n  name: company\n  policies:\n    frameworks: [cra]\n");
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(e.contains("scope.policies"), "{e}");
        assert!(e.contains("top-level `policies:`"), "{e}");
        assert!(c.validate().is_err(), "validate() calls the same guard");
    }

    #[test]
    fn a_top_level_policies_block_in_a_nested_scope_file_is_refused() {
        let document: serde_yaml_ng::Value = serde_yaml_ng::from_str(
            "scope:\n  id: s\n  name: demo\npolicies:\n  frameworks: [cra]\n",
        )
        .unwrap();
        let e = refuse_misplaced_scope_policies(
            &document,
            Path::new("/x/projects/demo/.factory/config.yaml"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("scope.policies"), "{e}");
        assert!(e.contains("projects/demo"), "{e}");

        let fine: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("scope:\n  id: s\n  name: demo\n  policies: {}\n").unwrap();
        refuse_misplaced_scope_policies(&fine, Path::new("x")).unwrap();
    }

    #[test]
    fn policies_dir_delegates_to_the_policy_modules_path() {
        let f = factory("/inst");
        assert_eq!(
            f.policies_dir(),
            factory_direction::policy::policies_dir(&f.root)
        );
    }

    // -- quality_chain_for_scope ---------------------------------------------

    /// A scope at `path` named `name`, binding `profiles` under
    /// `scope.quality` -- `scope_with_policies`, for quality profiles.
    fn scope_with_quality(name: &str, path: &str, profiles: &str) -> Scope {
        let mut yaml = format!("id: {name}-id\nname: {name}\n");
        if !profiles.is_empty() {
            yaml.push_str(&format!("quality: {profiles}\n"));
        }
        let mut scope: Scope = serde_yaml_ng::from_str(&yaml).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// `policy_tree()`'s shape: the root binds `baseline`, `engineering`
    /// (`projects`) adds `service`, `demo-app` (`projects/demo`) adds
    /// `latency` -- plus the same lookalike and sideways scopes, so the
    /// chain is proven to follow the path and nothing else.
    fn quality_tree() -> Config {
        let mut c = config_with("quality: [baseline]\n");
        c.scopes = vec![
            scope_with_quality("company", ".", ""),
            scope_with_quality("engineering", "projects", "[service]"),
            scope_with_quality("demo-app", "projects/demo", "[latency, service]"),
            scope_with_quality("engineering/tools", "projects/tools", ""),
            scope_with_quality("engineering/other", "elsewhere", ""),
            scope_with_quality("lookalike", "projects-x", ""),
        ];
        c
    }

    fn chain_of(chain: &[QualityLayer]) -> Vec<(String, Vec<String>)> {
        chain
            .iter()
            .map(|l| (l.scope.clone(), l.profiles.clone()))
            .collect()
    }

    #[test]
    fn the_quality_chain_is_root_first_then_each_ancestor_down_to_the_scope_itself() {
        let c = quality_tree();
        let chain = c.quality_chain_for_scope(scope_named(&c, "demo-app"));
        assert_eq!(
            chain_of(&chain),
            vec![
                ("n".to_string(), vec!["baseline".to_string()]),
                ("engineering".to_string(), vec!["service".to_string()]),
                (
                    "demo-app".to_string(),
                    vec!["latency".to_string(), "service".to_string()]
                ),
            ]
        );
    }

    #[test]
    fn a_nested_scope_inherits_quality_down_the_path_tree_and_never_up_or_sideways() {
        let c = quality_tree();
        let tools = c.quality_chain_for_scope(scope_named(&c, "engineering/tools"));
        let scopes: Vec<&str> = tools.iter().map(|l| l.scope.as_str()).collect();
        assert_eq!(scopes, vec!["n", "engineering"]);

        for elsewhere in ["company", "engineering/other", "lookalike"] {
            let chain = c.quality_chain_for_scope(scope_named(&c, elsewhere));
            let scopes: Vec<&str> = chain.iter().map(|l| l.scope.as_str()).collect();
            assert_eq!(
                scopes,
                vec!["n"],
                "{elsewhere} is not below projects by path, whatever its name says"
            );
        }
    }

    #[test]
    fn the_root_quality_layer_is_named_from_its_own_registered_scope_and_never_appears_twice() {
        let mut c = config_with("quality: [baseline]\n");
        let root_scope: Scope =
            serde_yaml_ng::from_str("id: root-id\nname: company\npath: .\n").unwrap();
        c.scope = Some(root_scope.clone());
        c.scopes = vec![
            root_scope,
            scope_with_quality("demo", "projects/demo", "[latency]"),
        ];

        let chain = c.quality_chain_for_scope(scope_named(&c, "demo"));
        assert_eq!(
            chain_of(&chain),
            vec![
                ("company".to_string(), vec!["baseline".to_string()]),
                ("demo".to_string(), vec!["latency".to_string()]),
            ]
        );
    }

    #[test]
    fn factory_quality_chain_matches_config_and_falls_back_to_the_root_for_an_unknown_name() {
        let mut f = factory_with(vec![scope_with_quality("demo", "demo", "[latency]")]);
        f.config.quality = vec!["baseline".to_string()];
        let expected = f.config.quality_chain_for_scope(f.scope("demo").unwrap());
        assert_eq!(f.quality_chain("demo"), expected);
        assert_eq!(
            chain_of(&f.quality_chain("gone")),
            vec![("n".to_string(), vec!["baseline".to_string()])]
        );
    }

    #[test]
    fn a_quality_declaration_round_trips_through_yaml_on_the_root_and_on_a_scope() {
        let c = config_with("quality: [daemon-service, baseline]\n");
        assert_eq!(
            c.quality,
            vec!["daemon-service".to_string(), "baseline".to_string()]
        );
        let reparsed: Config =
            serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&c).unwrap()).unwrap();
        assert_eq!(reparsed.quality, c.quality);

        let s: Scope = serde_yaml_ng::from_str("id: s\nname: demo\nquality: [latency]\n").unwrap();
        let reparsed: Scope =
            serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&s).unwrap()).unwrap();
        assert_eq!(reparsed.quality, vec!["latency".to_string()]);
    }

    #[test]
    fn a_config_and_scope_binding_no_quality_round_trip_with_no_quality_key() {
        let c: Config = serde_yaml_ng::from_str("instance:\n  id: i\n  name: n\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&c).unwrap().contains("quality"));
        let s: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&s).unwrap().contains("quality"));
    }

    #[test]
    fn the_root_config_refuses_scope_quality_and_points_at_its_own_quality() {
        let c = config_with("scope:\n  name: company\n  quality: [baseline]\n");
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(e.contains("scope.quality"), "{e}");
        assert!(e.contains("top-level `quality:`"), "{e}");
        assert!(e.contains("baseline"), "{e}");
        assert!(c.validate().is_err(), "validate() calls the same guard");
    }

    #[test]
    fn a_top_level_quality_block_in_a_nested_scope_file_is_refused() {
        let document: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("scope:\n  id: s\n  name: demo\nquality: [latency]\n").unwrap();
        let e = refuse_misplaced_scope_quality(
            &document,
            Path::new("/x/projects/demo/.factory/config.yaml"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("scope.quality"), "{e}");
        assert!(e.contains("projects/demo"), "{e}");

        let fine: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("scope:\n  id: s\n  name: demo\n  quality: [latency]\n")
                .unwrap();
        refuse_misplaced_scope_quality(&fine, Path::new("x")).unwrap();
    }

    #[test]
    fn quality_dir_delegates_to_the_quality_modules_path() {
        let f = factory("/inst");
        assert_eq!(
            f.quality_dir(),
            factory_assurance::quality::quality_dir(&f.root)
        );
    }

    // -- intake_chain_for_scope (#169) ----------------------------------------

    /// A scope at `path` named `name`, binding `names` under `scope.intake`
    /// -- `scope_with_quality`, for definitions of ready.
    fn scope_with_intake(name: &str, path: &str, names: &str) -> Scope {
        let mut yaml = format!("id: {name}-id\nname: {name}\n");
        if !names.is_empty() {
            yaml.push_str(&format!("intake: {names}\n"));
        }
        let mut scope: Scope = serde_yaml_ng::from_str(&yaml).unwrap();
        scope.path = PathBuf::from(path);
        scope
    }

    /// `quality_tree()`'s shape, for intake: `engineering` (`projects`) adds
    /// `security`, `demo-app` (`projects/demo`) adds `strict` -- plus the
    /// same lookalike and sideways scopes, so the chain is proven to follow
    /// the path and nothing else. The root binds nothing of its own here:
    /// its layer is `ready`, always, whether or not the file exists.
    fn intake_tree() -> Config {
        let mut c = config_with("");
        c.scopes = vec![
            scope_with_intake("company", ".", ""),
            scope_with_intake("engineering", "projects", "[security]"),
            scope_with_intake("demo-app", "projects/demo", "[strict, security]"),
            scope_with_intake("engineering/tools", "projects/tools", ""),
            scope_with_intake("engineering/other", "elsewhere", ""),
            scope_with_intake("lookalike", "projects-x", ""),
        ];
        c
    }

    fn intake_chain_of(chain: &[IntakeLayer]) -> Vec<(String, Vec<String>, bool)> {
        chain
            .iter()
            .map(|l| (l.scope.clone(), l.files.clone(), l.required))
            .collect()
    }

    #[test]
    fn the_intake_chain_is_root_first_then_each_ancestor_down_to_the_scope_itself() {
        let c = intake_tree();
        let chain = c.intake_chain_for_scope(scope_named(&c, "demo-app"));
        assert_eq!(
            intake_chain_of(&chain),
            vec![
                ("n".to_string(), vec!["ready".to_string()], false),
                (
                    "engineering".to_string(),
                    vec!["security".to_string()],
                    true
                ),
                (
                    "demo-app".to_string(),
                    vec!["strict".to_string(), "security".to_string()],
                    true
                ),
            ]
        );
    }

    #[test]
    fn a_nested_scope_inherits_intake_bindings_down_the_path_tree_and_never_up_or_sideways() {
        let c = intake_tree();
        let tools = c.intake_chain_for_scope(scope_named(&c, "engineering/tools"));
        let scopes: Vec<&str> = tools.iter().map(|l| l.scope.as_str()).collect();
        assert_eq!(scopes, vec!["n", "engineering"]);

        for elsewhere in ["company", "engineering/other", "lookalike"] {
            let chain = c.intake_chain_for_scope(scope_named(&c, elsewhere));
            let scopes: Vec<&str> = chain.iter().map(|l| l.scope.as_str()).collect();
            assert_eq!(
                scopes,
                vec!["n"],
                "{elsewhere} is not below projects by path, whatever its name says"
            );
        }
    }

    #[test]
    fn the_root_intake_layer_is_always_present_with_no_bindings_anywhere() {
        let c = config_with("");
        let chain = c.intake_chain_for_scope(&scope_at("demo", "demo"));
        assert_eq!(
            intake_chain_of(&chain),
            vec![("n".to_string(), vec!["ready".to_string()], false)]
        );
    }

    #[test]
    fn the_root_intake_layer_is_named_from_its_own_registered_scope_and_never_appears_twice() {
        let mut c = config_with("");
        let root_scope: Scope =
            serde_yaml_ng::from_str("id: root-id\nname: company\npath: .\n").unwrap();
        c.scope = Some(root_scope.clone());
        c.scopes = vec![
            root_scope,
            scope_with_intake("demo", "projects/demo", "[strict]"),
        ];

        let chain = c.intake_chain_for_scope(scope_named(&c, "demo"));
        assert_eq!(
            intake_chain_of(&chain),
            vec![
                ("company".to_string(), vec!["ready".to_string()], false),
                ("demo".to_string(), vec!["strict".to_string()], true),
            ]
        );
    }

    #[test]
    fn factory_intake_chain_matches_config_and_falls_back_to_the_root_for_an_unknown_name() {
        let f = factory_with(vec![scope_with_intake("demo", "demo", "[strict]")]);
        let expected = f.config.intake_chain_for_scope(f.scope("demo").unwrap());
        assert_eq!(f.intake_chain("demo"), expected);
        assert_eq!(
            intake_chain_of(&f.intake_chain("gone")),
            vec![("n".to_string(), vec!["ready".to_string()], false)]
        );
    }

    #[test]
    fn an_intake_declaration_round_trips_through_yaml_on_a_scope() {
        let s: Scope = serde_yaml_ng::from_str("id: s\nname: demo\nintake: [strict]\n").unwrap();
        let reparsed: Scope =
            serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&s).unwrap()).unwrap();
        assert_eq!(reparsed.intake, vec!["strict".to_string()]);
    }

    #[test]
    fn a_scope_binding_no_intake_round_trips_with_no_intake_key() {
        let s: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&s).unwrap().contains("intake"));
    }

    /// `#278`: the exact `scope.metrics` shape the issue proposes -- plain
    /// strings, no validation here (L5's `reported::validate` owns that on
    /// read). Own-scope data like `dependencies`/`environments` above, not
    /// a chained declaration like `policies`/`quality`: there is no
    /// root-level `metrics:` this would collide with, so nothing refuses
    /// it at the instance root either.
    #[test]
    fn a_metrics_source_declaration_round_trips_through_yaml_on_a_scope() {
        let yaml = "id: finance-id\n\
             name: finance\n\
             metrics:\n\
             \x20\x20source:\n\
             \x20\x20\x20\x20id: finance\n\
             \x20\x20\x20\x20file: ../data/finance/metrics.json\n\
             \x20\x20declare:\n\
             \x20\x20\x20\x20- { id: beleg_coverage, title: Beleg coverage, unit: ratio, better: higher }\n\
             \x20\x20\x20\x20- { id: unresolved_transactions, title: Unresolved transactions, unit: count, better: lower }\n";
        let s: Scope = serde_yaml_ng::from_str(yaml).unwrap();
        let declaration = s.metrics.as_ref().unwrap();
        let source = declaration.source.as_ref().unwrap();
        assert_eq!(source.id.as_deref(), Some("finance"));
        assert_eq!(source.file.as_deref(), Some("../data/finance/metrics.json"));
        assert_eq!(declaration.declare.len(), 2);
        assert_eq!(declaration.declare[0].id.as_deref(), Some("beleg_coverage"));
        assert_eq!(declaration.declare[0].title.as_deref(), Some("Beleg coverage"));
        assert_eq!(declaration.declare[0].unit.as_deref(), Some("ratio"));
        assert_eq!(declaration.declare[0].better.as_deref(), Some("higher"));
        assert_eq!(declaration.declare[1].better.as_deref(), Some("lower"));

        let reparsed: Scope =
            serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&s).unwrap()).unwrap();
        assert_eq!(reparsed.metrics, s.metrics);
    }

    /// `#278` fix: a `declare[]` entry missing `better` (or `title`/`unit`),
    /// or a `source` missing `file`, must still parse -- the whole point
    /// is that a missing value never fails the scope's config file to
    /// load, only L5's own `reported::validate` turns it into a finding.
    #[test]
    fn a_metrics_declaration_missing_optional_fields_still_parses() {
        let yaml = "id: finance-id\n\
             name: finance\n\
             metrics:\n\
             \x20\x20source:\n\
             \x20\x20\x20\x20id: finance\n\
             \x20\x20declare:\n\
             \x20\x20\x20\x20- { id: beleg_coverage, title: Beleg coverage, unit: ratio }\n";
        let s: Scope = serde_yaml_ng::from_str(yaml).unwrap();
        let declaration = s.metrics.as_ref().unwrap();
        assert_eq!(declaration.source.as_ref().unwrap().file, None);
        assert_eq!(declaration.declare[0].better, None);
    }

    #[test]
    fn a_scope_declaring_no_metrics_source_round_trips_with_no_metrics_key() {
        let s: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&s).unwrap().contains("metrics"));
    }

    #[test]
    fn the_root_config_refuses_scope_intake_and_says_there_is_nowhere_else_to_put_it() {
        let c = config_with("scope:\n  name: company\n  intake: [strict]\n");
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(e.contains("scope.intake"), "{e}");
        assert!(e.contains("automatically"), "{e}");
        assert!(e.contains("strict"), "{e}");
        assert!(c.validate().is_err(), "validate() calls the same guard");
    }

    #[test]
    fn intake_dir_delegates_to_the_ready_modules_path() {
        let f = factory("/inst");
        assert_eq!(f.intake_dir(), factory_process::ready::ready_dir(&f.root));
    }

    #[test]
    fn the_instance_root_message_does_not_claim_to_list_every_role() {
        let c = config_with("scope:\n  name: company\n  agents:\n    - name: critic\n      harness: pi\n      role: reviewer\n");
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(e.contains("defined at the instance root"), "{e}");
    }

    #[test]
    fn shell_agent_arguments_are_refused_with_the_declaration_named() {
        for declaration in [
            "    agent:\n      harness: shell\n      args: [--login]\n",
            "    agents:\n      - name: scripted\n        harness: shell\n        args: [--login]\n",
        ] {
            let c = config_with(&format!(
                "scopes:\n  - name: demo\n    path: .\n{declaration}"
            ));
            let e = c.validate().unwrap_err().to_string();
            assert!(e.contains("demo"), "{e}");
            assert!(e.contains("shell"), "{e}");
            assert!(e.contains("arguments"), "{e}");
        }
    }

    #[test]
    fn misspelled_agent_arguments_are_not_silently_ignored() {
        for yaml in [
            "name: a\npath: .\nagent:\n  harness: pi\n  arg: [--model, opus]\n",
            "name: a\npath: .\nagents:\n  - harness: pi\n    arg: [--model, opus]\n",
        ] {
            let e = serde_yaml_ng::from_str::<Scope>(yaml)
                .unwrap_err()
                .to_string();
            assert!(
                e.contains("arg"),
                "the error should name the bad field: {e}"
            );
        }
    }

    #[test]
    fn agent_arguments_have_to_be_a_list() {
        let e = serde_yaml_ng::from_str::<Scope>(
            "name: a\npath: .\nagent:\n  harness: pi\n  args: --model opus\n",
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("sequence"),
            "the error should say what args expects: {e}"
        );
    }

    #[test]
    fn the_two_that_ship_need_no_declaring() {
        let c = config_with(
            "scopes:\n  - name: demo\n    path: .\n    agents:\n      - name: boss\n        harness: pi\n        role: foreman\n",
        );
        c.validate().unwrap();
    }

    #[test]
    fn a_scope_collects_agents_from_both_spellings() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  harness: pi\n  lifetime: permanent\n  args: [--model, opus]\n\
             agents:\n  - name: watcher\n    harness: claude-code\n    lifetime: permanent\n\
             \x20   args: [--model, haiku]\n\
             \x20 - name: helper\n    harness: codex\n",
        )
        .unwrap();
        let declared = s.declared_agents();
        assert_eq!(declared.len(), 3);
        assert_eq!(s.agent_adapter(), Some("pi"));
        assert_eq!(declared[0].args, ["--model", "opus"]);
        assert_eq!(declared[1].args, ["--model", "haiku"]);
        let standing: Vec<String> = s.standing_agents().iter().map(|a| a.name()).collect();
        assert_eq!(
            standing,
            vec!["pi", "watcher"],
            "codex is a task agent, not standing"
        );
    }

    /// `sandbox` has to be added in three places -- `ScopeAgent`, the
    /// hand-written `Declaration` inside `AgentRef`'s `Deserialize`, and the
    /// `AgentRef::Declared` variant it builds -- or one of the two spellings
    /// below fails to load, or silently drops the field it was given. Prove
    /// both, from both spellings, rather than assume the third site was
    /// enough.
    #[test]
    fn a_declared_sandbox_loads_from_both_spellings_and_the_default_is_none() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  harness: pi\n  sandbox: docker\n\
             agents:\n  - name: watcher\n    harness: claude-code\n    sandbox: srt\n\
             \x20 - name: bare\n    harness: codex\n",
        )
        .unwrap();
        let declared = s.declared_agents();
        assert_eq!(declared[0].name(), "pi");
        assert_eq!(declared[0].sandbox, Sandbox::Docker, "the singular block");
        assert_eq!(declared[1].name(), "watcher");
        assert_eq!(declared[1].sandbox, Sandbox::Srt, "the agents: list");
        assert_eq!(declared[2].name(), "bare");
        assert_eq!(
            declared[2].sandbox,
            Sandbox::None,
            "an agent that never mentions it stays at today's default"
        );
    }

    /// `openshell` is the fourth value in the same three places, and its
    /// `openshell:` block rides along in each of them (`#218`).
    #[test]
    fn an_openshell_sandbox_and_its_block_load_from_both_spellings() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\n\
             agent:\n  harness: claude-code\n  sandbox: openshell\n  openshell:\n    image: img:1\n    providers: [claude]\n    policy:\n      network_policies: {}\n\
             agents:\n  - name: watcher\n    harness: shell\n    sandbox: openshell\n    openshell:\n      image: img:1\n      providers: [claude]\n      policy:\n        network_policies: {}\n",
        )
        .unwrap();
        let declared = s.declared_agents();
        assert_eq!(declared.len(), 2);
        for agent in &declared {
            assert_eq!(agent.sandbox, Sandbox::Openshell, "{}", agent.name());
            let os = agent.openshell.as_ref().expect("the block is kept");
            assert_eq!(os.image.as_deref(), Some("img:1"));
            assert_eq!(
                os.providers,
                [factory_environment::openshell::ProviderDecl::Named(
                    "claude".into()
                )]
            );
        }
        assert!(
            Sandbox::Openshell.is_enforced()
                && !Sandbox::Docker.is_enforced()
                && !Sandbox::Srt.is_enforced()
        );
        let bare = ScopeAgent {
            openshell: None,
            sandbox: Sandbox::None,
            ..declared[0].clone()
        };
        assert!(
            !serde_yaml_ng::to_string(&bare)
                .unwrap()
                .contains("openshell"),
            "an agent without the block is never written with one"
        );
    }

    #[test]
    fn openshell_without_its_block_or_a_block_without_openshell_or_a_bad_block_is_refused_naming_the_scope(
    ) {
        let cases = [
            ("        sandbox: openshell\n", "without an openshell: block"),
            ("        openshell:\n          image: img\n          policy: {}\n", "but sandbox: none"),
            (
                "        sandbox: openshell\n        openshell:\n          image: img\n          factory_bin: factory\n          policy: {}\n",
                "absolute path",
            ),
        ];
        for (agent, expected) in cases {
            let c = config_with(&format!(
                "scopes:\n  - name: demo\n    path: projects/demo\n    agents:\n      - name: curator\n        harness: claude-code\n{agent}"
            ));
            let e = c.validate().unwrap_err().to_string();
            assert!(
                e.contains(expected) && e.contains("curator") && e.contains("projects/demo"),
                "{e}"
            );
        }
        let fine = config_with(
            "scopes:\n  - name: demo\n    path: projects/demo\n    agents:\n      - name: curator\n        harness: claude-code\n        sandbox: openshell\n        openshell:\n          image: img\n          policy: {}\n",
        );
        fine.validate().unwrap();
        // A misspelt key never parses at all, so it is refused at load with
        // the file named by whoever read it ("parsing <path>: ...").
        let typo: std::result::Result<Scope, _> = serde_yaml_ng::from_str(
            "name: a\nagents:\n  - harness: claude-code\n    sandbox: openshell\n    openshell:\n      image: img\n      polcy: {}\n",
        );
        assert!(typo.unwrap_err().to_string().contains("polcy"));
    }

    #[test]
    fn a_secret_reference_names_a_declared_secret_and_agrees_with_its_date() {
        const SECRETS: &str = "secrets:\n  - { name: claude-oauth-token, kind: token, source: { from: file, path: ~/t }, expires: 2027-10-04 }\n  - { name: gh, kind: token, source: { from: command, run: gh auth token }, expires: never }\n  - { name: open, kind: other, source: { from: env, name: OPEN } }\n";
        let with = |secrets: &str, credential: &str| {
            config_with(&format!(
                "{secrets}scopes:\n  - name: demo\n    path: projects/demo\n    agents:\n      - name: curator\n        harness: claude-code\n        sandbox: openshell\n        openshell:\n          image: img\n          providers:\n            - name: factory-claude\n              type: claude-code-oauth\n              {credential}\n          policy: {{}}\n"
            ))
        };
        with(SECRETS, "credential: { secret: claude-oauth-token }")
            .validate()
            .unwrap();
        // The same date on the provider agrees, and is allowed.
        with(
            SECRETS,
            "credential: { secret: claude-oauth-token }\n              expires: 2027-10-04",
        )
        .validate()
        .unwrap();
        // An inline source keeps working (#234), with or without a catalogue.
        with(
            "",
            "credential: { from: file, path: ~/t }\n              expires: 2027-10-04",
        )
        .validate()
        .unwrap();
        for (secrets, credential, expected) in [
            ("", "credential: { secret: claude-oauth-token }", "does not declare (it declares: nothing)"),
            (SECRETS, "credential: { secret: claude }", "does not declare (it declares: claude-oauth-token, gh, open)"),
            (SECRETS, "credential: { secret: claude-oauth-token }\n              expires: 2027-11-04", "expires: 2027-11-04, and the secret claude-oauth-token it names says expires: 2027-10-04"),
            (SECRETS, "credential: { secret: gh }\n              expires: 2027-11-04", "says expires: never"),
            (SECRETS, "credential: { secret: open }\n              expires: 2027-11-04", "gives no expires:"),
        ] {
            let e = with(secrets, credential).validate().unwrap_err().to_string();
            assert!(e.contains(expected) && e.contains("factory-claude") && e.contains("curator"), "{credential}: {e}");
        }
        let twice = config_with("secrets:\n  - { name: a, kind: token, source: { from: env, name: A } }\n  - { name: a, kind: token, source: { from: env, name: B } }\n");
        assert!(twice.validate().unwrap_err().to_string().contains("twice"));
        assert!(twice
            .validate_instance()
            .unwrap_err()
            .to_string()
            .contains("twice"));
    }

    #[test]
    fn a_secrets_block_in_a_nested_scope_file_is_refused() {
        let document: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("scope:\n  id: s\n  name: demo\nsecrets: []\n").unwrap();
        let e = refuse_misplaced_scope_secrets(
            &document,
            Path::new("/x/projects/demo/.factory/config.yaml"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            e.contains("/x/projects/demo/.factory/config.yaml")
                && e.contains("only the instance root"),
            "{e}"
        );
        let fine: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("scope:\n  id: s\n  name: demo\n").unwrap();
        refuse_misplaced_scope_secrets(&fine, Path::new("x")).unwrap();
    }

    #[test]
    fn openshell_standing_agents_are_refused_rather_than_started_on_the_host() {
        for lifetime in ["permanent", "temporary"] {
            let c = config_with(&format!(
                "scopes:\n  - name: demo\n    path: projects/demo\n    agents:\n      - name: boxed\n        harness: claude-code\n        lifetime: {lifetime}\n        sandbox: openshell\n        openshell:\n          image: img\n          policy: {{}}\n"
            ));
            let error = c.validate().unwrap_err().to_string();
            assert!(
                error.contains("only lifetime: task") && error.contains("projects/demo"),
                "{error}"
            );
        }
    }

    #[test]
    fn a_temporary_agent_parses_and_does_not_autostart() {
        // Instances written before standing agents existed already say this.
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  name: reviewer\n  harness: pi\n  max_sessions: 1\n  lifetime: temporary\n",
        )
        .unwrap();
        let a = &s.standing_agents()[0];
        assert_eq!(a.name(), "reviewer");
        assert_eq!(a.lifetime, Lifetime::Temporary);
        assert!(!a.autostart(), "temporary agents wait to be asked for");
        assert_eq!(
            a.max_sessions,
            Some(1),
            "the legacy singular block's value is kept, not thrown away (#179)"
        );
    }

    #[test]
    fn a_permanent_agent_autostarts_unless_told_not_to() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagents:\n  - harness: pi\n    lifetime: permanent\n",
        )
        .unwrap();
        assert!(s.standing_agents()[0].autostart());
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagents:\n  - harness: pi\n    lifetime: permanent\n    autostart: false\n",
        )
        .unwrap();
        assert!(!s.standing_agents()[0].autostart());
    }

    /// `max_sessions` has to be added in three places -- `ScopeAgent`, the
    /// hand-written `Declaration` inside `AgentRef`'s `Deserialize`, and the
    /// `AgentRef::Declared` variant it builds -- exactly like `sandbox`'s own
    /// comment on this file describes. Prove both spellings, plus a scope's
    /// own cap, rather than assume the third site was enough (`#179`).
    #[test]
    fn max_sessions_loads_from_both_agent_spellings_and_from_the_scope_itself() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nmax_sessions: 4\nagents:\n  - name: codex\n    harness: codex\n    max_sessions: 3\n\
             \x20 - name: bare\n    harness: shell\n",
        )
        .unwrap();
        assert_eq!(s.max_sessions, Some(4), "the scope's own cap");
        let declared = s.declared_agents();
        assert_eq!(
            declared[0].max_sessions,
            Some(3),
            "the agents: list spelling"
        );
        assert_eq!(declared[1].max_sessions, None, "absent means unlimited");
    }

    #[test]
    fn a_zero_max_sessions_is_refused_at_load_naming_the_scopes_path() {
        let c = config_with(
            "scopes:\n  - name: demo\n    path: projects/demo\n    agents:\n      - name: codex\n        harness: codex\n        max_sessions: 0\n",
        );
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("projects/demo"), "{e}");
        assert!(e.contains("codex"), "{e}");
        assert!(e.contains("max_sessions: 0"), "{e}");
    }

    #[test]
    fn a_zero_scope_max_sessions_is_refused_at_load() {
        let c =
            config_with("scopes:\n  - name: demo\n    path: projects/demo\n    max_sessions: 0\n");
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("projects/demo"), "{e}");
        assert!(e.contains("max_sessions: 0"), "{e}");
    }

    #[test]
    fn a_scope_may_name_its_agent_either_way() {
        let plain: Scope = serde_yaml_ng::from_str("name: a\npath: .\nagent: pi\n").unwrap();
        assert_eq!(plain.agent_adapter(), Some("pi"));

        let declared: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  name: Factory Builder\n  harness: pi\n  max_sessions: 1\n",
        )
        .unwrap();
        assert_eq!(declared.agent_adapter(), Some("pi"));
        assert_eq!(declared.declared_agents()[0].max_sessions, Some(1));

        let none: Scope = serde_yaml_ng::from_str("name: a\npath: .\n").unwrap();
        assert_eq!(none.agent_adapter(), None);
    }

    #[test]
    fn instance_discovery_walks_past_a_nested_scope_config() {
        let root = std::env::temp_dir().join(format!(
            "factory-config-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let scope = root.join("projects/demo");
        std::fs::create_dir_all(root.join(FACTORY_DIR)).unwrap();
        std::fs::create_dir_all(scope.join(FACTORY_DIR)).unwrap();
        std::fs::write(
            root.join(FACTORY_DIR).join(CONFIG_FILE),
            "version: 1\ninstance: { id: i, name: instance }\n",
        )
        .unwrap();
        std::fs::write(
            scope.join(FACTORY_DIR).join(CONFIG_FILE),
            "version: 1\nscope: { id: s, name: demo }\n",
        )
        .unwrap();

        assert_eq!(
            Factory::discover(&scope.join("src")).unwrap(),
            Some(root.clone())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_short_root_keeps_its_socket_in_the_instance() {
        let f = factory("/tmp/x");
        assert_eq!(
            f.socket_path(),
            PathBuf::from("/tmp/x/.factory/factory.sock")
        );
    }

    #[test]
    fn a_deep_root_falls_back_to_something_that_fits() {
        let deep = format!("/tmp/{}", "verylongsegment/".repeat(12));
        let f = factory(&deep);
        let socket = f.socket_path();
        assert!(
            socket.as_os_str().len() < 104,
            "{} is still too long",
            socket.display()
        );
        assert_ne!(socket.parent(), Some(f.factory_dir().as_path()));
    }

    #[test]
    fn the_fallback_is_the_same_every_time() {
        let deep = format!("/tmp/{}", "verylongsegment/".repeat(12));
        assert_eq!(factory(&deep).socket_path(), factory(&deep).socket_path());
    }

    #[test]
    fn two_deep_roots_do_not_share_a_socket() {
        let a = format!("/tmp/{}a", "verylongsegment/".repeat(12));
        let b = format!("/tmp/{}b", "verylongsegment/".repeat(12));
        assert_ne!(factory(&a).socket_path(), factory(&b).socket_path());
    }

    #[test]
    fn a_scope_that_names_a_task_store_gets_it() {
        let mut f = factory("/tmp/x");
        f.config.scopes =
            vec![serde_yaml_ng::from_str("name: demo\npath: .\ntask_store: postgres\n").unwrap()];
        assert_eq!(f.task_store_for("demo"), "postgres");
    }

    #[test]
    fn a_scope_without_a_task_store_uses_the_instance_default() {
        let mut f = factory("/tmp/x");
        f.config.scopes = vec![serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap()];
        assert_eq!(f.task_store_for("demo"), "sqlite");
    }

    #[test]
    fn an_unknown_scope_name_gets_the_instance_default() {
        let f = factory("/tmp/x");
        assert_eq!(f.task_store_for("ghost"), "sqlite");
    }

    #[test]
    fn task_stores_lists_the_default_first_and_dedupes_the_rest() {
        let mut f = factory("/tmp/x");
        f.config.scopes = vec![
            serde_yaml_ng::from_str("name: a\npath: .\ntask_store: sqlite\n").unwrap(),
            serde_yaml_ng::from_str("name: b\npath: .\ntask_store: postgres\n").unwrap(),
            serde_yaml_ng::from_str("name: c\npath: .\ntask_store: postgres\n").unwrap(),
            serde_yaml_ng::from_str("name: d\npath: .\n").unwrap(),
        ];
        assert_eq!(
            f.task_stores(),
            vec!["sqlite".to_string(), "postgres".to_string()],
            "the instance default leads, and postgres only appears once"
        );
    }

    #[test]
    fn a_config_with_no_scope_naming_a_task_store_still_parses() {
        // The back-compat guarantee that matters: instances written before
        // this field existed must keep loading unchanged.
        let yaml =
            "version: 1\ninstance:\n  id: i\n  name: n\nscopes:\n  - name: demo\n    path: .\n";
        let cfg: Config = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(cfg.scopes[0].task_store, None);
    }

    #[test]
    fn a_scopes_task_store_round_trips_and_stays_silent_when_unset() {
        let named: Scope =
            serde_yaml_ng::from_str("name: demo\npath: .\ntask_store: postgres\n").unwrap();
        assert!(serde_yaml_ng::to_string(&named)
            .unwrap()
            .contains("task_store: postgres"));

        let unnamed: Scope = serde_yaml_ng::from_str("name: demo\npath: .\n").unwrap();
        assert!(!serde_yaml_ng::to_string(&unnamed)
            .unwrap()
            .contains("task_store"));
    }

    // -- infrastructure.providers ---------------------------------------------

    const PROVIDERS: &str = "infrastructure:\n  providers:\n\
        \x20   - name: claude-max\n      vendor: anthropic\n      kind: subscription\n      plan: Max 20x\n      harnesses: [claude-code]\n\
        \x20   - name: openrouter\n      vendor: openrouter\n      kind: api-key\n      env: OPENROUTER_API_KEY\n      harnesses: [pi, opencode]\n";

    /// A root config declaring `PROVIDERS`, plus `rest` after it.
    fn with_providers(rest: &str) -> std::result::Result<Config, String> {
        serde_yaml_ng::from_str::<Config>(&format!(
            "instance:\n  id: i\n  name: n\n{PROVIDERS}{rest}"
        ))
        .map_err(|e| e.to_string())
    }

    /// The issue's own example block, parsed.
    #[test]
    fn the_issues_provider_block_parses() {
        let c = with_providers("").unwrap();
        c.validate_instance().unwrap();
        c.validate().unwrap();
        let [max, router] = c.infrastructure.providers.as_slice() else {
            panic!("two providers: {:?}", c.infrastructure.providers)
        };
        assert_eq!(max.name, "claude-max");
        assert_eq!(max.kind, ProviderKind::Subscription);
        assert_eq!(max.plan.as_deref(), Some("Max 20x"));
        assert_eq!(max.env, None);
        assert_eq!(max.harnesses, ["claude-code"]);
        assert_eq!(router.kind, ProviderKind::ApiKey);
        assert_eq!(router.env.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(router.harnesses, ["pi", "opencode"]);

        let rendered = serde_yaml_ng::to_string(&c).unwrap();
        let reparsed: Config = serde_yaml_ng::from_str(&rendered).unwrap();
        assert_eq!(
            reparsed.infrastructure, c.infrastructure,
            "round-trips through its own YAML"
        );
    }

    #[test]
    fn a_config_declaring_no_providers_writes_no_infrastructure_key() {
        let c: Config = serde_yaml_ng::from_str("instance:\n  id: i\n  name: n\n").unwrap();
        assert!(c.infrastructure.is_empty());
        assert!(!serde_yaml_ng::to_string(&c)
            .unwrap()
            .contains("infrastructure"));
    }

    /// A backup block with no providers beside it is still a block: it must
    /// survive the root config being written back, and be checked on load.
    #[test]
    fn a_backup_block_alone_round_trips_and_is_validated_at_load() {
        let c: Config = serde_yaml_ng::from_str(
            "instance:\n  id: i\n  name: n\ninfrastructure:\n  backup:\n    destination: /Volumes/Backup/factory\n\
             \x20   schedule: { cron: \"0 3 * * *\", timezone: Europe/Berlin }\n",
        )
        .unwrap();
        assert!(!c.infrastructure.is_empty());
        c.validate_instance().unwrap();
        let rendered = serde_yaml_ng::to_string(&c).unwrap();
        assert!(
            rendered.contains("destination: /Volumes/Backup/factory"),
            "{rendered}"
        );
        let reparsed: Config = serde_yaml_ng::from_str(&rendered).unwrap();
        assert_eq!(reparsed.infrastructure, c.infrastructure);

        let relative: Config = serde_yaml_ng::from_str(
            "instance:\n  id: i\n  name: n\ninfrastructure:\n  backup:\n    destination: backups\n",
        )
        .unwrap();
        assert!(relative
            .validate_instance()
            .unwrap_err()
            .to_string()
            .contains("absolute"));
    }

    #[test]
    fn an_unknown_provider_kind_is_refused_at_parse_naming_the_kinds_there_are() {
        let e = serde_yaml_ng::from_str::<Config>(
            "instance:\n  id: i\n  name: n\ninfrastructure:\n  providers:\n    - name: x\n      vendor: y\n      kind: prepaid\n",
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("prepaid"), "{e}");
        assert!(e.contains("api-key") && e.contains("subscription"), "{e}");
    }

    #[test]
    fn a_misspelled_provider_field_is_refused_not_silently_dropped() {
        for yaml in [
            "infrastructure:\n  providers:\n    - name: x\n      vendor: y\n      kind: api-key\n      harness: [pi]\n",
            "infrastructure:\n  provider:\n    - name: x\n",
        ] {
            let e = serde_yaml_ng::from_str::<Config>(&format!("instance:\n  id: i\n  name: n\n{yaml}"))
                .unwrap_err()
                .to_string();
            assert!(e.contains("unknown field"), "{e}");
        }
    }

    #[test]
    fn duplicate_provider_names_are_refused() {
        let c = with_providers(
            "\x20   - name: claude-max\n      vendor: anthropic\n      kind: api-key\n",
        )
        .unwrap();
        for e in [
            c.validate_instance().unwrap_err(),
            c.validate().unwrap_err(),
        ] {
            let e = e.to_string();
            assert!(e.contains("claude-max") && e.contains("twice"), "{e}");
        }
    }

    #[test]
    fn two_providers_claiming_one_harness_are_refused_with_both_named() {
        let c = with_providers(
            "\x20   - name: anthropic-api\n      vendor: anthropic\n      kind: api-key\n      harnesses: [claude-code]\n",
        )
        .unwrap();
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(
            e.contains("claude-max") && e.contains("anthropic-api"),
            "{e}"
        );
        assert!(e.contains("claude-code"), "{e}");
    }

    #[test]
    fn env_on_a_subscription_is_refused() {
        let c: Config = serde_yaml_ng::from_str(
            "instance:\n  id: i\n  name: n\ninfrastructure:\n  providers:\n\
             \x20   - name: claude-max\n      vendor: anthropic\n      kind: subscription\n      env: ANTHROPIC_API_KEY\n",
        )
        .unwrap();
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(
            e.contains("claude-max") && e.contains("subscription"),
            "{e}"
        );
        assert!(e.contains("env"), "{e}");
    }

    #[test]
    fn a_provider_claiming_the_shell_harness_is_refused() {
        let c = with_providers("\x20   - name: sh\n      vendor: none\n      kind: api-key\n      harnesses: [shell]\n")
            .unwrap();
        let e = c.validate_instance().unwrap_err().to_string();
        assert!(e.contains("shell"), "{e}");
    }

    #[test]
    fn an_agent_naming_an_undeclared_provider_is_refused_with_what_there_is() {
        // Both entry points, both spellings: the root's own scope before
        // discovery, and a nested scope's `agents:` list after it.
        let root = with_providers(
            "scope:\n  name: company\n  agent:\n    harness: pi\n    provider: openruter\n",
        )
        .unwrap();
        let e = root.validate_instance().unwrap_err().to_string();
        assert!(e.contains("openruter"), "{e}");
        assert!(
            e.contains("claude-max, openrouter"),
            "it lists the declared ones: {e}"
        );

        let nested = with_providers(
            "scopes:\n  - name: lab\n    path: lab\n    agents:\n      - name: model-lab\n        harness: opencode\n        provider: nowhere\n",
        )
        .unwrap();
        nested.validate_instance().unwrap();
        let e = nested.validate().unwrap_err().to_string();
        assert!(e.contains("model-lab") && e.contains("nowhere"), "{e}");

        let bare = config_with("scopes:\n  - name: lab\n    path: lab\n    agent:\n      harness: pi\n      provider: x\n");
        let e = bare.validate().unwrap_err().to_string();
        assert!(e.contains("No providers are declared"), "{e}");
    }

    #[test]
    fn a_shell_agent_given_a_provider_is_refused() {
        let c = with_providers(
            "scopes:\n  - name: lab\n    path: lab\n    agents:\n      - name: scripted\n        harness: shell\n        provider: openrouter\n",
        )
        .unwrap();
        let e = c.validate().unwrap_err().to_string();
        assert!(e.contains("scripted") && e.contains("shell"), "{e}");
    }

    /// `provider` lives in the same three places `sandbox` does -- see
    /// `a_declared_sandbox_loads_from_both_spellings_and_the_default_is_none`.
    #[test]
    fn a_declared_provider_loads_from_both_spellings_and_the_default_is_none() {
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagent:\n  harness: pi\n  provider: openrouter\n\
             agents:\n  - name: watcher\n    harness: claude-code\n    provider: claude-max\n\
             \x20 - name: bare\n    harness: codex\n",
        )
        .unwrap();
        let declared = s.declared_agents();
        assert_eq!(
            declared[0].provider.as_deref(),
            Some("openrouter"),
            "the singular block"
        );
        assert_eq!(
            declared[1].provider.as_deref(),
            Some("claude-max"),
            "the agents: list"
        );
        assert_eq!(declared[2].provider, None);
        assert!(
            !serde_yaml_ng::to_string(&declared[2])
                .unwrap()
                .contains("provider"),
            "an agent that never named one is never written with one"
        );
    }

    #[test]
    fn an_agents_own_provider_wins_over_its_harness_default_and_shell_never_has_one() {
        let c = with_providers("").unwrap();
        let s: Scope = serde_yaml_ng::from_str(
            "name: a\npath: .\nagents:\n\
             \x20 - name: builder\n    harness: claude-code\n\
             \x20 - name: model-lab\n    harness: claude-code\n    provider: openrouter\n\
             \x20 - name: helper\n    harness: codex\n\
             \x20 - name: scripted\n    harness: shell\n",
        )
        .unwrap();
        let agents = s.declared_agents();
        let bound: Vec<Option<(&str, ProviderVia)>> = agents
            .iter()
            .map(|a| {
                c.infrastructure
                    .provider_for(a)
                    .map(|(p, via)| (p.name.as_str(), via))
            })
            .collect();
        assert_eq!(
            bound,
            vec![
                Some(("claude-max", ProviderVia::Harness)),
                Some(("openrouter", ProviderVia::Agent)),
                None, // a model agent nothing claims: unassigned, not an error
                None, // shell: no provider, ever
            ]
        );
    }

    #[test]
    fn an_infrastructure_block_in_a_nested_scope_file_is_refused() {
        let document: serde_yaml_ng::Value =
            serde_yaml_ng::from_str(&format!("scope:\n  id: s\n  name: demo\n{PROVIDERS}"))
                .unwrap();
        let e = refuse_misplaced_scope_infrastructure(
            &document,
            Path::new("/x/projects/demo/.factory/config.yaml"),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("infrastructure"), "{e}");
        assert!(e.contains("projects/demo"), "{e}");

        let fine: serde_yaml_ng::Value =
            serde_yaml_ng::from_str("scope:\n  id: s\n  name: demo\n").unwrap();
        refuse_misplaced_scope_infrastructure(&fine, Path::new("x")).unwrap();
    }

    #[test]
    fn dependency_services_are_strict_and_only_name_declared_agents() {
        let yaml = concat!(
            "name: demo\n",
            "agents:\n  - name: finance\n    harness: shell\n",
            "dependencies:\n  max_age: 30d\n  services:\n",
            "    - name: bank\n      transport: network\n",
            "      endpoints: [bank.test:443]\n      effects: read\n",
            "      agents: [finance]\n",
        );
        let scope: Scope = serde_yaml_ng::from_str(yaml).unwrap();
        scope.validate_dependencies().unwrap();
        assert_eq!(
            scope.dependencies.services[0].effects,
            vec![DependencyEffect::Read]
        );

        let bad = yaml.replace("agents: [finance]", "agents: [auditor]");
        let bad: Scope = serde_yaml_ng::from_str(&bad).unwrap();
        assert!(bad
            .validate_dependencies()
            .unwrap_err()
            .to_string()
            .contains("auditor"));

        let typo = yaml.replace("max_age:", "max_gae:");
        assert!(serde_yaml_ng::from_str::<Scope>(&typo).is_err());
    }
}

impl factory_kernel::ScopeIdentity for Scope {
    fn scope_name(&self) -> &str {
        &self.name
    }
    fn scope_path(&self) -> &Path {
        &self.path
    }
}

impl factory_direction::policy_intent::ScopePolicies for Scope {
    fn policy_declaration(&self) -> &PolicyDeclaration {
        &self.policies
    }
}

impl factory_agents::role_chain::ScopeRoles for Scope {
    fn role_specs(&self) -> &BTreeMap<String, RoleSpec> {
        &self.roles
    }
}
