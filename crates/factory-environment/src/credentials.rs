//! Presence-only filesystem metadata, never credential contents.
use factory_kernel::{FactoryError, Result, ScopeTree, SecretsPresence};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialRow {
    pub label: String,
    pub path: String,
    pub integration: String,
    pub present: bool,
    /// The scope this row belongs to, for the rows that belong to one at all.
    /// `None` is the honest answer for a credential in the owner's home: it
    /// sits outside every scope and is reachable from all of them, so the
    /// page goes on showing it whichever scope the rail has selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

pub struct Provider {
    pub root: PathBuf,
    pub scopes: ScopeTree,
    pub home: Option<PathBuf>,
}
impl factory_kernel::FactProvider for Provider {
    type Level = factory_kernel::L2;
}
impl Provider {
    pub async fn inventory(&self) -> Vec<CredentialRow> {
        let mut rows = Vec::new();

        if let Some(home) = self.home.clone() {
            let fixed = [
                (
                    "Claude Code credentials",
                    home.join(".claude/.credentials.json"),
                    "anthropic",
                ),
                (
                    "GitHub CLI hosts",
                    home.join(".config/gh/hosts.yml"),
                    "github",
                ),
                ("AWS credentials", home.join(".aws/credentials"), "aws"),
                ("netrc", home.join(".netrc"), "netrc"),
            ];
            for (label, path, integration) in fixed {
                let present = tokio::fs::try_exists(&path).await.unwrap_or(false);
                rows.push(CredentialRow {
                    label: label.into(),
                    path: path.display().to_string(),
                    integration: integration.into(),
                    present,
                    scope: None,
                });
            }

            // Presence only: an id_* file that is not a `.pub` is treated as
            // a private key without ever being opened to check.
            let ssh_dir = home.join(".ssh");
            let mut ssh_present = false;
            if let Ok(mut entries) = tokio::fs::read_dir(&ssh_dir).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.starts_with("id_") && !name.ends_with(".pub") {
                        ssh_present = true;
                        break;
                    }
                }
            }
            rows.push(CredentialRow {
                label: "SSH private keys".into(),
                path: ssh_dir.join("id_*").display().to_string(),
                integration: "ssh".into(),
                present: ssh_present,
                scope: None,
            });
        }

        for scope in &self.scopes.scopes {
            let scope_dir = self
                .scopes
                .scope(&scope.name)
                .map(|s| {
                    if s.path.is_absolute() {
                        s.path.clone()
                    } else {
                        self.root.join(&s.path)
                    }
                })
                .unwrap_or_else(|_| scope.path.clone());
            // A scope registered on the instance root has the path `<root>/.`,
            // so joining onto it raw would print `<root>/./.env` on the page.
            // Collecting the components drops the `.` without touching what
            // the path means.
            let env_path = scope_dir.components().collect::<PathBuf>().join(".env");
            let present = tokio::fs::try_exists(&env_path).await.unwrap_or(false);
            rows.push(CredentialRow {
                label: format!("{} .env", scope.name),
                path: env_path.display().to_string(),
                integration: "scope env".into(),
                present,
                scope: Some(scope.name.clone()),
            });
        }

        rows
    }
}
#[async_trait::async_trait]
impl factory_kernel::Provide<SecretsPresence> for Provider {
    type Query = BTreeSet<String>;
    type Value = BTreeMap<String, SecretsPresence>;
    type Error = FactoryError;
    async fn get(&self, requested: &Self::Query) -> Result<Self::Value> {
        if requested.is_empty() {
            return Ok(BTreeMap::new());
        }
        let names = requested
            .iter()
            .map(|name| self.scopes.scope(name).map(|scope| scope.name.clone()))
            .collect::<Result<Vec<_>>>()?;
        let rows = self.inventory().await;
        Ok(names
            .into_iter()
            .map(|name| (name.clone(), secrets_fact_map(&rows, &name).into()))
            .collect())
    }
}
fn secrets_fact_map(rows: &[CredentialRow], scope: &str) -> BTreeMap<String, bool> {
    let mut map = BTreeMap::new();
    for row in rows {
        let id = match (row.integration.as_str(), row.scope.as_deref()) {
            ("anthropic", None) => "anthropic",
            ("github", None) => "github",
            ("aws", None) => "aws",
            ("netrc", None) => "netrc",
            ("ssh", None) => "ssh",
            ("scope env", Some(s)) if s == scope => "scope_env",
            _ => continue,
        };
        map.insert(id.to_string(), row.present);
    }
    map
}
