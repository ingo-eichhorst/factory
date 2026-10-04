//! The dashboard's tile vocabulary and the config block that arranges it
//! (`#159`, phase 4 of `#150`) -- kept as small and closed as the metric
//! registry (`metrics.rs`) it draws from: a tile names a registry
//! [`crate::metrics::MetricId`] or one of a fixed set of richer view tiles
//! ([`ViewId`]), never a query, a filter or a formula (design §8, the same
//! line `policy.rs` and `metrics.rs` hold).
//!
//! What lives here is the shape and the shape's own validity -- whether a
//! tile is well-formed and whether its `metric` is one the registry knows.
//! Resolving *which* [`DashboardConfig`] applies to a scope is
//! `factory_core::config::Config::dashboard_for_scope`'s job, the same
//! split `role.rs` (what a role is) keeps from `config.rs`'s
//! `roles_for_scope` (which layer wins).

use crate::metrics::{resolve as resolve_metric, MetricError, MetricId};
use serde::{Deserialize, Serialize};

/// The fixed vocabulary of richer, endpoint-backed tiles -- everything a
/// [`Tile`] can show besides a bare registry metric. `kpis` is today's KPI
/// row; every other id reads one existing report or endpoint. Adding one
/// here is adding a card to the catalogue; nothing about the wire shape or
/// validation changes to support it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewId {
    Kpis,
    Throughput,
    OnTheLine,
    ProductionYear,
    ByScope,
    AgentHoursByScope,
    AgentHoursByAgent,
    OccupancyStrip,
    Inbox,
    Compliance,
    Cost,
    ImportantDates,
}

impl ViewId {
    /// Every view id, in the order the catalogue above lists them --
    /// nothing reads this today, but a validation error or a future
    /// catalogue listing wants the closed set spelled out once.
    pub const ALL: [ViewId; 12] = [
        ViewId::Kpis,
        ViewId::Throughput,
        ViewId::OnTheLine,
        ViewId::ProductionYear,
        ViewId::ByScope,
        ViewId::AgentHoursByScope,
        ViewId::AgentHoursByAgent,
        ViewId::OccupancyStrip,
        ViewId::Inbox,
        ViewId::Compliance,
        ViewId::Cost,
        ViewId::ImportantDates,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            ViewId::Kpis => "kpis",
            ViewId::Throughput => "throughput",
            ViewId::OnTheLine => "on_the_line",
            ViewId::ProductionYear => "production_year",
            ViewId::ByScope => "by_scope",
            ViewId::AgentHoursByScope => "agent_hours_by_scope",
            ViewId::AgentHoursByAgent => "agent_hours_by_agent",
            ViewId::OccupancyStrip => "occupancy_strip",
            ViewId::Inbox => "inbox",
            ViewId::Compliance => "compliance",
            ViewId::Cost => "cost",
            ViewId::ImportantDates => "important_dates",
        }
    }
}

impl std::fmt::Display for ViewId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A tile's fixed size on the dashboard's grid -- nothing finer, and no
/// pixel or column count a config file could drift out of step with the
/// UI's own CSS. Wire spelling is its variant name lowercased
/// (`rename_all = "snake_case"` on a single-letter or already-lowercase
/// name is a no-op past lowercasing, so `Xl` becomes `xl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TileSize {
    S,
    M,
    L,
    Xl,
}

impl TileSize {
    pub fn as_str(&self) -> &'static str {
        match self {
            TileSize::S => "s",
            TileSize::M => "m",
            TileSize::L => "l",
            TileSize::Xl => "xl",
        }
    }
}

impl std::fmt::Display for TileSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One tile: exactly one of a registry metric or a catalogue view, sized.
/// `deny_unknown_fields` so a typoed key (`vew:`) is refused rather than
/// silently read as neither `metric` nor `view` -- the same reasoning
/// `PolicyDeclaration` and `MetricMeasure` already give this attribute.
///
/// `metric`/`view` are typed ([`MetricId`], [`ViewId`]) so a syntactically
/// malformed metric id or a view outside the fixed vocabulary is refused at
/// parse time, by serde itself, the same way `Lifetime`, `Sandbox` and
/// `DependencyTransport` already are elsewhere in this crate -- no special
/// casing needed here for "unknown view". A metric id can be syntactically
/// valid and still name nothing the registry has ever heard of (an unbound
/// family, or a plain typo); that is a semantic check the registry alone
/// can answer, so it happens in [`Tile::validate`] instead, exactly the
/// asymmetry `metrics.rs`'s own module doc draws between `MetricId::new`
/// (syntax) and `resolve` (the registry).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metric: Option<MetricId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<ViewId>,
    pub size: TileSize,
}

impl Tile {
    /// `path` names this tile for the error, e.g. `dashboard.tiles[3]` --
    /// `Config::validate`/`validate_instance` build it, prefixed with which
    /// layer (the root, or a named scope) this tile's block belongs to.
    ///
    /// A metric [`MetricError::Unavailable`] passes: the issue is explicit
    /// that a metric the registry knows but cannot compute yet is a valid
    /// tile, the same way an unavailable metric renders its own reason
    /// rather than refusing the whole dashboard.
    pub fn validate(&self, path: &str) -> Result<(), String> {
        match (&self.metric, &self.view) {
            (Some(_), Some(_)) => {
                return Err(format!(
                    "{path} declares both `metric` and `view`; a tile is exactly one of the two"
                ))
            }
            (None, None) => {
                return Err(format!(
                    "{path} declares neither `metric` nor `view`; a tile is exactly one of the two"
                ))
            }
            _ => {}
        }
        if let Some(id) = &self.metric {
            match resolve_metric(id) {
                Ok(_) => {}
                Err(MetricError::Unavailable { .. }) => {}
                Err(MetricError::Unknown(_)) => {
                    return Err(format!("{path}.metric {:?} is not a known metric", id.as_str()));
                }
            }
        }
        Ok(())
    }
}

/// A scope's own dashboard layout: a `dashboard:` block at the instance
/// root's top level, or under a scope's `scope.dashboard` -- see
/// `config.rs`'s `Config::dashboard`/`Scope::dashboard` doc comments for
/// where each may be written, which mirrors `roles:`/`scope.roles` exactly.
///
/// Every holder of this type is `Option<DashboardConfig>` rather than a
/// bare `DashboardConfig` defaulting to empty, so "no `dashboard:` block at
/// all" (inherit) and "a `dashboard:` block is here" (replace whatever was
/// inherited, whole) are distinguishable at every layer: `None` inherits,
/// `Some` replaces. A `Some` with zero tiles is refused at
/// [`DashboardConfig::validate`], though, the same way an empty workflow
/// or an empty quality attribute list is -- a dashboard that draws nothing
/// is not a smaller valid layout, and the block to remove to fall back to
/// the inherited one is right there in the error.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardConfig {
    #[serde(default)]
    pub tiles: Vec<Tile>,
}

impl DashboardConfig {
    /// `path` names this whole block for an error, e.g. `dashboard` at the
    /// root or `scope "demo-app" dashboard` for a nested one -- passed
    /// through to each tile with its own index appended.
    pub fn validate(&self, path: &str) -> Result<(), String> {
        if self.tiles.is_empty() {
            return Err(format!(
                "{path} needs at least one tile; remove the `dashboard:` block to inherit"
            ));
        }
        for (i, tile) in self.tiles.iter().enumerate() {
            tile.validate(&format!("{path}.tiles[{i}]"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(metric: Option<&str>, view: Option<ViewId>, size: TileSize) -> Tile {
        Tile {
            metric: metric.map(|m| MetricId::new(m).unwrap()),
            view,
            size,
        }
    }

    #[test]
    fn a_tile_names_exactly_one_of_metric_or_view() {
        let both = tile(Some("throughput_week"), Some(ViewId::Kpis), TileSize::S);
        let e = both.validate("dashboard.tiles[0]").unwrap_err();
        assert!(e.contains("dashboard.tiles[0]"), "{e}");
        assert!(e.contains("exactly one"), "{e}");

        let neither = Tile { metric: None, view: None, size: TileSize::S };
        let e = neither.validate("dashboard.tiles[1]").unwrap_err();
        assert!(e.contains("exactly one"), "{e}");
    }

    #[test]
    fn an_unknown_metric_id_is_refused_and_a_bound_family_is_accepted() {
        let unknown = tile(Some("not_a_metric"), None, TileSize::M);
        let e = unknown.validate("dashboard.tiles[2]").unwrap_err();
        assert!(e.contains("dashboard.tiles[2].metric"), "{e}");
        assert!(e.contains("not_a_metric"), "{e}");

        let bound = tile(Some("compliance.cra"), None, TileSize::S);
        bound.validate("dashboard.tiles[3]").unwrap();

        let view = tile(None, Some(ViewId::Cost), TileSize::L);
        view.validate("dashboard.tiles[4]").unwrap();
    }

    #[test]
    fn an_unbound_family_is_not_a_known_metric() {
        let unbound = tile(Some("compliance"), None, TileSize::S);
        let e = unbound.validate("dashboard.tiles[5]").unwrap_err();
        assert!(e.contains("compliance"), "{e}");
    }

    #[test]
    fn an_empty_tile_list_is_refused_naming_the_block_and_the_fix() {
        let e = DashboardConfig::default().validate("dashboard").unwrap_err();
        assert!(e.contains("dashboard"), "{e}");
        assert!(e.contains("at least one tile"), "{e}");
        assert!(e.contains("remove the `dashboard:` block to inherit"), "{e}");

        let e = DashboardConfig::default()
            .validate(r#"scope "demo" dashboard"#)
            .unwrap_err();
        assert!(e.contains(r#"scope "demo" dashboard needs at least one tile"#), "{e}");
    }

    #[test]
    fn wire_shapes_match_the_fixed_vocabulary() {
        let t = tile(None, Some(ViewId::OnTheLine), TileSize::Xl);
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, r#"{"view":"on_the_line","size":"xl"}"#);

        let m = tile(Some("throughput_week"), None, TileSize::S);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, r#"{"metric":"throughput_week","size":"s"}"#);
    }

    #[test]
    fn a_typoed_key_is_refused_rather_than_silently_dropped() {
        let e = serde_yaml_ng::from_str::<Tile>("vew: kpis\nsize: s\n").unwrap_err();
        assert!(e.to_string().contains("unknown field"), "{e}");
    }
}
