//! Check 3: registry drift (ADR 0016). The backlog names "a registered path
//! that no longer exists" as an example, not a filter: this reports the
//! whole [`factory_registry::DriftReport`], because ADR 0016 already draws
//! the line doctor needs — "Reconcile reports; applying is a separate act...
//! Slice 10 makes the same split for `factory doctor`" — and its own text
//! names `UnreadableContext`, `PathChangedDifferentIdentity`, and
//! `ProjectedNotDeclared` as the variants that are *never* applied
//! automatically because a human must look. Filtering to
//! `Drift::MissingPath` here would silently drop exactly the set doctor
//! exists to surface.

use std::path::Path;

use crate::{CheckName, Finding};

/// `scopes.declared_path`, `.git`, `.dev`, `.ino`, and `.parent_id` do not
/// exist before migration 2 (`factory_store`'s `schema::V2_SCHEMA` — see
/// that crate's `migrations.rs` doc comment on migration 2 rebuilding
/// `scopes`). `factory_registry::reconcile` reads all five, so this check
/// needs schema 2 at minimum. A released migration is never edited (ADR
/// 0012 decision 2: forward-only, append-only), so this fact about an
/// already-shipped migration cannot drift the way
/// [`factory_store::latest_schema_version`] can.
pub(crate) const MIN_SCHEMA: i64 = 2;

pub(crate) fn check(
    company_root: &Path,
    store: &factory_store::Store,
    schema_version: i64,
    config: Option<&factory_config::InstanceConfig>,
    findings: &mut Vec<Finding>,
) {
    if schema_version < MIN_SCHEMA {
        findings.push(Finding::CheckSkipped {
            check: CheckName::RegistryDrift,
            reason: format!(
                "database schema is {schema_version}; registry reconciliation reads \
                 `scopes` columns introduced in schema {MIN_SCHEMA}"
            ),
        });
        return;
    }

    let Some(config) = config else {
        findings.push(Finding::CheckSkipped {
            check: CheckName::RegistryDrift,
            reason: "the instance configuration is invalid; see the configuration finding"
                .to_string(),
        });
        return;
    };

    let scopes = match factory_registry::resolve(config, company_root) {
        Ok(scopes) => scopes,
        Err(err) => {
            findings.push(Finding::RegistryUnresolvable(err.to_string()));
            return;
        }
    };

    match factory_registry::reconcile(store, &scopes) {
        Ok(report) => {
            for drift in report.items {
                findings.push(Finding::RegistryDrift(drift));
            }
        }
        Err(err) => findings.push(Finding::RegistryUnresolvable(err.to_string())),
    }
}
