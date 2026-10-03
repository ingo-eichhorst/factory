//! `#107` (L6 Quality attributes): which qualities matter for each scope,
//! how much, what they trade off against, and whether they are being met --
//! measured, never claimed. Same shape as `policy.rs`, `goals.rs` and
//! `scenario.rs`: authored YAML, re-read on every request, never written by
//! Factory, a [`Finding`] for anything an author got wrong rather than a hard
//! failure that takes the rest of the directory down with it.
//!
//! This slice ("quality-core") is pure `factory-core` only -- the compiled-in
//! catalogue, the profile loader, the add-or-tighten merge down the scope
//! chain, and the evaluator. No engine, protocol, access, CLI or HTTP wiring,
//! and no I/O anywhere in this module except [`load`] itself.
//!
//! ## The layers, and which method each one borrows
//!
//! - **Vocabulary:** ISO/IEC 25010:2023's nine characteristics and their
//!   sub-characteristics, plus ISO/IEC 25059:2023's AI additions as an
//!   optional pack -- [`CATALOGUE`], compiled in like the role presets, so
//!   an attribute id is checked against a fixed list rather than invented.
//! - **Declaration:** an ATAM utility tree per scope -- attributes ranked
//!   (importance, difficulty) as H/M/L only, at most seven of them. Authored
//!   as a *profile*, `<root>/.factory/quality/<profile>.yaml`, and bound to
//!   scopes by the config chain (`Config::quality_chain_for_scope`).
//! - **Specification:** each attribute carries SEI six-part scenarios
//!   (source, stimulus, artifact, environment, response, response measure),
//!   labelled usage or change. With no response measure a scenario is a
//!   draft, and a draft is never met.
//! - **Verification:** a response measure is either a threshold on a
//!   registry metric (`crate::metrics`, the one Goals and Scenarios read) or
//!   one of Policy's own check kinds (`policy::Check`), judged by
//!   `policy::evaluate` itself. There is no third registry and no second
//!   evidence path.
//!
//! ## Enforces nothing, scores nothing
//!
//! A quality attribute starts, stops and blocks nothing -- design §8's
//! deferral of a rule language holds here exactly as it does for policies.
//! And there is no aggregate: an attribute's status is the *worst* of its
//! scenarios ([`ScenarioStatus::worst`]), and nothing here ever averages
//! attributes into a scope or company score. An average is how a failing
//! H-importance attribute hides behind three green L ones.
//!
//! ## Storage
//!
//! `<root>/.factory/quality/<profile>.yaml` -- authored content like
//! `.factory/policies/` and `.factory/goals/`. A profile's id *is* its file
//! stem; unlike a policy catalogue or a goals cycle, the file does not repeat
//! it, so there is nothing to mismatch. [`load`] reads the whole directory
//! in the same shape `policy::load_all` and `goals::load` use: a file that
//! fails to parse is a [`Finding`] naming it and every other file still
//! loads; a missing directory is empty, not an error. An entry inside a file
//! that can never be placed -- an attribute id not in [`CATALOGUE`], a
//! repeated id -- is dropped from what `load` returns, with a finding, so
//! nothing downstream has to re-check it.
//!
//! ## Deviation from the issue text
//!
//! The issue's example spells a check measure `check: { task: quality-gate,
//! max_age: 7d }`. `policy::Check` is internally tagged, so the spelling
//! Policy's own catalogues already use -- and the one reused here verbatim --
//! is `{ check: task, task: quality-gate, max_age: 7d }`. The example's
//! short attribute ids (`performance.time-behaviour`) are written out in
//! full (`performance-efficiency.time-behaviour`), since the catalogue has
//! exactly one spelling of each. And a metric measure's `window: 7d` is
//! spelled `max_age: 7d`: it is a freshness bound on the value, exactly what
//! a policy check's `max_age` is, not the period a metric is computed over
//! (see [`MetricMeasure`]) -- so it takes that name rather than one that
//! reads as the other thing.

use crate::dataset::is_slug;
use crate::metrics::{self, MetricError, MetricId, MetricValue};
use crate::policy::{self, Applied, Check, ControlRef, Evidence, EvidenceRef, Kind, StatusKind};
use chrono::{DateTime, Utc};
use factory_kernel::Duration;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

// ============================================================== catalogue

/// Which standard a catalogue entry comes from. Everything is ISO/IEC
/// 25010:2023 except the AI pack, ISO/IEC 25059:2023's additions -- optional
/// in the sense that nothing makes a profile use them, and marked so a UI
/// can say which of a scope's attributes came from where.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Standard {
    #[serde(rename = "iso-25010")]
    Iso25010,
    #[serde(rename = "iso-25059")]
    Iso25059,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SubCharacteristic {
    pub id: &'static str,
    pub title: &'static str,
    pub standard: Standard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Characteristic {
    pub id: &'static str,
    pub title: &'static str,
    pub subs: &'static [SubCharacteristic],
}

const fn sub(id: &'static str, title: &'static str) -> SubCharacteristic {
    SubCharacteristic { id, title, standard: Standard::Iso25010 }
}

const fn ai(id: &'static str, title: &'static str) -> SubCharacteristic {
    SubCharacteristic { id, title, standard: Standard::Iso25059 }
}

/// ISO/IEC 25010:2023's nine characteristics, in the standard's own order,
/// each with its sub-characteristics -- and, folded in under the
/// characteristic each extends, ISO/IEC 25059:2023's five AI additions
/// (`standard: Iso25059`). An attribute id is either a characteristic's
/// `id` alone or `<characteristic>.<sub-characteristic>`; see [`lookup`].
pub const CATALOGUE: &[Characteristic] = &[
    Characteristic {
        id: "functional-suitability",
        title: "Functional suitability",
        subs: &[
            sub("functional-completeness", "Functional completeness"),
            sub("functional-correctness", "Functional correctness"),
            sub("functional-appropriateness", "Functional appropriateness"),
            ai("functional-adaptability", "Functional adaptability"),
        ],
    },
    Characteristic {
        id: "performance-efficiency",
        title: "Performance efficiency",
        subs: &[
            sub("time-behaviour", "Time behaviour"),
            sub("resource-utilization", "Resource utilization"),
            sub("capacity", "Capacity"),
        ],
    },
    Characteristic {
        id: "compatibility",
        title: "Compatibility",
        subs: &[sub("co-existence", "Co-existence"), sub("interoperability", "Interoperability")],
    },
    Characteristic {
        id: "interaction-capability",
        title: "Interaction capability",
        subs: &[
            sub("appropriateness-recognizability", "Appropriateness recognizability"),
            sub("learnability", "Learnability"),
            sub("operability", "Operability"),
            sub("user-error-protection", "User error protection"),
            sub("user-engagement", "User engagement"),
            sub("inclusivity", "Inclusivity"),
            sub("user-assistance", "User assistance"),
            sub("self-descriptiveness", "Self-descriptiveness"),
            ai("user-controllability", "User controllability"),
            ai("transparency", "Transparency"),
        ],
    },
    Characteristic {
        id: "reliability",
        title: "Reliability",
        subs: &[
            sub("faultlessness", "Faultlessness"),
            sub("availability", "Availability"),
            sub("fault-tolerance", "Fault tolerance"),
            sub("recoverability", "Recoverability"),
            ai("robustness", "Robustness"),
        ],
    },
    Characteristic {
        id: "security",
        title: "Security",
        subs: &[
            sub("confidentiality", "Confidentiality"),
            sub("integrity", "Integrity"),
            sub("non-repudiation", "Non-repudiation"),
            sub("accountability", "Accountability"),
            sub("authenticity", "Authenticity"),
            sub("resistance", "Resistance"),
            ai("intervenability", "Intervenability"),
        ],
    },
    Characteristic {
        id: "maintainability",
        title: "Maintainability",
        subs: &[
            sub("modularity", "Modularity"),
            sub("reusability", "Reusability"),
            sub("analysability", "Analysability"),
            sub("modifiability", "Modifiability"),
            sub("testability", "Testability"),
        ],
    },
    Characteristic {
        id: "flexibility",
        title: "Flexibility",
        subs: &[
            sub("adaptability", "Adaptability"),
            sub("scalability", "Scalability"),
            sub("installability", "Installability"),
            sub("replaceability", "Replaceability"),
        ],
    },
    Characteristic {
        id: "safety",
        title: "Safety",
        subs: &[
            sub("operational-constraint", "Operational constraint"),
            sub("risk-identification", "Risk identification"),
            sub("fail-safe", "Fail safe"),
            sub("hazard-warning", "Hazard warning"),
            sub("safe-integration", "Safe integration"),
        ],
    },
];

/// What an attribute id names in [`CATALOGUE`]: always a characteristic,
/// and a sub-characteristic of *that* characteristic when the id has a
/// second segment. `security.robustness` is `None` -- both halves exist,
/// but robustness is reliability's, not security's.
pub fn lookup(id: &str) -> Option<(&'static Characteristic, Option<&'static SubCharacteristic>)> {
    let (characteristic, sub) = match id.split_once('.') {
        Some((c, s)) => (c, Some(s)),
        None => (id, None),
    };
    let found = CATALOGUE.iter().find(|c| c.id == characteristic)?;
    match sub {
        None => Some((found, None)),
        Some(s) => found.subs.iter().find(|x| x.id == s).map(|x| (found, Some(x))),
    }
}

/// The characteristic an attribute id sits under -- its first segment, and
/// the column a company heatmap files it in, whichever sub-characteristic a
/// scope chose.
pub fn characteristic_of(id: &str) -> &str {
    id.split_once('.').map_or(id, |(c, _)| c)
}

// ================================================================ profile

/// ATAM's own three-point scale, for importance and difficulty alike --
/// never finer, because people "cannot reliably make finer distinctions".
/// Declared low to high, so the derived `Ord` is the ranking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Level {
    #[serde(rename = "L")]
    Low,
    #[serde(rename = "M")]
    Medium,
    #[serde(rename = "H")]
    High,
}

/// arc42's split: a *usage* scenario is the system running, a *change*
/// scenario is someone changing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioKind {
    Usage,
    Change,
}

/// A continual response measure: a registry metric held to a threshold.
/// `above` and `below` are both inclusive -- a value sitting exactly on the
/// bound still meets it -- and a measure may carry both, a band (one with
/// `above` over `below` can never be met, and is a finding).
///
/// `max_age` is freshness, the same word and the same rule as a policy
/// check's: how old the data behind the value (`MetricValue::as_of`) may be
/// before the scenario reads `stale` rather than `met`; with none, any
/// computed value is current. It is *not* the period the metric is computed
/// over -- every registry metric carries its own fixed one
/// (`first_pass_yield` is always the trailing 28 days), and nothing here can
/// change it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricMeasure {
    pub metric: MetricId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age: Option<Duration>,
}

/// A scenario's response measure: a metric threshold (continual, SRE-style)
/// or one of Policy's check kinds (triggered -- a fitness-function task's
/// newest run, a bench gate, an attestation), written exactly as a policy
/// catalogue writes one. See the module doc comment for the spelling.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Measure {
    Metric(MetricMeasure),
    Check(Check),
}

/// By hand rather than `#[serde(untagged)]`, whose only error is "data did
/// not match any variant" -- useless to someone who misspelled `max_age`.
/// The key that is present says which shape was meant, and that shape's own
/// error comes back.
impl<'de> Deserialize<'de> for Measure {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_yaml_ng::Value::deserialize(deserializer)?;
        let has = |key: &str| {
            value
                .as_mapping()
                .is_some_and(|m| m.contains_key(serde_yaml_ng::Value::String(key.into())))
        };
        match (has("metric"), has("check")) {
            (true, false) => serde_yaml_ng::from_value(value).map(Measure::Metric).map_err(D::Error::custom),
            (false, true) => serde_yaml_ng::from_value(value).map(Measure::Check).map_err(D::Error::custom),
            (true, true) => Err(D::Error::custom("a measure names `metric:` or `check:`, not both")),
            (false, false) => Err(D::Error::custom(
                "a measure names either `metric:` (with `above:`/`below:`) or `check:` (a policy check kind)",
            )),
        }
    }
}

/// An SEI quality-attribute scenario. Every part but `id` is optional, so a
/// half-written one still loads -- and with no `measure` it is a draft,
/// which is what it honestly is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QualityScenario {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ScenarioKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stimulus: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measure: Option<Measure>,
}

/// One node of the utility tree: a catalogue id, ranked, with its scenarios.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attribute {
    pub id: String,
    pub importance: Level,
    pub difficulty: Level,
    #[serde(default)]
    pub scenarios: Vec<QualityScenario>,
    /// The steps work of a given category must pass for this attribute
    /// (`#118`) -- the same `requires:` a policy control carries, folded
    /// into the same control plan (`control_plan::resolve`, via
    /// [`requirements_of`]). A descendant may add requirements to an
    /// inherited attribute, never remove one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<crate::control_plan::Requirement>,
}

/// An ATAM trade-off point: one design decision that is a sensitivity point
/// for two attributes at once. `decision` is a link -- typically a knowledge
/// page -- to where the rationale is written down.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tradeoff {
    pub between: [String; 2],
    pub point: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
}

/// One quality profile, exactly as authored at
/// `<root>/.factory/quality/<profile>.yaml` (its id is the file stem).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub attributes: Vec<Attribute>,
    #[serde(default)]
    pub tradeoffs: Vec<Tradeoff>,
}

// ============================================================== findings

/// The most attributes one scope may declare, ATAM's own ceiling: past this
/// a utility tree stops saying what matters *most*.
pub const MAX_ATTRIBUTES: usize = 7;

/// One of the things [`load`] or [`applicable`] checks for. Never stops
/// another file, or another part of the same file, from loading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    ParseFailed,
    /// A profile's file stem or a scenario id is not `dataset::is_slug`-shaped.
    /// Both are later named from outside the file -- a config's `quality:`
    /// list, a `quality=<scope>/<attribute>/<scenario>` task label.
    BadIdShape,
    /// An attribute or trade-off names an id [`lookup`] does not know. An
    /// unknown attribute is dropped from the loaded profile.
    UnknownAttribute,
    /// An attribute id repeated within one profile, or a scenario id
    /// repeated within one attribute. The first one is kept.
    DuplicateId,
    /// A metric measure names something `metrics::resolve` has never heard of.
    UnknownMetric,
    /// A metric measure names a metric `metrics::resolve` knows but cannot
    /// compute yet. None is today -- `unit_cost` was, until #117 gave runs
    /// their usage.
    UnavailableMetric,
    /// A metric measure with neither `above` nor `below`: no response
    /// measure at all, so the scenario can only ever be a draft.
    MissingThreshold,
    /// A metric measure bound that is not a finite number (`.nan`, `.inf`).
    /// The measure is dropped, so the scenario reads as the draft it is.
    NonFiniteThreshold,
    /// A metric measure whose `above` is over its `below` -- in one file, or
    /// once a descendant's tightening met an ancestor's. Nothing can be met.
    ImpossibleBand,
    /// A `daemon` check names a fact outside `policy::KNOWN_DAEMON_FACTS`.
    UnknownDaemonFact,
    /// A `secrets` check names a location outside
    /// `policy::KNOWN_SECRETS_LOCATIONS`.
    UnknownSecretsLocation,
    /// A trade-off between an attribute and itself.
    BadTradeoff,
    /// A metric measure names a `quality.<characteristic>` metric -- one
    /// computed from quality scenarios' own statuses, so judging a scenario
    /// by it would be circular ([`is_quality_metric`]). Always `no_data`.
    SelfReferentialMetric,
    /// A `task`/`workflow` check measure's name matches more than one task
    /// title or workflow name in the scope -- `policy::evidence_findings`'
    /// own `ambiguous_check_target`, carried over by `factory-daemon`, which
    /// is the one that gathers the evidence it is found in.
    AmbiguousCheckTarget,
    /// A `knowledge` check with no `tag:` under a dotted attribute id. Its
    /// default tag would be `control/quality/<attribute>/<scenario>`, and a
    /// knowledge tag cannot contain `.` (`knowledge::is_tag_char`), so no
    /// page could ever carry it -- the scenario could never be met.
    UntaggedKnowledgeCheck,
    /// A config `quality:` list names a profile with no file, or one whose
    /// file failed to parse.
    MissingProfile,
    /// A descendant tried to lower an inherited importance, lower an
    /// `above`, raise a `below`, or lengthen a `max_age`. The
    /// inherited, stricter value is kept.
    Loosening,
    /// A `requires:` entry that cannot do what it says -- see
    /// `control_plan::Requirement::problems`. Kept, never dropped.
    BadRequirement,
    /// A descendant restated something that is neither an addition nor a
    /// tightening -- a different metric, a different check, different
    /// scenario text. The inherited one is kept.
    ConflictingOverride,
    /// A scope's merged tree has more than [`MAX_ATTRIBUTES`] attributes.
    TooManyAttributes,
    /// An H-importance attribute none of whose scenarios has a measure.
    UnmeasuredHighImportance,
    /// A trade-off names an attribute the scope's merged tree does not declare.
    UndeclaredTradeoffAttribute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: FindingKind,
    /// The profile file ([`load`]) or the scope ([`applicable`]) the finding
    /// is about -- "where to go look", as in `policy::Finding`.
    pub subject: String,
    pub detail: String,
}

fn finding(kind: FindingKind, subject: impl Into<String>, detail: impl Into<String>) -> Finding {
    Finding { kind, subject: subject.into(), detail: detail.into() }
}

fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));
}

// ================================================================= loader

/// `<root>/.factory/quality`, the authored-content directory.
pub fn quality_dir(root: &Path) -> PathBuf {
    root.join(".factory").join("quality")
}

/// Everything [`load`] found in `<root>/.factory/quality/`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct QualityCatalogue {
    /// Keyed by profile id, the file stem.
    pub profiles: BTreeMap<String, Profile>,
    /// Sorted by `(subject, kind, detail)`.
    pub findings: Vec<Finding>,
}

/// Load every `<profile>.yaml` in `dir`. A missing directory is empty, not
/// an error; a file that fails to parse is a finding and every other file
/// still loads. See the module doc comment for what is dropped from a file
/// that did parse.
pub fn load(dir: &Path) -> QualityCatalogue {
    let mut findings = Vec::new();
    let mut profiles = BTreeMap::new();

    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(read) => read
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect(),
        Err(_) => Vec::new(),
    };
    paths.sort();

    for path in &paths {
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                findings.push(finding(FindingKind::ParseFailed, &file_name, format!("reading: {e}")));
                continue;
            }
        };
        match serde_yaml_ng::from_str::<Profile>(&text) {
            Ok(profile) => {
                if !is_slug(&stem) {
                    findings.push(finding(
                        FindingKind::BadIdShape,
                        &file_name,
                        format!("profile id {stem:?} is not shaped like dataset::is_slug ([a-z0-9][a-z0-9-]*)"),
                    ));
                }
                profiles.insert(stem, validate_profile(profile, &file_name, &mut findings));
            }
            Err(e) => findings.push(finding(FindingKind::ParseFailed, &file_name, format!("parsing: {e}"))),
        }
    }

    sort_findings(&mut findings);
    QualityCatalogue { profiles, findings }
}

/// One parsed profile's own checks, returning it with every attribute that
/// can never be placed (unknown id, repeated id) and every repeated scenario
/// removed. Everything else stays, finding or not: a scenario naming an
/// unavailable metric still shows, as `no_data` with the reason.
fn validate_profile(mut profile: Profile, subject: &str, findings: &mut Vec<Finding>) -> Profile {
    let mut seen = BTreeSet::new();
    profile.attributes.retain_mut(|attr| {
        if lookup(&attr.id).is_none() {
            findings.push(finding(
                FindingKind::UnknownAttribute,
                subject,
                format!("attribute {:?} is not an ISO 25010/25059 characteristic[.sub-characteristic]", attr.id),
            ));
            return false;
        }
        if !seen.insert(attr.id.clone()) {
            findings.push(finding(FindingKind::DuplicateId, subject, format!("attribute {:?} is declared twice", attr.id)));
            return false;
        }
        for requirement in &attr.requires {
            for detail in requirement.problems() {
                findings.push(finding(FindingKind::BadRequirement, subject, format!("{} {detail}", attr.id)));
            }
        }
        let mut scenario_ids = BTreeSet::new();
        attr.scenarios.retain(|s| {
            if !scenario_ids.insert(s.id.clone()) {
                findings.push(finding(
                    FindingKind::DuplicateId,
                    subject,
                    format!("scenario {:?} is declared twice under {}", s.id, attr.id),
                ));
                return false;
            }
            true
        });
        for s in &mut attr.scenarios {
            if !is_slug(&s.id) {
                findings.push(finding(
                    FindingKind::BadIdShape,
                    subject,
                    format!(
                        "scenario {:?} under {} is not shaped like dataset::is_slug ([a-z0-9][a-z0-9-]*)",
                        s.id, attr.id
                    ),
                ));
            }
            let what = format!("{}/{}", attr.id, s.id);
            match &s.measure {
                Some(Measure::Metric(m)) => {
                    if !check_metric(m, subject, &what, findings) {
                        s.measure = None;
                    }
                }
                Some(Measure::Check(check)) => {
                    for (kind, detail) in policy::check_vocabulary(check) {
                        let kind = match kind {
                            policy::FindingKind::UnknownDaemonFact => FindingKind::UnknownDaemonFact,
                            _ => FindingKind::UnknownSecretsLocation,
                        };
                        findings.push(finding(kind, subject, format!("{what} {detail}")));
                    }
                    if matches!(check, Check::Knowledge { tag: None }) && attr.id.contains('.') {
                        findings.push(finding(
                            FindingKind::UntaggedKnowledgeCheck,
                            subject,
                            format!(
                                "{what}'s knowledge check needs a `tag:`: the default one would contain `.`, \
                                 which a knowledge tag cannot"
                            ),
                        ));
                    }
                }
                None => {}
            }
        }
        true
    });


    for t in &profile.tradeoffs {
        for id in &t.between {
            if lookup(id).is_none() {
                findings.push(finding(
                    FindingKind::UnknownAttribute,
                    subject,
                    format!("trade-off {:?} names {id:?}, which is not an ISO 25010/25059 id", t.point),
                ));
            }
        }
        if t.between[0] == t.between[1] {
            findings.push(finding(
                FindingKind::BadTradeoff,
                subject,
                format!("trade-off {:?} is between {} and itself", t.point, t.between[0]),
            ));
        }
    }
    profile
}

/// Whether `metric` is one of the `quality.<characteristic>` metrics this
/// module's own results are summed into. A measure may never name one: the
/// value would be computed from the very scenario being judged -- a loop,
/// not a measurement ([`FindingKind::SelfReferentialMetric`]).
pub fn is_quality_metric(metric: &MetricId) -> bool {
    metric.as_str().split('.').next() == Some("quality")
}

/// A metric measure's own checks. `false` when a bound is not a finite
/// number -- `.nan` compares false both ways, so it would read as met every
/// time and could never be told apart from a tightening or a loosening --
/// and the caller drops the measure, leaving an honest draft. A measure on
/// a `quality.*` metric is kept (it reads `no_data`, with the reason), but
/// nothing else is checked about it: whatever its bounds say, it can never
/// be judged.
fn check_metric(m: &MetricMeasure, subject: &str, what: &str, findings: &mut Vec<Finding>) -> bool {
    if is_quality_metric(&m.metric) {
        findings.push(finding(
            FindingKind::SelfReferentialMetric,
            subject,
            format!("{what} measures by {}, which is computed from quality scenarios themselves", m.metric),
        ));
        return true;
    }
    match metrics::resolve(&m.metric) {
        Ok(_) => {}
        Err(MetricError::Unknown(_)) => {
            findings.push(finding(FindingKind::UnknownMetric, subject, format!("{what} names unknown metric {}", m.metric)));
        }
        Err(MetricError::Unavailable { reason, .. }) => findings.push(finding(
            FindingKind::UnavailableMetric,
            subject,
            format!("{what} names metric {} which is not available yet: {reason}", m.metric),
        )),
    }
    if m.above.is_none() && m.below.is_none() {
        findings.push(finding(
            FindingKind::MissingThreshold,
            subject,
            format!("{what}'s measure on {} has neither `above` nor `below`", m.metric),
        ));
    }
    if let Some(band) = impossible_band(m) {
        findings.push(finding(FindingKind::ImpossibleBand, subject, format!("{what}'s measure {band}")));
    }
    let finite = bounds_finite(m);
    if !finite {
        findings.push(finding(
            FindingKind::NonFiniteThreshold,
            subject,
            format!("{what}'s measure on {} has a bound that is not a finite number; the measure is dropped", m.metric),
        ));
    }
    finite
}

fn bounds_finite(m: &MetricMeasure) -> bool {
    m.above.is_none_or(f64::is_finite) && m.below.is_none_or(f64::is_finite)
}

/// `Some(description)` for a band no value can sit in: `above` over `below`.
fn impossible_band(m: &MetricMeasure) -> Option<String> {
    match (m.above, m.below) {
        (Some(a), Some(b)) if a > b => Some(format!("on {} needs >= {a} and <= {b} at once, which nothing can be", m.metric)),
        _ => None,
    }
}

/// Whether `measure` can ever produce a verdict: any check, or a metric
/// measure with at least one bound, every bound a finite number. A bound-less
/// metric measure is written down but judges nothing, so it does not make a
/// scenario "measured" -- not for [`FindingKind::UnmeasuredHighImportance`],
/// and not in [`evaluate`], where it stays a draft.
pub fn is_judgeable(measure: &Measure) -> bool {
    match measure {
        Measure::Check(_) => true,
        Measure::Metric(m) => (m.above.is_some() || m.below.is_some()) && bounds_finite(m),
    }
}

// ============================================================ applicability

/// One scope's own `quality:` list, as `Config::quality_chain_for_scope`
/// resolves it -- `policy::PolicyLayer`'s role, for quality profiles.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityLayer {
    pub scope: String,
    pub profiles: Vec<String>,
}

/// Where a merged node was first declared: the scope whose layer bound the
/// profile, and the profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    pub scope: String,
    pub profile: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedScenario {
    #[serde(flatten)]
    pub scenario: QualityScenario,
    pub declared_at: Origin,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppliedAttribute {
    pub id: String,
    pub importance: Level,
    pub difficulty: Level,
    pub declared_at: Origin,
    pub scenarios: Vec<AppliedScenario>,
    /// Every layer's `requires:` for this attribute, unioned -- add only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<crate::control_plan::Requirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedTradeoff {
    #[serde(flatten)]
    pub tradeoff: Tradeoff,
    pub declared_at: Origin,
}

/// One scope's utility tree after folding its whole chain. Attributes keep
/// the order they were first declared in, root first -- authored order is
/// itself information, not something to alphabetise away.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityTree {
    pub scope: String,
    /// Every profile folded in, in the order it was, each once.
    pub profiles: Vec<String>,
    pub attributes: Vec<AppliedAttribute>,
    pub tradeoffs: Vec<AppliedTradeoff>,
}

/// Fold `chain` (root first) into `scope`'s utility tree. Add or tighten
/// only, like `policy::applicable`:
///
/// - an attribute or scenario not yet in the tree is **added**;
/// - an inherited importance may only be **raised**;
/// - an inherited measure may only be **tightened**: `above` raised, `below`
///   lowered, `max_age` shortened, or a bound it lacked added. A
///   field a descendant leaves out is inherited, never cleared;
/// - an inherited draft scenario may be **given** a measure, or any of the
///   six parts it lacks.
///
/// Anything looser is a [`FindingKind::Loosening`], anything incomparable
/// (another metric, another check, a different sentence) a
/// [`FindingKind::ConflictingOverride`]; either way the inherited value is
/// kept. Difficulty is the one exception, nearest wins: it estimates how
/// hard the attribute is *here*, a judgement rather than a commitment
/// anyone could loosen.
///
/// Profiles apply in chain order, and within a layer in the order listed;
/// a profile bound at two layers is folded once, at the higher one.
pub fn applicable(catalogue: &QualityCatalogue, scope: &str, chain: &[QualityLayer]) -> (QualityTree, Vec<Finding>) {
    let mut findings = Vec::new();
    let mut tree = QualityTree {
        scope: scope.to_string(),
        profiles: Vec::new(),
        attributes: Vec::new(),
        tradeoffs: Vec::new(),
    };

    for layer in chain {
        for profile_id in &layer.profiles {
            if tree.profiles.contains(profile_id) {
                continue;
            }
            let Some(profile) = catalogue.profiles.get(profile_id) else {
                findings.push(finding(
                    FindingKind::MissingProfile,
                    &layer.scope,
                    format!("quality: names profile {profile_id:?}, but .factory/quality/{profile_id}.yaml did not load"),
                ));
                continue;
            };
            tree.profiles.push(profile_id.clone());
            let origin = Origin { scope: layer.scope.clone(), profile: profile_id.clone() };
            for attr in &profile.attributes {
                merge_attribute(&mut tree, attr, &origin, &mut findings);
            }
            for t in &profile.tradeoffs {
                let duplicate = tree
                    .tradeoffs
                    .iter()
                    .any(|have| have.tradeoff.point == t.point && same_pair(&have.tradeoff.between, &t.between));
                if !duplicate {
                    tree.tradeoffs.push(AppliedTradeoff { tradeoff: t.clone(), declared_at: origin.clone() });
                }
            }
        }
    }

    if tree.attributes.len() > MAX_ATTRIBUTES {
        findings.push(finding(
            FindingKind::TooManyAttributes,
            scope,
            format!(
                "{} quality attributes apply here; a utility tree names at most {MAX_ATTRIBUTES}",
                tree.attributes.len()
            ),
        ));
    }
    for attr in &tree.attributes {
        let measured = attr.scenarios.iter().any(|s| s.scenario.measure.as_ref().is_some_and(is_judgeable));
        if attr.importance == Level::High && !measured {
            findings.push(finding(
                FindingKind::UnmeasuredHighImportance,
                scope,
                format!("{} is H importance but none of its scenarios has a response measure that can be judged", attr.id),
            ));
        }
    }
    let declared: BTreeSet<&str> = tree.attributes.iter().map(|a| a.id.as_str()).collect();
    for t in &tree.tradeoffs {
        for id in &t.tradeoff.between {
            if !declared.contains(id.as_str()) {
                findings.push(finding(
                    FindingKind::UndeclaredTradeoffAttribute,
                    scope,
                    format!(
                        "trade-off {:?} (profile {}) names {id}, which this scope does not declare",
                        t.tradeoff.point, t.declared_at.profile
                    ),
                ));
            }
        }
    }

    sort_findings(&mut findings);
    (tree, findings)
}

fn same_pair(a: &[String; 2], b: &[String; 2]) -> bool {
    (a[0] == b[0] && a[1] == b[1]) || (a[0] == b[1] && a[1] == b[0])
}

fn merge_attribute(tree: &mut QualityTree, attr: &Attribute, origin: &Origin, findings: &mut Vec<Finding>) {
    let Some(have) = tree.attributes.iter_mut().find(|a| a.id == attr.id) else {
        tree.attributes.push(AppliedAttribute {
            id: attr.id.clone(),
            importance: attr.importance,
            difficulty: attr.difficulty,
            declared_at: origin.clone(),
            scenarios: attr
                .scenarios
                .iter()
                .map(|s| AppliedScenario { scenario: s.clone(), declared_at: origin.clone() })
                .collect(),
            requires: attr.requires.clone(),
        });
        return;
    };
    for requirement in &attr.requires {
        if !have.requires.contains(requirement) {
            have.requires.push(requirement.clone());
        }
    }

    let scope = tree.scope.as_str();
    let from = format!("profile {} at {}", origin.profile, origin.scope);
    if attr.importance < have.importance {
        findings.push(finding(
            FindingKind::Loosening,
            scope,
            format!(
                "{from} lowers {}'s importance from {:?} to {:?}; importance may only be raised",
                attr.id, have.importance, attr.importance
            ),
        ));
    } else {
        have.importance = attr.importance;
    }
    have.difficulty = attr.difficulty;

    for s in &attr.scenarios {
        let what = format!("{}/{}", attr.id, s.id);
        match have.scenarios.iter_mut().find(|h| h.scenario.id == s.id) {
            None => have.scenarios.push(AppliedScenario { scenario: s.clone(), declared_at: origin.clone() }),
            Some(h) => merge_scenario(&mut h.scenario, s, &Context { what: &what, from: &from, scope }, findings),
        }
    }
}

/// Who is restating what, for a merge finding's wording.
struct Context<'a> {
    what: &'a str,
    from: &'a str,
    scope: &'a str,
}

impl Context<'_> {
    fn conflict(&self, findings: &mut Vec<Finding>, detail: String) {
        findings.push(finding(
            FindingKind::ConflictingOverride,
            self.scope,
            format!("{} {detail} for {}; the inherited one is kept", self.from, self.what),
        ));
    }

    fn loosening(&self, findings: &mut Vec<Finding>, detail: String) {
        findings.push(finding(
            FindingKind::Loosening,
            self.scope,
            format!(
                "{} {detail} for {}; a measure may only be tightened, so the inherited one is kept",
                self.from, self.what
            ),
        ));
    }
}

fn merge_scenario(have: &mut QualityScenario, new: &QualityScenario, cx: &Context, findings: &mut Vec<Finding>) {
    let mut conflicts = Vec::new();
    fill(&mut have.kind, &new.kind, "kind", &mut conflicts);
    fill(&mut have.source, &new.source, "source", &mut conflicts);
    fill(&mut have.stimulus, &new.stimulus, "stimulus", &mut conflicts);
    fill(&mut have.artifact, &new.artifact, "artifact", &mut conflicts);
    fill(&mut have.environment, &new.environment, "environment", &mut conflicts);
    fill(&mut have.response, &new.response, "response", &mut conflicts);
    for part in conflicts {
        cx.conflict(findings, format!("restates the {part} differently"));
    }

    match (&mut have.measure, &new.measure) {
        (_, None) => {}
        (None, Some(Measure::Metric(m))) if !bounds_finite(m) => {
            cx.conflict(findings, format!("gives {} a bound that is not a finite number", m.metric));
        }
        (None, Some(m)) => have.measure = Some(m.clone()),
        (Some(old), Some(m)) => merge_measure(old, m, cx, findings),
    }
}

/// An inherited `None` takes the descendant's value; an inherited value
/// stays, and a different one from the descendant is recorded as `part`.
fn fill<T: Clone + PartialEq>(have: &mut Option<T>, new: &Option<T>, part: &'static str, conflicts: &mut Vec<&'static str>) {
    match (have.as_ref(), new) {
        (_, None) => {}
        (None, Some(v)) => *have = Some(v.clone()),
        (Some(h), Some(v)) if h != v => conflicts.push(part),
        (Some(_), Some(_)) => {}
    }
}

fn merge_measure(have: &mut Measure, new: &Measure, cx: &Context, findings: &mut Vec<Finding>) {
    match (&mut *have, new) {
        (Measure::Metric(_), Measure::Metric(m)) if !bounds_finite(m) => {
            cx.conflict(findings, format!("gives {} a bound that is not a finite number", m.metric));
        }
        (Measure::Metric(old), Measure::Metric(m)) if old.metric == m.metric => {
            let was_possible = impossible_band(old).is_none();
            match (old.above, m.above) {
                (Some(a), Some(b)) if b < a => cx.loosening(findings, format!("lowers `above` from {a} to {b}")),
                (_, Some(b)) => old.above = Some(b),
                _ => {}
            }
            match (old.below, m.below) {
                (Some(a), Some(b)) if b > a => cx.loosening(findings, format!("raises `below` from {a} to {b}")),
                (_, Some(b)) => old.below = Some(b),
                _ => {}
            }
            match (old.max_age, m.max_age) {
                (Some(a), Some(b)) if b > a => cx.loosening(findings, format!("lengthens `max_age` from {a} to {b}")),
                (_, Some(b)) => old.max_age = Some(b),
                _ => {}
            }
            if let Some(band) = impossible_band(old).filter(|_| was_possible) {
                findings.push(finding(
                    FindingKind::ImpossibleBand,
                    cx.scope,
                    format!("{} tightens {} until its measure {band}", cx.from, cx.what),
                ));
            }
        }
        (Measure::Check(old), Measure::Check(c)) if without_max_age(old) == without_max_age(c) => {
            match (check_max_age(old), check_max_age(c)) {
                (Some(a), Some(b)) if b > a => cx.loosening(findings, format!("lengthens `max_age` from {a} to {b}")),
                (_, Some(b)) => set_check_max_age(old, b),
                _ => {}
            }
        }
        (old, m) => {
            let detail = format!("measures by {} instead of the inherited {}", describe_measure(m), describe_measure(old));
            cx.conflict(findings, detail);
        }
    }
}

/// The freshness window of the check kinds that have one -- the same set
/// `policy::Check::own_max_age` (private to that module) reads. `attested`'s
/// own field is a required `Duration`, never `Option`, but it still counts:
/// wrapped in `Some` here the same as every other kind's.
fn check_max_age(check: &Check) -> Option<Duration> {
    match check {
        Check::Task { max_age, .. } | Check::Workflow { max_age, .. } | Check::Gate { max_age, .. } => *max_age,
        Check::Attested { max_age, .. } => Some(*max_age),
        _ => None,
    }
}

fn set_check_max_age(check: &mut Check, to: Duration) {
    if let Check::Task { max_age, .. } | Check::Workflow { max_age, .. } | Check::Gate { max_age, .. } = check {
        *max_age = Some(to);
    }
    if let Check::Attested { max_age, .. } = check {
        *max_age = to;
    }
}

/// `check` with its own `max_age` zeroed out, so `merge_measure` can tell
/// whether two check measures are "the same check, maybe a different
/// window" from "a different check entirely" by plain equality.
/// `attested`'s own field is a required `Duration`, not `Option`, so it
/// cannot join the `Task`/`Workflow`/`Gate` pattern above (`None` has
/// nowhere to go); zeroing it to `Duration::from_hours(0)` is the same
/// "ignore this field for the comparison" move, just spelled for a
/// non-optional field.
fn without_max_age(check: &Check) -> Check {
    let mut c = check.clone();
    if let Check::Task { max_age, .. } | Check::Workflow { max_age, .. } | Check::Gate { max_age, .. } = &mut c {
        *max_age = None;
    }
    if let Check::Attested { max_age, .. } = &mut c {
        *max_age = Duration::from_hours(0);
    }
    c
}

/// One line naming a measure -- `scrap_rate <= 0.05 within 7d`, or a
/// check's own `policy::Check::describe`.
pub fn describe_measure(measure: &Measure) -> String {
    match measure {
        Measure::Metric(m) => {
            let mut parts = vec![m.metric.to_string()];
            if let Some(a) = m.above {
                parts.push(format!(">= {a}"));
            }
            if let Some(b) = m.below {
                parts.push(format!("<= {b}"));
            }
            if let Some(w) = m.max_age {
                parts.push(format!("(max_age {w})"));
            }
            parts.join(" ")
        }
        Measure::Check(c) => c.describe(),
    }
}

// =============================================================== evaluate

/// A scenario's standing. Declared best to worst, so the derived `Ord` makes
/// [`ScenarioStatus::worst`] a plain `max`. `draft` sits between `met` and
/// `no_data`: a scenario with no measure is less alarming than a measure
/// nobody can read, but it is never green.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    Met,
    Draft,
    NoData,
    Stale,
    NotMet,
}

impl ScenarioStatus {
    /// The worst of `statuses` -- an attribute's rollup. With nothing to roll
    /// up (an attribute declared with no scenarios) it is `draft`: a stated
    /// concern with no specification yet, never `met`.
    pub fn worst(statuses: impl IntoIterator<Item = ScenarioStatus>) -> ScenarioStatus {
        statuses.into_iter().max().unwrap_or(ScenarioStatus::Draft)
    }

    /// The wire's own spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ScenarioStatus::Met => "met",
            ScenarioStatus::Draft => "draft",
            ScenarioStatus::NoData => "no_data",
            ScenarioStatus::Stale => "stale",
            ScenarioStatus::NotMet => "not_met",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScenarioResult {
    #[serde(flatten)]
    pub scenario: AppliedScenario,
    pub status: ScenarioStatus,
    pub reasons: Vec<String>,
    /// For a metric measure, the value it was judged on, when there was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of: Option<DateTime<Utc>>,
    /// For a check measure, whatever `policy::evaluate` could point at.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributeResult {
    pub id: String,
    pub characteristic: String,
    pub importance: Level,
    pub difficulty: Level,
    pub declared_at: Origin,
    /// The worst of `scenarios`' statuses.
    pub status: ScenarioStatus,
    pub scenarios: Vec<ScenarioResult>,
}

/// One scope's evaluated utility tree. Deliberately no score field -- see
/// the module doc comment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScopeReport {
    pub scope: String,
    pub profiles: Vec<String>,
    pub attributes: Vec<AttributeResult>,
    pub tradeoffs: Vec<AppliedTradeoff>,
}

/// Judge every scenario in `tree`. Pure, like `policy::evaluate` and
/// `goals::evaluate`: metric values and policy evidence are handed in
/// already computed (`factory-daemon`'s job), and `now` is a parameter.
///
/// - no measure: `draft`;
/// - a metric measure: `no_data` when the metric is unknown, unavailable or
///   has no value yet; `stale` when its `as_of` is older than `max_age`;
///   otherwise `met` if every bound holds and `not_met` if one does not;
/// - a check measure: through `policy::evaluate`, as a one-control
///   catalogue. `satisfied`/`attested` is `met`, `stale` is `stale`, and
///   `open` is `no_data` when the evidence the check reads was never
///   gathered or holds nothing finished to judge ([`gathered`]), `not_met`
///   otherwise.
pub fn evaluate(
    tree: &QualityTree,
    values: &BTreeMap<MetricId, MetricValue>,
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> ScopeReport {
    let attributes = tree
        .attributes
        .iter()
        .map(|attr| {
            let scenarios: Vec<ScenarioResult> = attr
                .scenarios
                .iter()
                .map(|s| evaluate_scenario(&attr.id, s, values, evidence, now))
                .collect();
            AttributeResult {
                id: attr.id.clone(),
                characteristic: characteristic_of(&attr.id).to_string(),
                importance: attr.importance,
                difficulty: attr.difficulty,
                declared_at: attr.declared_at.clone(),
                status: ScenarioStatus::worst(scenarios.iter().map(|s| s.status)),
                scenarios,
            }
        })
        .collect();
    ScopeReport {
        scope: tree.scope.clone(),
        profiles: tree.profiles.clone(),
        attributes,
        tradeoffs: tree.tradeoffs.clone(),
    }
}

fn evaluate_scenario(
    attribute: &str,
    applied: &AppliedScenario,
    values: &BTreeMap<MetricId, MetricValue>,
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> ScenarioResult {
    let mut result = ScenarioResult {
        scenario: applied.clone(),
        status: ScenarioStatus::Draft,
        reasons: Vec::new(),
        value: None,
        as_of: None,
        refs: Vec::new(),
    };
    match &applied.scenario.measure {
        None => result.reasons.push("no response measure yet".to_string()),
        Some(Measure::Metric(m)) => evaluate_metric(m, values, now, &mut result),
        Some(Measure::Check(check)) => {
            let (status, reasons, refs) = evaluate_check(attribute, &applied.scenario.id, check, evidence, now);
            result.status = status;
            result.reasons = reasons;
            result.refs = refs;
        }
    }
    result
}

fn evaluate_metric(m: &MetricMeasure, values: &BTreeMap<MetricId, MetricValue>, now: DateTime<Utc>, result: &mut ScenarioResult) {
    if is_quality_metric(&m.metric) {
        result.status = ScenarioStatus::NoData;
        result.reasons.push(format!("{} is computed from quality scenarios themselves, so it cannot measure one", m.metric));
        return;
    }
    match metrics::resolve(&m.metric) {
        Err(MetricError::Unknown(_)) => {
            result.status = ScenarioStatus::NoData;
            result.reasons.push(format!("{} is not a known metric", m.metric));
            return;
        }
        Err(MetricError::Unavailable { reason, .. }) => {
            result.status = ScenarioStatus::NoData;
            result.reasons.push(format!("{} is not available yet: {reason}", m.metric));
            return;
        }
        Ok(_) => {}
    }
    if !is_judgeable(&Measure::Metric(m.clone())) {
        result.status = ScenarioStatus::Draft;
        result.reasons.push(format!("the measure on {} has no finite threshold", m.metric));
        return;
    }
    let Some(mv) = values.get(&m.metric) else {
        result.status = ScenarioStatus::NoData;
        result.reasons.push(format!("no value for {} yet", m.metric));
        return;
    };
    result.as_of = Some(mv.as_of);
    let Some(v) = mv.value.filter(|v| v.is_finite()) else {
        result.status = ScenarioStatus::NoData;
        let why = match mv.value {
            Some(v) => format!("{v} is not a finite number"),
            None => mv.reason.clone().unwrap_or_else(|| "no reason given".to_string()),
        };
        result.reasons.push(format!("{} could not be computed: {why}", m.metric));
        return;
    };
    result.value = Some(v);
    if let Some(max_age) = m.max_age {
        if now - mv.as_of > max_age.as_time_delta() {
            result.status = ScenarioStatus::Stale;
            result.reasons.push(format!("{} = {v} as of {}, older than {max_age}", m.metric, mv.as_of));
            return;
        }
    }
    let mut failed = Vec::new();
    if let Some(a) = m.above {
        if v < a {
            failed.push(format!("{} = {v}, below the required {a}", m.metric));
        }
    }
    if let Some(b) = m.below {
        if v > b {
            failed.push(format!("{} = {v}, above the allowed {b}", m.metric));
        }
    }
    if failed.is_empty() {
        result.status = ScenarioStatus::Met;
        result.reasons.push(format!("{} = {v}, meets {}", m.metric, describe_measure(&Measure::Metric(m.clone()))));
    } else {
        result.status = ScenarioStatus::NotMet;
        result.reasons = failed;
    }
}

/// The control a check measure is evaluated as: framework `quality`, id
/// `<attribute>/<scenario>`. `policy::evaluate` needs *some* `ControlRef`,
/// and this one names the scenario exactly. It is built with
/// `ControlRef::new` and does not survive `ControlRef`'s own `FromStr` (its
/// id holds `.` and `/`), which is why an `attestation` check measure reads
/// `no_data` -- see [`ATTESTATION_UNSUPPORTED`].
pub fn control_ref(attribute: &str, scenario: &str) -> ControlRef {
    ControlRef::new("quality", format!("{attribute}/{scenario}"))
}

/// The one-control `Applied` a check measure is judged as -- built here
/// and nowhere else, so what [`evaluate`] hands `policy::evaluate` and what
/// [`applied_checks`] hands the daemon's evidence gathering can never
/// disagree about which check is being asked.
fn check_as_applied(attribute: &str, scenario: &str, check: &Check) -> Applied {
    Applied {
        control: control_ref(attribute, scenario),
        title: format!("{attribute}/{scenario}"),
        kind: Kind::Standard,
        maps_to: Vec::new(),
        evidence: vec![check.clone()],
        max_age: check_max_age(check),
        not_applicable: None,
        remediation: None,
        requires: Vec::new(),
    }
}

/// Every attribute's `requires:` in `tree`, as the `(source, requirements)`
/// pairs `control_plan::resolve` takes -- the source named
/// `quality/<attribute>`, the way a policy control is `<framework>/<id>`.
pub fn requirements_of(tree: &QualityTree) -> Vec<(String, Vec<crate::control_plan::Requirement>)> {
    tree.attributes
        .iter()
        .filter(|a| !a.requires.is_empty())
        .map(|a| (format!("quality/{}", a.id), a.requires.clone()))
        .collect()
}

/// Every check measure in `tree`, each as the one-control `Applied`
/// [`evaluate`] judges it as. This is what `factory-daemon` hands Policy's
/// own evidence gathering (`dataset_level_facts`/`evidence_for_scope`,
/// which read nothing but each `Applied`'s checks), so a quality scenario's
/// task, workflow, gate, roles, sandbox, secrets or daemon fact is gathered
/// by exactly the code that gathers a policy control's -- lazily, only for
/// the kinds some scenario actually asks -- with no second evidence path.
pub fn applied_checks(tree: &QualityTree) -> Vec<Applied> {
    tree.attributes
        .iter()
        .flat_map(|attr| {
            attr.scenarios.iter().filter_map(move |s| match &s.scenario.measure {
                Some(Measure::Check(check)) => Some(check_as_applied(&attr.id, &s.scenario.id, check)),
                _ => None,
            })
        })
        .collect()
}

/// Why an `attestation` check measure reads `no_data` rather than being
/// judged: nothing can record one. An `Attestation` lives in the policy
/// store keyed by its `ControlRef`, which is stored as the `framework/id`
/// string and parsed back through `ControlRef::from_str` -- and
/// [`control_ref`]'s id holds `.` and `/`, so a row written under it would
/// be dropped as malformed on the very next read. Rather than a scenario
/// that reads `not_met` forever because nothing could ever satisfy it,
/// the honest answer is that there is no data, and why.
pub const ATTESTATION_UNSUPPORTED: &str = "a quality scenario cannot be attested yet: an attestation is keyed by a policy \
     control ref, and quality/<attribute>/<scenario> is not one the policy store can read back; \
     measure it with a task, workflow or gate check instead";

fn evaluate_check(
    attribute: &str,
    scenario: &str,
    check: &Check,
    evidence: &Evidence,
    now: DateTime<Utc>,
) -> (ScenarioStatus, Vec<String>, Vec<EvidenceRef>) {
    if matches!(check, Check::Attestation) {
        return (ScenarioStatus::NoData, vec![ATTESTATION_UNSUPPORTED.to_string()], Vec::new());
    }
    let applied = check_as_applied(attribute, scenario, check);
    let Some(status) = policy::evaluate(std::slice::from_ref(&applied), evidence, now).into_iter().next() else {
        return (ScenarioStatus::NoData, vec!["the check could not be evaluated".to_string()], Vec::new());
    };
    let quality = match status.status.kind() {
        StatusKind::Satisfied | StatusKind::Attested => ScenarioStatus::Met,
        StatusKind::Stale => ScenarioStatus::Stale,
        StatusKind::Open if !gathered(check, evidence) => ScenarioStatus::NoData,
        StatusKind::Open => ScenarioStatus::NotMet,
        // Never built with `not_applicable`, so never reached; and were it,
        // "does not apply" is not evidence of anything being met.
        StatusKind::NotApplicable => ScenarioStatus::NoData,
    };
    (quality, status.status.reasons().to_vec(), status.refs)
}

/// Whether `evidence` holds something finished for `check` to judge -- the
/// line between `no_data` and `not_met` for an `open` policy status, which
/// `policy::evaluate` uses both for "the task failed" and for "there is no
/// such task". This only asks what was gathered, never re-judges it; the
/// verdict is still `policy::evaluate`'s. `knowledge` and `attestation`
/// always count as gathered -- a missing tag or attestation is itself the
/// fact. A `gate` whose run has no gated case to judge counts as not
/// gathered: a dataset nobody put a gate on is missing evidence, not a
/// failing product.
pub fn gathered(check: &Check, evidence: &Evidence) -> bool {
    match check {
        Check::Knowledge { .. } | Check::Attestation => true,
        Check::Task { task, .. } => match evidence.tasks.get(task).map(Vec::as_slice) {
            Some([fact]) => fact.runs.iter().any(|r| r.status.is_terminal()),
            _ => false,
        },
        Check::Workflow { workflow, .. } => match evidence.workflows.get(workflow).map(Vec::as_slice) {
            Some([fact]) => fact.runs.iter().any(|r| r.status.is_terminal()),
            _ => false,
        },
        // A settled run with nothing gated to judge -- no gated case at all,
        // or a named case missing or ungated -- is policy's `open` too, but
        // it says nothing about the product: no data, not a failure.
        Check::Gate { dataset, case, .. } => evidence.gates.get(dataset).is_some_and(|fact| match case {
            Some(id) => fact.cases.iter().any(|c| &c.id == id && c.gated),
            None => fact.cases.iter().any(|c| c.gated),
        }),
        Check::Roles { .. } | Check::Sandbox => evidence.agents.is_some(),
        // The same default location `policy::evaluate` reads for an empty
        // `absent`.
        Check::Secrets { absent } if absent.is_empty() => evidence.secrets.contains_key("scope_env"),
        Check::Secrets { absent } => absent.iter().all(|n| evidence.secrets.contains_key(n)),
        Check::Daemon { .. } => evidence.daemon.is_some(),
        Check::Dependencies { .. } => evidence.dependencies.is_some(),
        Check::Attested { .. } => evidence.attested.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::Verdict;
    use crate::policy::{GateCase, GateFact, RunFact, TaskFact};
    use crate::run::RunStatus;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 25, 12, 0, 0).unwrap()
    }

    fn write(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).unwrap();
    }

    /// Load `files` from a fresh directory, then remove it -- `load` reads
    /// everything up front, so nothing needs the files afterwards.
    fn catalogue_from(files: &[(&str, &str)]) -> QualityCatalogue {
        let dir = std::env::temp_dir().join(format!("factory-quality-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            write(&dir, name, body);
        }
        let q = load(&dir);
        std::fs::remove_dir_all(&dir).ok();
        q
    }

    fn kinds(findings: &[Finding]) -> Vec<FindingKind> {
        findings.iter().map(|f| f.kind).collect()
    }

    fn layer(scope: &str, profiles: &[&str]) -> QualityLayer {
        QualityLayer { scope: scope.to_string(), profiles: profiles.iter().map(|p| p.to_string()).collect() }
    }

    fn metric_value(id: &str, value: Option<f64>, as_of: DateTime<Utc>) -> (MetricId, MetricValue) {
        let id = MetricId::new(id).unwrap();
        (id.clone(), MetricValue { id, value, as_of, reason: value.is_none().then(|| "no finished runs".to_string()) })
    }

    // -- catalogue -------------------------------------------------------

    #[test]
    fn the_catalogue_is_25010s_nine_characteristics_with_slug_ids_and_no_repeats() {
        let ids: Vec<&str> = CATALOGUE.iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            vec![
                "functional-suitability",
                "performance-efficiency",
                "compatibility",
                "interaction-capability",
                "reliability",
                "security",
                "maintainability",
                "flexibility",
                "safety",
            ]
        );
        for c in CATALOGUE {
            assert!(is_slug(c.id));
            let subs: BTreeSet<&str> = c.subs.iter().map(|s| s.id).collect();
            assert_eq!(subs.len(), c.subs.len(), "{} repeats a sub-characteristic", c.id);
            assert!(c.subs.iter().all(|s| is_slug(s.id)));
        }
    }

    #[test]
    fn the_ai_pack_is_exactly_25059s_five_additions() {
        let ai: Vec<String> = CATALOGUE
            .iter()
            .flat_map(|c| c.subs.iter().filter(|s| s.standard == Standard::Iso25059).map(move |s| format!("{}.{}", c.id, s.id)))
            .collect();
        assert_eq!(
            ai,
            vec![
                "functional-suitability.functional-adaptability",
                "interaction-capability.user-controllability",
                "interaction-capability.transparency",
                "reliability.robustness",
                "security.intervenability",
            ]
        );
    }

    #[test]
    fn lookup_takes_a_characteristic_alone_or_one_of_its_own_subs_only() {
        assert!(lookup("reliability").is_some());
        let (c, s) = lookup("reliability.robustness").unwrap();
        assert_eq!((c.id, s.unwrap().standard), ("reliability", Standard::Iso25059));
        assert!(lookup("security.robustness").is_none(), "robustness is reliability's");
        assert!(lookup("performance.time-behaviour").is_none(), "the short spelling is not an id");
        assert!(lookup("reliability.recoverability.extra").is_none());
        assert_eq!(characteristic_of("reliability.recoverability"), "reliability");
        assert_eq!(characteristic_of("safety"), "safety");
    }

    // -- parsing -------------------------------------------------------------

    #[test]
    fn importance_is_h_m_or_l_and_orders_that_way() {
        let levels: Vec<Level> = serde_yaml_ng::from_str("[L, M, H]").unwrap();
        assert_eq!(levels, vec![Level::Low, Level::Medium, Level::High]);
        assert!(Level::Low < Level::Medium && Level::Medium < Level::High);
        assert!(serde_yaml_ng::from_str::<Level>("7").is_err(), "no finer scale than H/M/L");
    }

    #[test]
    fn a_measure_is_a_metric_threshold_or_a_policy_check_written_as_policy_writes_it() {
        let m: Measure = serde_yaml_ng::from_str("{ metric: scrap_rate, below: 0.05, max_age: 7d }").unwrap();
        assert!(matches!(&m, Measure::Metric(x) if x.below == Some(0.05) && x.max_age == Some("7d".parse().unwrap())));

        let c: Measure = serde_yaml_ng::from_str("{ check: task, task: quality-gate, max_age: 7d }").unwrap();
        assert_eq!(
            c,
            Measure::Check(Check::Task { task: "quality-gate".to_string(), max_age: Some("7d".parse().unwrap()) })
        );

        // Round-trips through its own serialization, both shapes.
        for m in [m, c] {
            let back: Measure = serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&m).unwrap()).unwrap();
            assert_eq!(back, m);
        }
    }

    #[test]
    fn a_bad_measure_says_what_was_wrong_rather_than_matching_no_variant() {
        let both = serde_yaml_ng::from_str::<Measure>("{ metric: scrap_rate, check: sandbox }").unwrap_err();
        assert!(both.to_string().contains("not both"), "{both}");
        let neither = serde_yaml_ng::from_str::<Measure>("{ below: 0.05 }").unwrap_err();
        assert!(neither.to_string().contains("either `metric:`"), "{neither}");
        let typo = serde_yaml_ng::from_str::<Measure>("{ check: task, task: t, maxage: 7d }").unwrap_err();
        assert!(typo.to_string().contains("maxage"), "{typo}");
        let typo = serde_yaml_ng::from_str::<Measure>("{ metric: scrap_rate, bellow: 0.05 }").unwrap_err();
        assert!(typo.to_string().contains("bellow"), "{typo}");
    }

    // -- load ------------------------------------------------------------------

    #[test]
    fn a_missing_directory_is_empty_not_an_error() {
        let q = load(Path::new("/definitely/not/here/quality"));
        assert!(q.profiles.is_empty() && q.findings.is_empty());
    }

    #[test]
    fn a_bad_file_is_a_finding_and_the_others_still_load() {
        let q = catalogue_from(&[
            ("broken.yaml", "attributes: [ this is not"),
            ("typo.yaml", "atributes: []\n"),
            ("fine.yaml", "attributes:\n  - { id: safety, importance: L, difficulty: L }\n"),
            ("notes.txt", "ignored"),
        ]);
        assert_eq!(q.profiles.keys().collect::<Vec<_>>(), vec!["fine"]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::ParseFailed, FindingKind::ParseFailed]);
    }

    #[test]
    fn load_drops_unknown_and_repeated_attributes_and_repeated_scenarios_with_a_finding() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n\
             \x20 - { id: performance.time-behaviour, importance: H, difficulty: L }\n\
             \x20 - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: up }\n      - { id: up, response: again }\n\
             \x20 - { id: reliability.availability, importance: H, difficulty: H }\n",
        )]);
        let p = &q.profiles["p"];
        assert_eq!(p.attributes.len(), 1);
        assert_eq!(p.attributes[0].importance, Level::Medium, "the first declaration is kept");
        assert_eq!(p.attributes[0].scenarios.len(), 1);
        assert_eq!(
            kinds(&q.findings),
            vec![FindingKind::UnknownAttribute, FindingKind::DuplicateId, FindingKind::DuplicateId]
        );
    }

    #[test]
    fn load_reports_bad_id_shapes_but_keeps_the_profile() {
        let q = catalogue_from(&[(
            "Bad_Name.yaml",
            "attributes:\n  - id: safety\n    importance: L\n    difficulty: L\n    scenarios: [ { id: Not_A_Slug } ]\n",
        )]);
        assert!(q.profiles.contains_key("Bad_Name"));
        assert_eq!(kinds(&q.findings), vec![FindingKind::BadIdShape, FindingKind::BadIdShape]);
    }

    #[test]
    fn load_refuses_an_unknown_metric_and_a_missing_threshold_and_accepts_unit_cost() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n  - id: performance-efficiency.resource-utilization\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: cost, measure: { metric: unit_cost, below: 1.0 } }\n\
             \x20     - { id: fail, measure: { metric: not_a_metric, below: 0.05 } }\n\
             \x20     - { id: bare, measure: { metric: scrap_rate } }\n",
        )]);
        // `unit_cost` is computable since #117, so `cost` is a measure
        // like any other; no metric is left to be unavailable.
        assert_eq!(kinds(&q.findings), vec![FindingKind::UnknownMetric, FindingKind::MissingThreshold]);
    }

    #[test]
    fn a_knowledge_check_under_a_dotted_attribute_must_name_its_tag() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n\
             \x20 - id: security.accountability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: untagged, measure: { check: knowledge } }\n\
             \x20     - { id: tagged, measure: { check: knowledge, tag: audit/runbook } }\n\
             \x20 - id: safety\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: default-tag-is-fine, measure: { check: knowledge } }\n",
        )]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::UntaggedKnowledgeCheck]);
        assert!(q.findings[0].detail.contains("security.accountability/untagged"), "{}", q.findings[0].detail);
    }

    #[test]
    fn load_checks_a_tradeoffs_ids_against_the_catalogue_and_refuses_a_self_tradeoff() {
        let q = catalogue_from(&[(
            "p.yaml",
            "tradeoffs:\n\
             \x20 - { between: [security.confidentiality, performance.time-behaviour], point: sandbox }\n\
             \x20 - { between: [safety, safety], point: nothing }\n",
        )]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::UnknownAttribute, FindingKind::BadTradeoff]);
    }

    // -- applicable --------------------------------------------------------------

    const ROOT: &str = "attributes:\n\
        \x20 - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
        \x20     - id: up\n        kind: usage\n        response: the daemon answers\n\
        \x20       measure: { metric: first_pass_yield, above: 0.8, max_age: 7d }\n\
        \x20     - id: gate\n        measure: { check: task, task: quality-gate, max_age: 7d }\n\
        \x20     - id: attested-gate\n        measure: { check: attested, category: feature, step: tests, max_age: 7d }\n\
        \x20     - id: draft\n        stimulus: something happens\n";

    fn chained(child: &str) -> (QualityTree, Vec<Finding>) {
        let q = catalogue_from(&[("root.yaml", ROOT), ("child.yaml", child)]);
        assert!(q.findings.is_empty(), "{:?}", q.findings);
        applicable(&q, "demo", &[layer("company", &["root"]), layer("demo", &["child"])])
    }

    fn scenario<'a>(tree: &'a QualityTree, attr: &str, id: &str) -> &'a QualityScenario {
        &tree.attributes.iter().find(|a| a.id == attr).unwrap().scenarios.iter().find(|s| s.scenario.id == id).unwrap().scenario
    }

    #[test]
    fn a_descendant_adds_attributes_and_scenarios_and_raises_importance() {
        let (tree, findings) = chained(
            "attributes:\n\
             \x20 - id: reliability.availability\n    importance: H\n    difficulty: L\n    scenarios:\n\
             \x20     - { id: extra, measure: { metric: scrap_rate, below: 0.1 } }\n\
             \x20 - { id: safety, importance: L, difficulty: L }\n",
        );
        assert!(findings.is_empty(), "{findings:?}");
        let ids: Vec<&str> = tree.attributes.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["reliability.availability", "safety"], "root's first, in authored order");
        let avail = &tree.attributes[0];
        assert_eq!(avail.importance, Level::High);
        assert_eq!(avail.difficulty, Level::Low, "difficulty is the nearest layer's estimate");
        assert_eq!(avail.declared_at, Origin { scope: "company".into(), profile: "root".into() });
        let extra = avail.scenarios.iter().find(|s| s.scenario.id == "extra").unwrap();
        assert_eq!(extra.declared_at.scope, "demo");
        assert_eq!(tree.profiles, vec!["root".to_string(), "child".to_string()]);
    }

    #[test]
    fn lowering_importance_is_a_loosening_and_the_inherited_rank_stays() {
        let (tree, findings) = chained("attributes:\n  - { id: reliability.availability, importance: L, difficulty: M }\n");
        assert_eq!(kinds(&findings), vec![FindingKind::Loosening]);
        assert_eq!(tree.attributes[0].importance, Level::Medium);
    }

    #[test]
    fn a_metric_measure_may_be_tightened_and_given_a_bound_it_lacked() {
        let (tree, findings) = chained(
            "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: up, measure: { metric: first_pass_yield, above: 0.9, below: 0.99, max_age: 1d } }\n",
        );
        assert!(findings.is_empty(), "{findings:?}");
        let Some(Measure::Metric(m)) = &scenario(&tree, "reliability.availability", "up").measure else { panic!() };
        assert_eq!((m.above, m.below, m.max_age), (Some(0.9), Some(0.99), Some("1d".parse().unwrap())));
    }

    #[test]
    fn loosening_a_metric_measure_is_a_finding_and_each_looser_value_is_ignored() {
        let (tree, findings) = chained(
            "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: up, measure: { metric: first_pass_yield, above: 0.5, max_age: 30d } }\n",
        );
        assert_eq!(kinds(&findings), vec![FindingKind::Loosening, FindingKind::Loosening]);
        let Some(Measure::Metric(m)) = &scenario(&tree, "reliability.availability", "up").measure else { panic!() };
        assert_eq!((m.above, m.max_age), (Some(0.8), Some("7d".parse().unwrap())));
    }

    #[test]
    fn a_different_metric_or_check_is_a_conflict_and_the_inherited_measure_stays() {
        let (tree, findings) = chained(
            "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: up, measure: { metric: scrap_rate, below: 0.1 } }\n\
             \x20     - { id: gate, measure: { check: task, task: another-gate } }\n",
        );
        assert_eq!(kinds(&findings), vec![FindingKind::ConflictingOverride, FindingKind::ConflictingOverride]);
        let Some(Measure::Metric(m)) = &scenario(&tree, "reliability.availability", "up").measure else { panic!() };
        assert_eq!(m.metric.as_str(), "first_pass_yield");
    }

    #[test]
    fn a_check_measures_max_age_may_only_shrink() {
        let tighter = "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: gate, measure: { check: task, task: quality-gate, max_age: 2d } }\n";
        let (tree, findings) = chained(tighter);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(
            scenario(&tree, "reliability.availability", "gate").measure,
            Some(Measure::Check(Check::Task { task: "quality-gate".into(), max_age: Some("2d".parse().unwrap()) }))
        );

        let (tree, findings) = chained(&tighter.replace("2d", "2w"));
        assert_eq!(kinds(&findings), vec![FindingKind::Loosening]);
        assert_eq!(
            scenario(&tree, "reliability.availability", "gate").measure,
            Some(Measure::Check(Check::Task { task: "quality-gate".into(), max_age: Some("7d".parse().unwrap()) }))
        );
    }

    /// `#158`: `attested`'s own `max_age` is a required `Duration`, not
    /// `Option` -- unlike `task`/`workflow`/`gate`, so `without_max_age`
    /// cannot fold it into their shared `None` pattern. This is the
    /// regression `check_max_age`/`set_check_max_age`/`without_max_age`
    /// each need their own `Attested` arm for: without one, a child
    /// profile that only shortens an inherited `attested` measure's
    /// `max_age` would read as `measures by ... instead of the inherited
    /// ...` (`ConflictingOverride`), a different check entirely, rather
    /// than the tighten it actually is.
    #[test]
    fn an_attested_check_measures_max_age_may_only_shrink() {
        let tighter = "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: attested-gate, measure: { check: attested, category: feature, step: tests, max_age: 2d } }\n";
        let (tree, findings) = chained(tighter);
        assert!(findings.is_empty(), "{findings:?}");
        assert_eq!(
            scenario(&tree, "reliability.availability", "attested-gate").measure,
            Some(Measure::Check(Check::Attested {
                category: "feature".into(),
                step: "tests".into(),
                max_age: "2d".parse().unwrap()
            }))
        );

        let (tree, findings) = chained(&tighter.replace("2d", "2w"));
        assert_eq!(kinds(&findings), vec![FindingKind::Loosening]);
        assert_eq!(
            scenario(&tree, "reliability.availability", "attested-gate").measure,
            Some(Measure::Check(Check::Attested {
                category: "feature".into(),
                step: "tests".into(),
                max_age: "7d".parse().unwrap()
            }))
        );
    }

    #[test]
    fn a_draft_may_be_given_a_measure_and_missing_parts_but_not_have_its_text_rewritten() {
        let (tree, findings) = chained(
            "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: draft, response: it recovers, measure: { metric: scrap_rate, below: 0.1 } }\n\
             \x20     - { id: up, response: something else entirely, kind: change }\n",
        );
        assert_eq!(kinds(&findings), vec![FindingKind::ConflictingOverride, FindingKind::ConflictingOverride]);
        let draft = scenario(&tree, "reliability.availability", "draft");
        assert!(draft.measure.is_some());
        assert_eq!(draft.response.as_deref(), Some("it recovers"));
        let up = scenario(&tree, "reliability.availability", "up");
        assert_eq!(up.response.as_deref(), Some("the daemon answers"));
        assert_eq!(up.kind, Some(ScenarioKind::Usage));
    }

    #[test]
    fn a_missing_profile_is_a_finding_at_the_scope_that_named_it_and_a_repeat_folds_once() {
        let q = catalogue_from(&[("root.yaml", ROOT)]);
        let (tree, findings) =
            applicable(&q, "demo", &[layer("company", &["root"]), layer("demo", &["root", "gone"])]);
        assert_eq!(kinds(&findings), vec![FindingKind::MissingProfile]);
        assert_eq!(findings[0].subject, "demo");
        assert_eq!(tree.profiles, vec!["root".to_string()]);
        assert_eq!(tree.attributes[0].declared_at.scope, "company");
    }

    #[test]
    fn more_than_seven_attributes_on_one_scope_is_a_finding() {
        let subs = [
            "reliability.faultlessness",
            "reliability.availability",
            "reliability.fault-tolerance",
            "reliability.recoverability",
            "security.integrity",
            "safety.fail-safe",
            "compatibility",
            "flexibility",
        ];
        let mut body = String::from("attributes:\n");
        for id in &subs[..4] {
            body.push_str(&format!("  - {{ id: {id}, importance: L, difficulty: L }}\n"));
        }
        let mut more = String::from("attributes:\n");
        for id in &subs[4..] {
            more.push_str(&format!("  - {{ id: {id}, importance: L, difficulty: L }}\n"));
        }
        let q = catalogue_from(&[("a.yaml", &body), ("b.yaml", &more)]);
        let (_, findings) = applicable(&q, "demo", &[layer("company", &["a"])]);
        assert!(findings.is_empty(), "four is fine");
        let (_, findings) = applicable(&q, "demo", &[layer("company", &["a"]), layer("demo", &["b"])]);
        assert_eq!(kinds(&findings), vec![FindingKind::TooManyAttributes]);
        assert_eq!(findings[0].subject, "demo");
    }

    #[test]
    fn an_h_attribute_with_no_measured_scenario_is_a_finding() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n\
             \x20 - { id: safety, importance: H, difficulty: L }\n\
             \x20 - { id: security, importance: H, difficulty: L, scenarios: [ { id: draft-only } ] }\n\
             \x20 - { id: flexibility, importance: M, difficulty: L }\n",
        )]);
        let (_, findings) = applicable(&q, "demo", &[layer("demo", &["p"])]);
        assert_eq!(kinds(&findings), vec![FindingKind::UnmeasuredHighImportance, FindingKind::UnmeasuredHighImportance]);
    }

    #[test]
    fn a_tradeoff_naming_an_attribute_the_scope_does_not_declare_is_a_finding() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n  - { id: safety, importance: L, difficulty: L }\n\
             tradeoffs:\n  - { between: [safety, flexibility], point: rigid on purpose }\n\
             \x20 - { between: [flexibility, safety], point: rigid on purpose }\n",
        )]);
        let (tree, findings) = applicable(&q, "demo", &[layer("demo", &["p"])]);
        assert_eq!(tree.tradeoffs.len(), 1, "the same point between the same pair, either way round, is one");
        assert_eq!(kinds(&findings), vec![FindingKind::UndeclaredTradeoffAttribute]);
    }

    // -- evaluate ----------------------------------------------------------------

    fn tree_with(measures: &[(&str, Option<&str>)]) -> QualityTree {
        let mut body = String::from("attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n");
        for (id, measure) in measures {
            match measure {
                Some(m) => body.push_str(&format!("      - {{ id: {id}, measure: {m} }}\n")),
                None => body.push_str(&format!("      - {{ id: {id} }}\n")),
            }
        }
        let q = catalogue_from(&[("p.yaml", &body)]);
        applicable(&q, "demo", &[layer("demo", &["p"])]).0
    }

    fn statuses(report: &ScopeReport) -> Vec<(String, ScenarioStatus)> {
        report.attributes[0].scenarios.iter().map(|s| (s.scenario.scenario.id.clone(), s.status)).collect()
    }

    fn status_of(report: &ScopeReport, id: &str) -> ScenarioStatus {
        statuses(report).into_iter().find(|(s, _)| s == id).unwrap().1
    }

    #[test]
    fn a_metric_measure_is_met_on_its_bound_and_not_met_past_it() {
        let tree = tree_with(&[
            ("above-on", Some("{ metric: first_pass_yield, above: 0.8 }")),
            ("above-under", Some("{ metric: first_pass_yield, above: 0.81 }")),
            ("below-on", Some("{ metric: scrap_rate, below: 0.05 }")),
            ("below-over", Some("{ metric: scrap_rate, below: 0.04 }")),
            ("band", Some("{ metric: scrap_rate, above: 0.01, below: 0.1 }")),
        ]);
        let values = BTreeMap::from([
            metric_value("first_pass_yield", Some(0.8), now()),
            metric_value("scrap_rate", Some(0.05), now()),
        ]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        assert_eq!(status_of(&report, "above-on"), ScenarioStatus::Met);
        assert_eq!(status_of(&report, "above-under"), ScenarioStatus::NotMet);
        assert_eq!(status_of(&report, "below-on"), ScenarioStatus::Met);
        assert_eq!(status_of(&report, "below-over"), ScenarioStatus::NotMet);
        assert_eq!(status_of(&report, "band"), ScenarioStatus::Met);
        assert_eq!(report.attributes[0].status, ScenarioStatus::NotMet, "the rollup is the worst");
        assert_eq!(report.attributes[0].scenarios[0].value, Some(0.8));
    }

    #[test]
    fn a_metric_with_no_value_an_uncomputed_value_or_no_registry_entry_is_no_data() {
        let tree = tree_with(&[
            ("missing", Some("{ metric: throughput_week, above: 5 }")),
            ("uncomputed", Some("{ metric: scrap_rate, below: 0.05 }")),
            ("cost", Some("{ metric: unit_cost, below: 1 }")),
            ("unknown", Some("{ metric: not_a_metric, below: 0.05 }")),
        ]);
        let values = BTreeMap::from([metric_value("scrap_rate", None, now())]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        for (_, status) in statuses(&report) {
            assert_eq!(status, ScenarioStatus::NoData);
        }
        let reasons = |id: &str| {
            report.attributes[0].scenarios.iter().find(|s| s.scenario.scenario.id == id).unwrap().reasons.join(" ")
        };
        assert!(reasons("uncomputed").contains("no finished runs"), "{}", reasons("uncomputed"));
        assert!(reasons("cost").contains("no value for unit_cost"), "{}", reasons("cost"));
        assert!(reasons("unknown").contains("not a known metric"), "{}", reasons("unknown"));
    }

    #[test]
    fn a_metric_value_older_than_its_max_age_is_stale_never_met() {
        let tree = tree_with(&[
            ("fresh", Some("{ metric: first_pass_yield, above: 0.5, max_age: 7d }")),
            ("old", Some("{ metric: first_pass_yield, above: 0.5, max_age: 1d }")),
        ]);
        let values = BTreeMap::from([metric_value("first_pass_yield", Some(0.9), now() - chrono::Duration::days(3))]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        assert_eq!(status_of(&report, "fresh"), ScenarioStatus::Met);
        assert_eq!(status_of(&report, "old"), ScenarioStatus::Stale);
    }

    #[test]
    fn a_scenario_without_a_measure_is_a_draft_and_an_empty_attribute_rolls_up_to_draft() {
        let tree = tree_with(&[("draft", None), ("bare", Some("{ metric: scrap_rate }"))]);
        let report = evaluate(&tree, &BTreeMap::new(), &Evidence::default(), now());
        assert_eq!(status_of(&report, "draft"), ScenarioStatus::Draft);
        assert_eq!(status_of(&report, "bare"), ScenarioStatus::Draft);
        assert_eq!(ScenarioStatus::worst([]), ScenarioStatus::Draft);
        assert_eq!(ScenarioStatus::worst([ScenarioStatus::Met, ScenarioStatus::Stale, ScenarioStatus::Draft]), ScenarioStatus::Stale);
    }

    fn task(id: &str, status: RunStatus, ended_days_ago: Option<i64>) -> Vec<TaskFact> {
        vec![TaskFact {
            id: id.to_string(),
            title: "quality-gate".to_string(),
            runs: vec![RunFact {
                id: format!("{id}-run"),
                status,
                started_at: now() - chrono::Duration::days(30),
                ended_at: ended_days_ago.map(|d| now() - chrono::Duration::days(d)),
            }],
        }]
    }

    #[test]
    fn a_check_measure_goes_through_policy_evaluate_and_distinguishes_no_data_from_not_met() {
        let tree = tree_with(&[("gate", Some("{ check: task, task: quality-gate, max_age: 7d }"))]);
        let judge = |facts: Option<Vec<TaskFact>>| {
            let mut evidence = Evidence::default();
            if let Some(f) = facts {
                evidence.tasks.insert("quality-gate".to_string(), f);
            }
            let report = evaluate(&tree, &BTreeMap::new(), &evidence, now());
            report.attributes[0].scenarios[0].clone()
        };

        let done = judge(Some(task("t1", RunStatus::Done, Some(1))));
        assert_eq!(done.status, ScenarioStatus::Met);
        assert!(done.refs.iter().any(|r| r.id == "t1-run"), "policy's own refs come through: {:?}", done.refs);
        assert_eq!(judge(Some(task("t1", RunStatus::Done, Some(10)))).status, ScenarioStatus::Stale);
        assert_eq!(judge(Some(task("t1", RunStatus::Failed, Some(1)))).status, ScenarioStatus::NotMet);
        assert_eq!(judge(Some(task("t1", RunStatus::Running, None))).status, ScenarioStatus::NoData, "nothing finished yet");
        assert_eq!(judge(Some(vec![])).status, ScenarioStatus::NoData, "no task by that name");
        assert_eq!(judge(None).status, ScenarioStatus::NoData, "never gathered");
    }

    #[test]
    fn a_gate_or_sandbox_check_reads_no_data_until_its_evidence_is_gathered() {
        let tree = tree_with(&[("gate", Some("{ check: gate, dataset: smoke }")), ("sandbox", Some("{ check: sandbox }"))]);
        let report = evaluate(&tree, &BTreeMap::new(), &Evidence::default(), now());
        assert_eq!(status_of(&report, "gate"), ScenarioStatus::NoData);
        assert_eq!(status_of(&report, "sandbox"), ScenarioStatus::NoData);

        let evidence = Evidence {
            gates: BTreeMap::from([(
                "smoke".to_string(),
                GateFact { run_id: "b1".into(), ended_at: Some(now()), cases: vec![] },
            )]),
            agents: Some(vec![]),
            ..Default::default()
        };
        let report = evaluate(&tree, &BTreeMap::new(), &evidence, now());
        assert_eq!(status_of(&report, "sandbox"), ScenarioStatus::Met, "an empty scope has no unsandboxed agent");
        assert_eq!(status_of(&report, "gate"), ScenarioStatus::NoData, "a settled run with nothing gated judges nothing");

        let gate_with = |gated: bool, verdict: Verdict| {
            let evidence = Evidence {
                gates: BTreeMap::from([(
                    "smoke".to_string(),
                    GateFact {
                        run_id: "b1".into(),
                        ended_at: Some(now()),
                        cases: vec![GateCase { id: "c1".into(), gated, verdicts: vec![verdict] }],
                    },
                )]),
                ..Default::default()
            };
            status_of(&evaluate(&tree, &BTreeMap::new(), &evidence, now()), "gate")
        };
        assert_eq!(gate_with(false, Verdict::Fail), ScenarioStatus::NoData);
        assert_eq!(gate_with(true, Verdict::Fail), ScenarioStatus::NotMet);
        assert_eq!(gate_with(true, Verdict::Pass), ScenarioStatus::Met);
    }

    #[test]
    fn a_named_gate_case_that_is_missing_or_ungated_is_no_data() {
        let tree = tree_with(&[("gate", Some("{ check: gate, dataset: smoke, case: c2 }"))]);
        let evidence = Evidence {
            gates: BTreeMap::from([(
                "smoke".to_string(),
                GateFact {
                    run_id: "b1".into(),
                    ended_at: Some(now()),
                    cases: vec![GateCase { id: "c1".into(), gated: true, verdicts: vec![Verdict::Pass] }],
                },
            )]),
            ..Default::default()
        };
        assert_eq!(status_of(&evaluate(&tree, &BTreeMap::new(), &evidence, now()), "gate"), ScenarioStatus::NoData);
    }

    // -- review fixes: NaN, bands, judgeability, vocabularies, huge max_age --

    #[test]
    fn a_non_finite_bound_is_a_finding_at_load_and_the_measure_is_dropped() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n  - id: reliability.availability\n    importance: H\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: nan, measure: { metric: first_pass_yield, above: .nan } }\n\
             \x20     - { id: inf, measure: { metric: scrap_rate, below: .inf } }\n",
        )]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::NonFiniteThreshold, FindingKind::NonFiniteThreshold]);
        assert!(q.profiles["p"].attributes[0].scenarios.iter().all(|s| s.measure.is_none()));
        let (_, findings) = applicable(&q, "demo", &[layer("demo", &["p"])]);
        assert_eq!(kinds(&findings), vec![FindingKind::UnmeasuredHighImportance], "a dropped measure measures nothing");
    }

    #[test]
    fn a_non_finite_bound_from_a_descendant_is_a_conflict_never_a_silent_loosening() {
        let root = catalogue_from(&[("root.yaml", ROOT)]);
        let mut q = root.clone();
        let mut child: Profile = serde_yaml_ng::from_str(
            "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: up, measure: { metric: first_pass_yield, above: 0.9 } }\n\
             \x20     - { id: draft, measure: { metric: scrap_rate, below: 0.1 } }\n",
        )
        .unwrap();
        // Built by hand, past `load`'s own guard, to prove the merge holds too.
        for s in &mut child.attributes[0].scenarios {
            if let Some(Measure::Metric(m)) = &mut s.measure {
                m.above = m.above.map(|_| f64::NAN);
                m.below = m.below.map(|_| f64::NAN);
            }
        }
        q.profiles.insert("child".to_string(), child);
        let (tree, findings) = applicable(&q, "demo", &[layer("company", &["root"]), layer("demo", &["child"])]);
        assert_eq!(kinds(&findings), vec![FindingKind::ConflictingOverride, FindingKind::ConflictingOverride]);
        let Some(Measure::Metric(m)) = &scenario(&tree, "reliability.availability", "up").measure else { panic!() };
        assert_eq!(m.above, Some(0.8));
        assert!(scenario(&tree, "reliability.availability", "draft").measure.is_none());
    }

    #[test]
    fn a_non_finite_bound_or_value_never_reads_as_met() {
        let mut tree = tree_with(&[("up", Some("{ metric: first_pass_yield, above: 0.5 }"))]);
        let values = BTreeMap::from([metric_value("first_pass_yield", Some(f64::NAN), now())]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        assert_eq!(status_of(&report, "up"), ScenarioStatus::NoData);

        if let Some(Measure::Metric(m)) = &mut tree.attributes[0].scenarios[0].scenario.measure {
            m.above = Some(f64::NAN);
        }
        let values = BTreeMap::from([metric_value("first_pass_yield", Some(0.9), now())]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        assert_eq!(status_of(&report, "up"), ScenarioStatus::Draft);
    }

    #[test]
    fn a_bound_less_measure_does_not_clear_the_unmeasured_h_finding() {
        let q = catalogue_from(&[
            ("root.yaml", "attributes:\n  - { id: safety, importance: H, difficulty: L, scenarios: [ { id: s } ] }\n"),
            (
                "child.yaml",
                "attributes:\n  - { id: safety, importance: H, difficulty: L, scenarios: [ { id: s, measure: { metric: scrap_rate } } ] }\n",
            ),
        ]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::MissingThreshold]);
        let (tree, findings) = applicable(&q, "demo", &[layer("company", &["root"]), layer("demo", &["child"])]);
        assert_eq!(kinds(&findings), vec![FindingKind::UnmeasuredHighImportance]);
        assert!(!is_judgeable(tree.attributes[0].scenarios[0].scenario.measure.as_ref().unwrap()));
        assert!(is_judgeable(&Measure::Check(Check::Sandbox)));
    }

    #[test]
    fn a_band_nothing_can_meet_is_a_finding_in_one_file_or_after_a_merge() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n  - id: safety\n    importance: L\n    difficulty: L\n    scenarios:\n\
             \x20     - { id: s, measure: { metric: scrap_rate, above: 0.2, below: 0.1 } }\n",
        )]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::ImpossibleBand]);

        let (_, findings) = chained(
            "attributes:\n  - id: reliability.availability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: up, measure: { metric: first_pass_yield, below: 0.5 } }\n",
        );
        assert_eq!(kinds(&findings), vec![FindingKind::ImpossibleBand], "0.8 <= x <= 0.5 is empty");
    }

    #[test]
    fn a_mistyped_daemon_fact_or_secrets_location_is_a_finding_at_load() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n  - id: security\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: loopback, measure: { check: daemon, fact: http_loopback } }\n\
             \x20     - { id: no-keys, measure: { check: secrets, absent: [github, gitlab] } }\n\
             \x20     - { id: fine, measure: { check: daemon, fact: http_loopback_only } }\n",
        )]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::UnknownDaemonFact, FindingKind::UnknownSecretsLocation]);
        assert!(q.findings[0].detail.starts_with("security/loopback names daemon fact"), "{}", q.findings[0].detail);
    }

    #[test]
    fn an_absurd_max_age_never_panics_and_never_goes_stale() {
        let tree = tree_with(&[("up", Some("{ metric: first_pass_yield, above: 0.5, max_age: 9999999999999999h }"))]);
        let values = BTreeMap::from([metric_value("first_pass_yield", Some(0.9), now() - chrono::Duration::days(3650))]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        assert_eq!(status_of(&report, "up"), ScenarioStatus::Met);

        let tree = tree_with(&[("gate", Some("{ check: task, task: quality-gate, max_age: 9999999999999999h }"))]);
        let evidence = Evidence {
            tasks: BTreeMap::from([("quality-gate".to_string(), task("t1", RunStatus::Done, Some(3650)))]),
            ..Default::default()
        };
        let report = evaluate(&tree, &BTreeMap::new(), &evidence, now());
        assert_eq!(status_of(&report, "gate"), ScenarioStatus::Met);
    }

    #[test]
    fn a_report_carries_no_aggregate_score() {
        let tree = tree_with(&[("gate", Some("{ check: sandbox }"))]);
        let json = serde_json::to_value(evaluate(&tree, &BTreeMap::new(), &Evidence::default(), now())).unwrap();
        let keys: Vec<&String> = json.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["attributes", "profiles", "scope", "tradeoffs"]);
        let attr = json["attributes"][0].as_object().unwrap();
        assert!(!attr.keys().any(|k| k.contains("score")), "{attr:?}");
        assert_eq!(json["attributes"][0]["status"], "no_data");
        assert_eq!(json["attributes"][0]["scenarios"][0]["status"], "no_data");
    }

    #[test]
    fn an_attestation_measure_reads_no_data_with_the_reason_never_not_met() {
        let tree = tree_with(&[("signed-off", Some("{ check: attestation }"))]);
        let report = evaluate(&tree, &BTreeMap::new(), &Evidence::default(), now());
        let s = &report.attributes[0].scenarios[0];
        assert_eq!(s.status, ScenarioStatus::NoData);
        assert_eq!(s.reasons, vec![ATTESTATION_UNSUPPORTED.to_string()]);
    }

    #[test]
    fn a_measure_on_a_quality_metric_is_a_finding_and_reads_no_data() {
        let q = catalogue_from(&[(
            "p.yaml",
            "attributes:\n  - id: reliability\n    importance: M\n    difficulty: M\n    scenarios:\n\
             \x20     - { id: loop, measure: { metric: quality.reliability, above: 0.5 } }\n",
        )]);
        assert_eq!(kinds(&q.findings), vec![FindingKind::SelfReferentialMetric]);
        let (tree, _) = applicable(&q, "demo", &[layer("demo", &["p"])]);
        let values = BTreeMap::from([metric_value("quality.reliability", Some(1.0), now())]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        assert_eq!(report.attributes[0].scenarios[0].status, ScenarioStatus::NoData, "even with a value on hand");
    }

    #[test]
    fn applied_checks_is_exactly_the_check_measures_each_as_evaluate_judges_it() {
        let tree = tree_with(&[
            ("metric", Some("{ metric: scrap_rate, below: 0.1 }")),
            ("draft", None),
            ("gate", Some("{ check: task, task: quality-gate, max_age: 7d }")),
            ("sandboxed", Some("{ check: sandbox }")),
        ]);
        let applied = applied_checks(&tree);
        let ids: Vec<String> = applied.iter().map(|a| a.control.id.clone()).collect();
        assert_eq!(ids, vec!["reliability.availability/gate", "reliability.availability/sandboxed"]);
        assert_eq!(applied[0], check_as_applied("reliability.availability", "gate", &Check::Task {
            task: "quality-gate".into(),
            max_age: Some("7d".parse().unwrap()),
        }));
        assert_eq!(applied[0].max_age, Some("7d".parse().unwrap()), "the check's own freshness window carries over");
    }

    #[test]
    fn a_scope_report_round_trips_through_json() {
        let tree = tree_with(&[("metric", Some("{ metric: scrap_rate, below: 0.1 }")), ("draft", None)]);
        let values = BTreeMap::from([metric_value("scrap_rate", Some(0.05), now())]);
        let report = evaluate(&tree, &values, &Evidence::default(), now());
        let back: ScopeReport = serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();
        assert_eq!(back, report);
    }

    // -- examples ------------------------------------------------------------------

    #[test]
    fn examples_quality_load_with_zero_findings() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/quality");
        let q = load(&dir);
        assert!(q.findings.is_empty(), "{:#?}", q.findings);
        assert_eq!(q.profiles.keys().collect::<Vec<_>>(), vec!["baseline", "daemon-service"]);

        for chain in [
            vec![layer("company", &["baseline"])],
            vec![layer("factory", &["daemon-service"])],
            vec![layer("company", &["baseline"]), layer("factory", &["daemon-service"])],
        ] {
            let (tree, findings) = applicable(&q, "factory", &chain);
            assert!(findings.is_empty(), "{chain:?}: {findings:#?}");
            assert!(tree.attributes.len() <= MAX_ATTRIBUTES);
        }

        let (tree, _) = applicable(
            &q,
            "factory",
            &[layer("company", &["baseline"]), layer("factory", &["daemon-service"])],
        );
        let recoverability = tree.attributes.iter().find(|a| a.id == "reliability.recoverability").unwrap();
        assert_eq!(recoverability.importance, Level::High, "the daemon-service profile raises the baseline's M");
        assert_eq!(recoverability.scenarios.len(), 2);
    }
}
