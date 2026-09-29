//! The fact vocabulary behind `policy::Evidence` (#193, phase 2): a `Level`
//! marker for each of Factory's six levels, the `Fact` trait that names a
//! fact's producer, and the handful of fact types plain enough to move here
//! unchanged. No behaviour changes: `policy::Evidence` still has the same
//! fields, `evaluate` still reads them the same way, and every moved type
//! is re-exported from where it used to live (`policy::DaemonFact`,
//! `backup::BackupFact`, `backup::VerifySummary`), the same way phase 1 kept
//! `policy::Duration` working.
//!
//! ## What moved, and what did not
//!
//! A fact moves here when it is plain data all the way down -- no field
//! whose type is itself owned by a level's own module. Several of
//! `Evidence`'s facts are not that: `TaskFact` carries a `RunFact` typed by
//! `crate::run::RunStatus` (L4's own status enum, with its `Verifying`
//! variant tied to `#118`'s control-plan semantics); `WorkflowFact` carries
//! `crate::workflow::WorkflowRunStatus` (L4); `GateFact` carries
//! `crate::bench::Verdict` (L5); `AgentFact` carries `crate::role::Grant`
//! (the whole cross-level authorization vocabulary `access.rs` matches
//! exhaustively -- the biggest of the four, and the least like a single
//! level's own fact); `dependencies::DependenciesFact` carries
//! `dependencies::Severity` (L2); `dependencies::ExploitedFinding` carries
//! `dependencies::{Attachment, AffectedComponent, LifecycleState}` (L2); and
//! `intake::ConfirmedSecurityReport` carries `intake::IntakeSource` (L4,
//! with its own `identity()` method). Moving any of those here now would
//! mean moving a producing level's status/identity vocabulary into L0 ahead
//! of the crate split that is supposed to draw that boundary (phase 4) --
//! exactly the "L0 becomes the next hub" the issue's guardrails warn against
//! (policy.rs is 4,082 lines and counting; L0 must not grow the same way).
//! Applying one rule to all seven kept this phase from quietly moving the
//! four that happened to look small (`DaemonFact`, `BackupFact`) while
//! leaving the rest for an inconsistent reason.
//!
//! Each of those seven still implements [`Fact`] and still names its
//! producer -- Rust's orphan rule allows a foreign trait (`Fact`, defined
//! here) on a local type (`TaskFact`, defined in `factory-core`), so the
//! `impl` sits right beside each type instead of waiting for a future move.
//! [`FACT_CATALOGUE`] lists all eleven either way, moved or not, with the
//! producer, the readers, and -- for the seven still elsewhere -- why.
//!
//! What is genuinely new here rather than moved: [`SecretsPresence`], a name
//! for the shape `Evidence::secrets` already was (`BTreeMap<String, bool>`,
//! keyed by a `KNOWN_SECRETS_LOCATIONS` entry) so it too can implement
//! [`Fact`] -- a type alias, not a wrapper, so nothing that builds or reads
//! an `Evidence` changes at all.

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ================================================================== levels

/// A level in Factory's six-level ladder (the README's L1-L6), as a
/// zero-sized marker -- never constructed, only named as a [`Fact`]'s
/// `Producer`. Nothing about a level's behaviour lives here, only its
/// identity: the marker is what lets the compiler tell L1's facts apart
/// from L6's once `Below`/`Provide` (phases 3-4) start bounding who may read
/// what.
pub trait Level {}

/// Infrastructure: the daemon's own process, configuration and mounted
/// interfaces (`factory infra`, the L1 Infrastructure page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L1;
impl Level for L1 {}

/// Environment: secrets presence and the dependency/SBOM inventory (the L2
/// Environment page's Secrets and Dependencies tabs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L2;
impl Level for L2 {}

/// Agent runtime: the roster, its roles, sandboxes and grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L3;
impl Level for L3 {}

/// Process: tasks, runs, workflows and intake (the L4 Process page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L4;
impl Level for L4 {}

/// Improvement: quality attributes, benchmarks and datasets (the L5
/// Improvement page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L5;
impl Level for L5 {}

/// Policy and Direction: controls, goals and scenarios (the L6 Policy and
/// Direction pages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct L6;
impl Level for L6 {}

// ==================================================================== fact

/// A fact type one level's own state produces, for a level above it to read.
/// Phase 2 only catalogues the vocabulary and names each fact's producer;
/// the fact *port* -- a `Below` bound, a `Provide<F>` impl, a `Facts<R>`
/// handle a reader is given at startup -- is phases 3-4's work, not this
/// trait's. Until then, `policy::Evidence` still gathers every fact the way
/// `policies/mod.rs` always has.
pub trait Fact: Serialize + DeserializeOwned {
    /// The level that derives this fact fresh, live, on every read (ADR
    /// 0004: "status is computed on every read") -- never the level that
    /// merely reads it, and never stored as of-record evidence (that is
    /// `policy::EvidenceRef`'s question, deferred to its own later ADR).
    type Producer: Level;
}

// ============================================================ daemon (L1)

/// The `daemon` check's own configuration facts, resolved once per report
/// by the engine (`Engine::daemon_facts`) rather than per scope -- the
/// daemon's own configuration is the same wherever it is asked from, the
/// same reasoning `Evidence::gates` already uses for datasets.
///
/// Named `DaemonConfigFact`, not `DaemonFact`: `factory_core::protocol`
/// already has a `DaemonFacts` (plural -- the `factory infra`/`GET
/// /api/infrastructure` wire payload, a different type for a different
/// purpose, the daemon's whole runtime shape rather than this narrower
/// policy vocabulary). The triage that planned this phase flagged the two
/// names as confusable and left resolving it to phase 2. `policy.rs`
/// re-exports this type as `DaemonFact`, so no L6 caller's source changes;
/// the wire form was never affected either way, since serde never
/// serializes a Rust type's own name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonConfigFact {
    pub foreman_enabled: bool,
    /// `None` when it could not be determined -- a mounted `http`
    /// interface whose `bind` does not parse as a socket address.
    /// `Some(true)` covers both "every mounted `http` interface binds to a
    /// loopback address" and "the daemon mounts no `http` interface at
    /// all": nothing is exposed beyond loopback either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_loopback_only: Option<bool>,
    pub power_assertion: bool,
}

impl Fact for DaemonConfigFact {
    type Producer = L1;
}

/// The `daemon` check's whole fixed vocabulary -- everything else
/// `DaemonConfig` carries is either not a policy-relevant fact or ambiguous
/// enough that "does it hold" would be a guess (ADR 0004's "facts only, no
/// heuristics"). `policy::load_all` checks a `fact` string against this at
/// parse time, so an authoring mistake shows up on the catalogue, not only
/// once a report is evaluated.
///
/// - `foreman_enabled` -- `daemon.foreman.enabled`.
/// - `http_loopback_only` -- every `http` interface the daemon mounts binds
///   to a loopback address, or none is mounted at all.
/// - `power_assertion` -- `daemon.power_assertion`.
/// - `backup_recent` -- `#154`: the newest backup is within its schedule (or
///   the unscheduled yardstick) plus grace -- [`BackupFact::recent`].
/// - `backup_offsite` -- `#154`: the destination is not on the same device
///   as the instance root -- [`BackupFact::offsite`].
/// - `backup_verified` -- `#154`: the newest verification of a snapshot
///   still in the destination passed, within 30 days --
///   [`BackupFact::verified`].
pub const KNOWN_DAEMON_FACTS: &[&str] =
    &["foreman_enabled", "http_loopback_only", "power_assertion", "backup_recent", "backup_offsite", "backup_verified"];

// ============================================================ backup (L1)

/// One verification of a snapshot still present in a backup destination --
/// carried by [`BackupFact::last_verified`] whatever
/// [`BackupFact::verified`] itself decided, so a reader can say why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifySummary {
    pub snapshot: String,
    pub at: DateTime<Utc>,
    pub ok: bool,
}

impl Fact for VerifySummary {
    type Producer = L1;
}

/// The one L1 fact a policy `daemon` check or a registry metric reads about
/// backups (`#154`) -- computed by `Engine::backup_fact(now)` in
/// `factory-daemon`'s `backup` module, off the same captured state
/// `backup_report` reads, via `factory_core::backup::resolve_backup_fact`.
/// Plain data: nothing here reads a clock, a store or a disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupFact {
    /// The instant this fact was derived -- the same "now" a policy report
    /// or a metric call reads every other fact against, and a metric
    /// value's own `as_of`.
    pub at: DateTime<Utc>,
    /// Whether `infrastructure.backup` names a destination at all.
    pub configured: bool,
    /// The newest archive's timestamp -- `None` before the first backup, or
    /// when the destination cannot be listed at all.
    pub newest: Option<DateTime<Utc>>,
    /// Within its schedule (or the unscheduled yardstick) plus grace.
    /// `None` when the destination is missing or unmounted: even "no" would
    /// be a guess about an archive nobody can currently list.
    pub recent: Option<bool>,
    /// The destination is not on the same device as the instance root.
    /// `None` when that cannot be determined -- the destination is
    /// missing, or its device could not be read.
    pub offsite: Option<bool>,
    /// The newest verification of a snapshot still in the destination
    /// passed, within 30 days of `at`. `None` when the destination is
    /// missing or unmounted.
    pub verified: Option<bool>,
    /// The same verification `verified` is read from, whatever it decided
    /// -- present even when `verified` is `Some(false)` (failed, or too
    /// old), so a reader can say why.
    pub last_verified: Option<VerifySummary>,
}

impl Fact for BackupFact {
    type Producer = L1;
}

// =========================================================== secrets (L2)

/// Whether each of [`KNOWN_SECRETS_LOCATIONS`] a `secrets` check names is
/// present, keyed by that location id -- a scoped slice of the L2 Secrets
/// tab's own inventory (`Engine::credential_inventory`). A location absent
/// from this map was never asked about, not confirmed absent.
///
/// A type alias, not a wrapper: `Evidence::secrets` was already exactly this
/// shape (`BTreeMap<String, bool>`), so naming it here to give it a [`Fact`]
/// impl changes nothing about how it is built or read -- no `.0`, no
/// `From`/`Into`, no construction-site churn. The richer shape the design
/// sketch's `CredentialInventory` imagines (one entry per location, still no
/// value) is phase 3's to build, once something actually reads it through a
/// fact port.
pub type SecretsPresence = BTreeMap<String, bool>;

impl Fact for SecretsPresence {
    type Producer = L2;
}

/// The `secrets` check's fixed vocabulary -- exactly the locations the L2
/// Secrets tab already reports on (`Engine::credential_inventory`): the
/// five machine-wide locations every scope shares (an agent runs as the
/// daemon's owner, so these are the same regardless of scope) plus a
/// scope's own `.env`.
pub const KNOWN_SECRETS_LOCATIONS: &[&str] = &["anthropic", "github", "aws", "netrc", "ssh", "scope_env"];

// =============================================================== catalogue

/// One row of [`FACT_CATALOGUE`]: a fact's name, the level that produces it,
/// which levels read it today (through `policy::Evidence`, until phases 3-4
/// add real fact ports), whether the type itself lives here yet, and -- for
/// the ones that do not -- why.
#[derive(Debug, Clone, Copy)]
pub struct FactCatalogueEntry {
    pub fact: &'static str,
    pub producer: &'static str,
    pub readers: &'static [&'static str],
    pub lives_in_kernel: bool,
    pub note: &'static str,
}

/// Every fact behind `policy::Evidence`, whatever module its type actually
/// lives in -- the catalogue the issue's guardrails ask for ("a catalogue
/// test lists fact, producer and readers"). `readers` is documentation, not
/// a compiled reference: phases 3-4's `Facts<Reader>::get` is what will make
/// a wrong reader a compile error instead of a comment. See this module's
/// own doc comment for why the four still in `policy.rs`, the two in
/// `dependencies.rs` and the one in `intake.rs` did not move.
pub const FACT_CATALOGUE: &[FactCatalogueEntry] = &[
    FactCatalogueEntry {
        fact: "DaemonConfigFact",
        producer: "L1",
        readers: &["L6 policy (the `daemon` check)"],
        lives_in_kernel: true,
        note: "moved whole in phase 2: no field's type is owned by another level's module",
    },
    FactCatalogueEntry {
        fact: "BackupFact",
        producer: "L1",
        readers: &["L6 policy (the `daemon` check's backup_* names)", "L6 metrics (goals/scenario drivers)"],
        lives_in_kernel: true,
        note: "moved whole in phase 2, with VerifySummary",
    },
    FactCatalogueEntry {
        fact: "SecretsPresence",
        producer: "L2",
        readers: &["L6 policy (the `secrets` check)"],
        lives_in_kernel: true,
        note: "newly named in phase 2 as a type alias over the map `Evidence::secrets` already was",
    },
    FactCatalogueEntry {
        fact: "DependenciesFact",
        producer: "L2",
        readers: &["L6 policy (the `dependencies` check)"],
        lives_in_kernel: false,
        note: "stays in factory_core::dependencies: carries Severity, L2's own status enum (from_cyclonedx); \
               moving it means moving L2's status vocabulary into L0 ahead of phase 4's crate split",
    },
    FactCatalogueEntry {
        fact: "ExploitedFinding",
        producer: "L2",
        readers: &["L6 policy (the CRA Article 14 reporting clock, `#157`)"],
        lives_in_kernel: false,
        note: "stays in factory_core::dependencies: carries Attachment/AffectedComponent/LifecycleState, \
               L2's own SBOM-lifecycle vocabulary",
    },
    FactCatalogueEntry {
        fact: "TaskFact",
        producer: "L4",
        readers: &["L6 policy (the `task` check)"],
        lives_in_kernel: false,
        note: "stays in factory_core::policy: RunFact carries crate::run::RunStatus, L4's own status enum",
    },
    FactCatalogueEntry {
        fact: "WorkflowFact",
        producer: "L4",
        readers: &["L6 policy (the `workflow` check)"],
        lives_in_kernel: false,
        note: "stays in factory_core::policy: WorkflowRunFact carries crate::workflow::WorkflowRunStatus, L4's own status enum",
    },
    FactCatalogueEntry {
        fact: "ConfirmedSecurityReport",
        producer: "L4",
        readers: &["L6 policy (the CRA Article 14 reporting clock, `#157`)"],
        lives_in_kernel: false,
        note: "stays in factory_core::intake: carries IntakeSource, which has its own identity() method",
    },
    FactCatalogueEntry {
        fact: "GateFact",
        producer: "L5",
        readers: &["L6 policy (the `gate` check)"],
        lives_in_kernel: false,
        note: "stays in factory_core::policy: GateCase carries crate::bench::Verdict, L5's own status enum",
    },
    FactCatalogueEntry {
        fact: "AgentFact",
        producer: "L3",
        readers: &["L6 policy (the `roles`/`sandbox` checks)"],
        lives_in_kernel: false,
        note: "stays in factory_core::policy: carries crate::role::Grant, the whole cross-level authorization \
               vocabulary access.rs matches exhaustively -- the least single-level of the seven",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned to the exact shape `policy::DaemonFact` serialized as before
    /// this phase -- field names and the `http_loopback_only` `Option`
    /// omitted when `None`, unaffected by the Rust-side rename to
    /// `DaemonConfigFact`.
    #[test]
    fn daemon_config_fact_serializes_exactly_as_daemon_fact_used_to() {
        let with_bind = DaemonConfigFact { foreman_enabled: true, http_loopback_only: Some(false), power_assertion: true };
        assert_eq!(
            serde_json::to_string(&with_bind).unwrap(),
            r#"{"foreman_enabled":true,"http_loopback_only":false,"power_assertion":true}"#
        );

        let never_gathered_bind = DaemonConfigFact { foreman_enabled: false, http_loopback_only: None, power_assertion: false };
        assert_eq!(
            serde_json::to_string(&never_gathered_bind).unwrap(),
            r#"{"foreman_enabled":false,"power_assertion":false}"#
        );
    }

    #[test]
    fn backup_fact_round_trips_through_json_unchanged() {
        let fact = BackupFact {
            at: "2026-09-29T00:00:00Z".parse().unwrap(),
            configured: true,
            newest: Some("2026-09-28T00:00:00Z".parse().unwrap()),
            recent: Some(true),
            offsite: Some(false),
            verified: Some(true),
            last_verified: Some(VerifySummary { snapshot: "snap-1".into(), at: "2026-09-28T01:00:00Z".parse().unwrap(), ok: true }),
        };
        let json = serde_json::to_string(&fact).unwrap();
        let back: BackupFact = serde_json::from_str(&json).unwrap();
        assert_eq!(fact, back);
    }

    #[test]
    fn secrets_presence_is_exactly_a_string_to_bool_map_on_the_wire() {
        let mut presence: SecretsPresence = SecretsPresence::new();
        presence.insert("github".to_string(), true);
        presence.insert("scope_env".to_string(), false);
        assert_eq!(serde_json::to_string(&presence).unwrap(), r#"{"github":true,"scope_env":false}"#);
    }

    #[test]
    fn known_daemon_facts_and_secrets_locations_are_unchanged() {
        assert_eq!(
            KNOWN_DAEMON_FACTS,
            &["foreman_enabled", "http_loopback_only", "power_assertion", "backup_recent", "backup_offsite", "backup_verified"]
        );
        assert_eq!(KNOWN_SECRETS_LOCATIONS, &["anthropic", "github", "aws", "netrc", "ssh", "scope_env"]);
    }
}
