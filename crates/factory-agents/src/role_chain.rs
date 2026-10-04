//! One live role-chain resolution for configuration, authorization and facts.
use crate::role::{RoleOrigin, RoleSpec, Roles};
use factory_kernel::{Result, ScopeIdentity};
use std::collections::BTreeMap;

/// Only the identity and role declarations a chain walk needs.
pub trait ScopeRoles: ScopeIdentity {
    fn role_specs(&self) -> &BTreeMap<String, RoleSpec>;
}

/// Presets, root roles, then each configured path ancestor and the scope itself.
/// Nearest definitions replace whole; nothing flows up or sideways.
pub fn roles_for_scope<S: ScopeRoles>(
    root: &BTreeMap<String, RoleSpec>,
    scopes: &[S],
    scope: &S,
) -> Result<Roles> {
    let mut roles = Roles::resolve(root)?;
    for layer in factory_kernel::scope_ancestors(scopes, scope)
        .into_iter()
        .chain(std::iter::once(scope))
    {
        if layer.role_specs().is_empty() {
            continue;
        }
        roles = roles.layered(
            RoleOrigin::Scope {
                scope: layer.scope_name().to_owned(),
            },
            layer.role_specs(),
        )?;
    }
    Ok(roles)
}
