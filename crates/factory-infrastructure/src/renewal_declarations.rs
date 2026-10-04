//! L1-owned authored renewal metadata read. Only selected fields leave this
//! parser. Bad edits retain the last-good metadata and surface a finding;
//! the ledger never copies policy attestations or other authored controls.
use crate::renewals;
use factory_kernel::{error::Result, RenewalDecl};
use factory_kernel::{FactoryError, Provide};
use factory_kernel::{RenewalDeclarationsFact, ScopedRenewalDeclaration};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use tokio::io::AsyncReadExt;

#[derive(Default, Deserialize)]
struct Section {
    #[serde(default)]
    renewals: Vec<RenewalDecl>,
}
#[derive(Deserialize)]
struct Document {
    #[serde(default)]
    renewals: Vec<RenewalDecl>,
    #[serde(default)]
    scope: Option<Section>,
}

async fn selected(
    path: &Path,
    root: bool,
    instance_file: bool,
) -> std::result::Result<Option<Vec<RenewalDecl>>, ()> {
    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| ())?;
    if bytes.len() > 1024 * 1024 {
        return Err(());
    }
    let document: Document = serde_yaml_ng::from_slice(&bytes).map_err(|_| ())?;
    if !instance_file && !document.renewals.is_empty() {
        return Err(());
    }
    let declarations = if root {
        document.renewals
    } else {
        document.scope.unwrap_or_default().renewals
    };
    renewals::validate(&declarations, None).map_err(|_| ())?;
    Ok(Some(declarations))
}

/// Authored inputs only; file parsing and last-good cache stay in L1.
#[derive(Clone)]
pub struct ScopeRenewals {
    pub name: String,
    pub path: PathBuf,
    pub declarations: Vec<RenewalDecl>,
}
#[derive(Default)]
pub struct DeclarationCache(
    std::sync::Mutex<std::collections::BTreeMap<PathBuf, Vec<RenewalDecl>>>,
);
pub struct Provider<'a> {
    pub cache: &'a DeclarationCache,
    pub instance_path: PathBuf,
    pub declarations: Vec<RenewalDecl>,
    pub scopes: Vec<ScopeRenewals>,
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L1;
}
#[async_trait::async_trait]
impl Provide<RenewalDeclarationsFact> for Provider<'_> {
    type Query = ();
    type Value = RenewalDeclarationsFact;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<RenewalDeclarationsFact> {
        let instance_path = &self.instance_path;
        let mut sources: Vec<(PathBuf, Option<String>, bool, Vec<RenewalDecl>)> =
            vec![(instance_path.clone(), None, true, self.declarations.clone())];
        let mut names = std::collections::BTreeSet::new();
        for scope in self
            .scopes
            .iter()
            .filter(|scope| names.insert(scope.name.clone()))
        {
            sources.push((
                scope.path.join(".factory/config.yaml"),
                Some(scope.name.clone()),
                false,
                scope.declarations.clone(),
            ));
        }
        let mut declarations = Vec::new();
        let mut findings = Vec::new();
        for (path, scope, root, fallback) in sources {
            // Root and scope sections can share one file; their cache keys must
            // not collide, or a bad edit would mix global and scoped metadata.
            let key = if root {
                path.with_extension("root-renewals")
            } else {
                path.with_extension("scope-renewals")
            };
            let values = match selected(&path, root, &path == instance_path).await {
                Ok(Some(values)) => {
                    self.cache
                        .0
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(key, values.clone());
                    values
                }
                Ok(None) => {
                    let old = self
                        .cache
                        .0
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(&key)
                        .cloned();
                    if let Some(old) = old {
                        findings.push(format!("renewal declaration file for {} is unavailable; last-good metadata retained", scope.as_deref().unwrap_or("instance")));
                        old
                    } else {
                        fallback
                    }
                }
                Err(()) => {
                    findings.push(format!("renewal declarations for {} could not be parsed or validated; last-good metadata retained", scope.as_deref().unwrap_or("instance")));
                    self.cache
                        .0
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .get(&key)
                        .cloned()
                        .unwrap_or(fallback)
                }
            };
            declarations.extend(
                values
                    .into_iter()
                    .map(|declaration| ScopedRenewalDeclaration {
                        scope: scope.clone(),
                        declaration,
                    }),
            );
        }
        Ok(RenewalDeclarationsFact {
            declarations,
            findings,
        })
    }
}
