//! What a plugin has to say about itself before the daemon will run it.

use factory_core::adapter::AdapterKind;
use factory_core::error::{FactoryError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "plugin.yaml";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// The name the adapter registers under. A plugin may not take the name of
    /// a built-in adapter; the registry refuses the collision rather than
    /// silently shadowing.
    pub name: String,
    pub kind: AdapterKind,
    #[serde(default)]
    pub description: String,
    /// Argv. A relative program is resolved against the plugin's own directory,
    /// so a plugin can ship its executable next to its manifest.
    pub command: Vec<String>,
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Seconds to wait for a reply before giving up on a call.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,
}

fn default_timeout() -> u64 {
    30
}

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    pub manifest_path: PathBuf,
}

impl LoadedPlugin {
    /// Argv with the program made absolute if it lives in the plugin directory.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = self.manifest.command.clone();
        if let Some(first) = argv.first_mut() {
            let p = Path::new(first.as_str());
            if p.is_relative() && (first.contains('/') || self.dir.join(p).exists()) {
                *first = self.dir.join(p).display().to_string();
            }
        }
        argv
    }
}

/// Read every `<dir>/*/plugin.yaml`. A plugin that will not parse is reported
/// and skipped -- one bad plugin must not keep the daemon from starting.
pub fn discover(dir: &Path) -> (Vec<LoadedPlugin>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        // No plugins directory is the normal case, not a problem.
        Err(_) => return (found, problems),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            continue;
        }
        match load(&manifest_path) {
            Ok(p) => found.push(p),
            Err(e) => problems.push(format!("{}: {e}", manifest_path.display())),
        }
    }
    found.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    (found, problems)
}

pub fn load(manifest_path: &Path) -> Result<LoadedPlugin> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("{e}")))?;
    let manifest: PluginManifest =
        serde_yaml_ng::from_str(&text).map_err(|e| FactoryError::Other(anyhow::anyhow!("{e}")))?;
    if manifest.command.is_empty() {
        return Err(FactoryError::Other(anyhow::anyhow!(
            "`command` is empty; a plugin needs something to run"
        )));
    }
    Ok(LoadedPlugin {
        dir: manifest_path
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf(),
        manifest_path: manifest_path.to_path_buf(),
        manifest,
    })
}
