//! Check 1: configuration validity for every registered scope.
//!
//! `factory_config::load` validates the whole instance file as one unit —
//! its own module docs describe walking `version`, `instance.id`, then each
//! scope entry in file order — so "for every registered scope" is satisfied
//! by validating the one file that declares them all; there is no separate
//! per-scope entry point to call. A missing or unreadable file is reported
//! the same way as a validation problem, because [`factory_config::load`]
//! itself does not distinguish them (its `ConfigError` covers "could not
//! read" and "did not validate" alike).

use std::path::Path;

pub(crate) fn check(
    company_root: &Path,
) -> Result<factory_config::InstanceConfig, factory_config::ConfigError> {
    let config_path = company_root.join(".factory").join("config.yaml");
    factory_config::load(config_path)
}
