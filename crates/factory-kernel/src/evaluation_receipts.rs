//! Plain shared identifiers and authored receipt values, not live facts.
//! No evaluation, clock arithmetic, stores or providers belong here.
use crate::is_slug;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A control's stable identity, everywhere but inside the file that defines
/// it: `framework/id`, e.g. `cra/annex-i-2-1`. Serializes as exactly that
/// string, never as a two-field object -- it is a name, not a record.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ControlRef {
    pub framework: String,
    pub id: String,
}

impl ControlRef {
    pub fn new(framework: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            framework: framework.into(),
            id: id.into(),
        }
    }

    /// `control/<framework>/<id>` -- the knowledge tag a `knowledge` check
    /// looks for when the control does not name one of its own. Amends the
    /// ADR's `control:<framework>/<id>`; see the module doc comment.
    pub fn default_tag(&self) -> String {
        format!("control/{}/{}", self.framework, self.id)
    }
}

impl std::fmt::Display for ControlRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.framework, self.id)
    }
}

impl std::str::FromStr for ControlRef {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let (framework, id) = s
            .split_once('/')
            .ok_or_else(|| format!("{s:?} is not framework/id"))?;
        if !is_slug(framework) || !is_slug(id) {
            return Err(format!(
                "{s:?} is not framework/id, each matching [a-z0-9][a-z0-9-]*"
            ));
        }
        Ok(ControlRef {
            framework: framework.to_string(),
            id: id.to_string(),
        })
    }
}

impl TryFrom<String> for ControlRef {
    type Error = String;

    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        s.parse()
    }
}

impl From<ControlRef> for String {
    fn from(r: ControlRef) -> String {
        r.to_string()
    }
}

/// One thing the clock counts: an exploited L2 finding or a confirmed L4
/// security report. The CLI's text form (`factory policy attest
/// --clock-item`) is `finding:<scope>:<vulnerability>` -- parsed by finding
/// the *last* `:` in `<scope>:<vulnerability>`, so everything after it is
/// the vulnerability id and everything before is the scope -- or
/// `report:<task-id>`; see [`std::str::FromStr`] below. Every vulnerability
/// id this clock actually sees is a bare CVE/GHSA/EUVD id with no `:` of its
/// own, so this only ever matters for the ordinary, single-colon case; a
/// hypothetical colon-bearing id (a raw CPE URI, never how CycloneDX names a
/// vulnerability) would parse with everything but its last segment folded
/// into `scope`. The wire form (`ClockMark.item`) is this tagged enum, not
/// that string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClockItemRef {
    Finding {
        scope: String,
        vulnerability: String,
    },
    Report {
        item: String,
    },
}

impl std::fmt::Display for ClockItemRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClockItemRef::Finding {
                scope,
                vulnerability,
            } => {
                write!(f, "finding:{scope}:{vulnerability}")
            }
            ClockItemRef::Report { item } => write!(f, "report:{item}"),
        }
    }
}

impl std::str::FromStr for ClockItemRef {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        let bad = || format!("{s:?} is not finding:<scope>:<vulnerability> or report:<task-id>");
        let (kind, rest) = s.split_once(':').ok_or_else(bad)?;
        match kind {
            "finding" => {
                let (scope, vulnerability) = rest.rsplit_once(':').ok_or_else(bad)?;
                if scope.is_empty() || vulnerability.is_empty() {
                    return Err(bad());
                }
                Ok(ClockItemRef::Finding {
                    scope: scope.to_string(),
                    vulnerability: vulnerability.to_string(),
                })
            }
            "report" => {
                if rest.is_empty() {
                    return Err(bad());
                }
                Ok(ClockItemRef::Report {
                    item: rest.to_string(),
                })
            }
            _ => Err(bad()),
        }
    }
}

/// Which reporting deadline a submission counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockDeadlineKind {
    EarlyWarning,
    Notification,
    FinalReport,
}

impl ClockDeadlineKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EarlyWarning => "early_warning",
            Self::Notification => "notification",
            Self::FinalReport => "final_report",
        }
    }
}

impl std::fmt::Display for ClockDeadlineKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ClockDeadlineKind {
    type Err = String;

    /// Accepts both the wire spelling (`early_warning`) and the CLI's own
    /// hyphenated one (`early-warning`) -- a deliberate looseness on the one
    /// argument a person types by hand, not extended to anything else here.
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.replace('-', "_").as_str() {
            "early_warning" => Ok(Self::EarlyWarning),
            "notification" => Ok(Self::Notification),
            "final_report" => Ok(Self::FinalReport),
            _ => Err(format!(
                "{s:?} is not early-warning, notification or final-report"
            )),
        }
    }
}

/// What a submission against the clock records, carried on an ordinary
/// [`Attestation`] (`Attestation.clock`) rather than a status table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockMark {
    pub item: ClockItemRef,
    pub deadline: ClockDeadlineKind,
}

/// Evidenced availability of a corrective/mitigating measure. This is an
/// authored observation, never inferred from a commit or a task finishing.
/// Recorded append-only with the ordinary policy.attest grant and evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorrectiveMeasureMark {
    pub item: ClockItemRef,
    pub available_at: DateTime<Utc>,
}

/// One person's word that a control is met, with an expiry -- the only
/// check kind a person satisfies by saying so, and the only new state this
/// slice introduces (everything else is a lookup against what already
/// exists). Recorded through the API into the instance database,
/// append-only, once `policy.attest` exists (`#76` or later); this module
/// only reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attestation {
    pub id: String,
    pub control: ControlRef,
    pub scope: String,
    /// A pointer to the evidence -- a document, a ticket, a page -- not the
    /// evidence itself.
    pub evidence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub attested_by: String,
    pub attested_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub withdrawn: Option<Withdrawal>,
    /// A submission against the CRA Art. 14 reporting clock, phase 1 of
    /// `#157` -- absent for every attestation recorded before this and for
    /// an ordinary one recorded since. `direct_status`'s `attestation`
    /// check skips a row that carries one: a single notification is not the
    /// whole control being met. `policy_attest` (`factory-daemon/src/
    /// policies/mod.rs`) is the only writer, and only for `cra/art-14`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockMark>,
    /// An evidenced corrective-measure availability time for the final
    /// report clock; not an attestation that the whole control is met.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corrective: Option<CorrectiveMeasureMark>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withdrawal {
    pub at: DateTime<Utc>,
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
