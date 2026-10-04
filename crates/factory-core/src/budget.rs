//! L6 authored budget intent and pure monthly assessments (#164).
//! Spend is an L4 fact; neither this catalogue nor its views invent prices
//! or derive a limit from a provider plan. No aggregate store is maintained.
use crate::usage::{CostGroupBy, CostReport, CostRow};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MAX_CATALOGUE_BYTES: u64 = 1024 * 1024;

pub fn default_group_by() -> CostGroupBy {
    CostGroupBy::Scope
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MonthlyLimit {
    pub monthly_usd: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalogue {
    pub version: u32,
    #[serde(default, deserialize_with = "unique_scopes")]
    pub scopes: BTreeMap<String, MonthlyLimit>,
}

impl Default for Catalogue {
    fn default() -> Self {
        Self {
            version: 1,
            scopes: BTreeMap::new(),
        }
    }
}

fn unique_scopes<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<BTreeMap<String, MonthlyLimit>, D::Error> {
    struct Unique;
    impl<'de> serde::de::Visitor<'de> for Unique {
        type Value = BTreeMap<String, MonthlyLimit>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("unique scope ids mapped to monthly USD limits")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut out = BTreeMap::new();
            while let Some((id, limit)) = map.next_entry::<String, MonthlyLimit>()? {
                if out.insert(id.clone(), limit).is_some() {
                    return Err(serde::de::Error::custom(format!(
                        "duplicate budget scope id {id:?}"
                    )));
                }
            }
            Ok(out)
        }
    }
    d.deserialize_map(Unique)
}

impl Catalogue {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let value: Self = serde_yaml_ng::from_str(raw).map_err(|e| e.to_string())?;
        if value.version != 1 {
            return Err(format!(
                "unsupported budget catalogue version {}",
                value.version
            ));
        }
        for (id, limit) in &value.scopes {
            if id.is_empty() || id.trim() != id {
                return Err("budget scope ids must be nonempty and unpadded".into());
            }
            if !limit.monthly_usd.is_finite() || limit.monthly_usd < 0.0 {
                return Err(format!(
                    "scope {id:?}: monthly_usd must be finite and nonnegative"
                ));
            }
        }
        Ok(value)
    }
}

pub fn budgets_dir(root: &Path) -> PathBuf {
    root.join(".factory/budgets")
}
pub fn catalogue_path(root: &Path) -> PathBuf {
    budgets_dir(root).join("limits.yaml")
}

/// Re-read authored intent, never create it. Missing means no limit;
/// malformed, oversized, symlink or special-file intent is never ignored.
pub fn load(root: &Path) -> Result<Catalogue, String> {
    use std::io::Read;
    let path = catalogue_path(root);
    let metadata = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Catalogue::default()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        Ok(metadata) => metadata,
    };
    if !metadata.is_file() || metadata.len() > MAX_CATALOGUE_BYTES {
        return Err(format!(
            "{}: expected a regular catalogue file of at most 1 MiB",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_CATALOGUE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_CATALOGUE_BYTES {
        return Err(format!("{}: catalogue exceeds 1 MiB", path.display()));
    }
    let raw = std::str::from_utf8(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Catalogue::parse(raw).map_err(|e| format!("{}: {e}", path.display()))
}

pub use factory_assurance::budget::{assess, Assessment, Month, State};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeBudget {
    pub id: String,
    pub scope: String,
    pub path: String,
    /// Parent cards can include spending outside the selected subtree.
    pub relation: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_usd: Option<f64>,
    pub spent: CostRow,
    pub unattributed_runs: u32,
    pub daily: Vec<factory_kernel::DailySpend>,
    pub assessment: Assessment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub catalogue: String,
    pub month: Month,
    pub group_by: CostGroupBy,
    /// Exactly the selected scope's spend; never a sum of overlapping caps.
    pub spend: CostReport,
    pub budgets: Vec<ScopeBudget>,
    pub findings: Vec<String>,
}

/// Authored intent passed down as configuration alongside unchanged L4
/// spend facts. This is not a new L6-produced live fact or status read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub catalogue: Option<Catalogue>,
    pub error: Option<String>,
}

pub use factory_assurance::budget::{within, PolicyCap, PolicyInput};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_limits_are_strict_finite_and_unique_with_no_inferred_default() {
        assert!(Catalogue::default().scopes.is_empty());
        let c = Catalogue::parse("version: 1\nscopes:\n  stable-id: {monthly_usd: 50}\n").unwrap();
        assert_eq!(c.scopes["stable-id"].monthly_usd, 50.0);
        assert_eq!(
            Catalogue::parse(&serde_yaml_ng::to_string(&c).unwrap()).unwrap(),
            c
        );
        for raw in [
            "version: 2",
            "version: 1\nunknown: 1",
            "version: 1\nscopes: {s: {monthly_usd: -1}}",
            "version: 1\nscopes: {s: {monthly_usd: .nan}}",
            "version: 1\nscopes: {s: {monthly_usd: .inf}}",
            "version: 1\nscopes: {s: {monthly_usd: 1, guessed: 2}}",
            "version: 1\nscopes:\n  s: {monthly_usd: 1}\n  s: {monthly_usd: 2}",
        ] {
            assert!(Catalogue::parse(raw).is_err(), "{raw}");
        }
        assert!(Catalogue::parse("version: 1\nscopes: {s: {monthly_usd: 0}}").is_ok());
    }
    #[test]
    fn file_reads_do_not_create_intent_and_bad_intent_is_not_treated_as_absent() {
        let root = std::env::temp_dir().join(format!("factory-budget-{}", uuid::Uuid::new_v4()));
        assert!(load(&root).unwrap().scopes.is_empty());
        assert!(!root.exists());
        std::fs::create_dir_all(budgets_dir(&root)).unwrap();
        std::fs::write(
            catalogue_path(&root),
            "version: 1\nscopes: {s: {monthly_usd: -1}}",
        )
        .unwrap();
        let error = load(&root).unwrap_err();
        assert!(error.contains("limits.yaml") && error.contains("nonnegative"));
        std::fs::write(
            catalogue_path(&root),
            vec![b' '; MAX_CATALOGUE_BYTES as usize + 1],
        )
        .unwrap();
        assert!(load(&root).unwrap_err().contains("1 MiB"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
