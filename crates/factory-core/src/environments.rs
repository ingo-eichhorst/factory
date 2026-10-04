//! Operations of running systems (`#185`): the environments a scope deploys
//! what it builds to, what is running on each, whether it is healthy, and
//! whether it meets its SLO -- not Factory's own production line, which is
//! `operations.rs` (the L4 "Line" tab) and stays as it is.
//!
//! Four nouns:
//!
//! - an **environment** is declared, authored config: `scope.environments`
//!   in a scope's `.factory/config.yaml` ([`EnvironmentDecl`]). Its name is
//!   unique across the instance, because `availability.<env>` has to name
//!   exactly one.
//! - a **release** is a deployable version: a commit of the scope's
//!   repository ([`Release`]). The catalogue is what was recorded -- built,
//!   deployed or added by hand -- never read off git tags.
//! - a **deployment** is one attempt to put a release on an environment
//!   ([`Deployment`]), recorded by whoever made it: a run, an agent, a script
//!   or a person.
//! - a **sample** is one health check's answer at one moment ([`Sample`]).
//!   Status, incidents and every SLA figure are computed from samples, never
//!   typed in.
//!
//! This module holds the vocabulary and every decision -- status, incidents,
//! availability, error budget, the four DORA keys -- as pure functions over
//! those records, tested on their own. Running a check, keeping samples and
//! recording deployments is `factory-daemon`'s job.

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{FactoryError, Result};

// ================================================================== config

/// How many samples a check keeps: the daemon prunes anything older, so an
/// SLO window may not ask for more.
pub const SAMPLE_RETENTION_DAYS: i64 = 90;

/// Consecutive failures of one check that open an incident. One failed
/// sample is a blip; two in a row is the service being down.
pub const INCIDENT_THRESHOLD: usize = 2;

/// How long after a deployment finished an incident still counts against
/// it in the change failure rate.
pub const CHANGE_FAILURE_HORIZON_HOURS: i64 = 1;

/// The window SLA figures and the DORA keys use when an environment declares
/// no SLO of its own.
pub const DEFAULT_WINDOW_DAYS: i64 = 28;

/// The fewest seconds between two runs of one check. A tighter loop is a
/// load test, not a health check.
pub const MIN_EVERY_SECONDS: u64 = 5;

/// The longest a single check may take.
pub const MAX_TIMEOUT_SECONDS: u64 = 120;

/// A span of time as a person writes it in YAML: `30s`, `5m`, `2h`, `28d`.
/// Kept as the text it was written as, so a config written back reads the
/// same.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Span(String);

impl Span {
    pub fn seconds(&self) -> u64 {
        parse_span(&self.0).expect("validated when parsed")
    }
}

impl TryFrom<String> for Span {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, String> {
        parse_span(&s)?;
        Ok(Span(s.trim().to_string()))
    }
}

impl From<Span> for String {
    fn from(s: Span) -> String {
        s.0
    }
}

/// `<n><unit>`, unit one of `s`, `m`, `h`, `d`; `n` a positive integer.
pub fn parse_span(text: &str) -> std::result::Result<u64, String> {
    let t = text.trim();
    let bad = || format!("{text:?} is not a span of time; write it as 30s, 5m, 2h or 28d");
    let unit = t.chars().last().ok_or_else(bad)?;
    let n: u64 = t[..t.len() - unit.len_utf8()].parse().map_err(|_| bad())?;
    let mult = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => return Err(bad()),
    };
    if n == 0 {
        return Err(bad());
    }
    n.checked_mul(mult).ok_or_else(bad)
}

/// An availability target, written `99.5%` (or the bare number `99.5`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "serde_yaml_ng::Value", into = "String")]
pub struct Percent(String);

impl Percent {
    /// The target as a ratio, `0.995` for `99.5%`.
    pub fn ratio(&self) -> f64 {
        parse_percent(&self.0).expect("validated when parsed") / 100.0
    }
}

impl TryFrom<serde_yaml_ng::Value> for Percent {
    type Error = String;
    fn try_from(v: serde_yaml_ng::Value) -> std::result::Result<Self, String> {
        let text = match v {
            serde_yaml_ng::Value::String(s) => s,
            serde_yaml_ng::Value::Number(n) => format!("{n}%"),
            other => return Err(format!("{other:?} is not a percentage; write it as 99.5%")),
        };
        parse_percent(&text)?;
        Ok(Percent(text.trim().to_string()))
    }
}

impl From<Percent> for String {
    fn from(p: Percent) -> String {
        p.0
    }
}

fn parse_percent(text: &str) -> std::result::Result<f64, String> {
    let t = text.trim().trim_end_matches('%').trim();
    let v: f64 = t
        .parse()
        .map_err(|_| format!("{text:?} is not a percentage; write it as 99.5%"))?;
    if !(v > 0.0 && v < 100.0) {
        return Err(format!("{text:?}: an availability target is above 0% and below 100%"));
    }
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// A review environment, meant to be thrown away. What an environment
    /// nobody declared is taken to be.
    #[default]
    Ephemeral,
    Staging,
    Production,
}

/// One environment, as a scope declares it.
///
/// ```yaml
/// environments:
///   - name: production
///     tier: production
///     url: https://factory.example.ts.net:8790
///     checks:
///       - { kind: http, path: /api/status, expect: 200, every: 60s, timeout: 5s }
///     slo: { availability: 99.5%, window: 28d }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDecl {
    pub name: String,
    #[serde(default)]
    pub tier: Tier,
    /// Where a person reaches it, and what an `http` check's `path` is
    /// appended to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The environment a release here goes to next: `staging` names
    /// `production`. The chain is a promotion path; the DORA keys are read
    /// at its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotes_to: Option<String>,
    /// Opt-in release recipe for promoting a verified release here. Executed
    /// only as an approved category-release task, never by the health loop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy: Option<ReleaseCommand>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<CheckDecl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slo: Option<Slo>,
    /// Stop running its checks. Its status reads `unknown`, and nothing is
    /// sampled -- a paused stretch neither spends nor earns error budget.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseCommand {
    /// Declared shell agent, with deploy.record in this environment's scope.
    pub agent: String,
    pub command: String,
    /// Build or prepare evidence before release policy gates judge it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare: Option<String>,
    /// Run timeout, 30 minutes unless declared; at most one hour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Span>,
}

impl ReleaseCommand {
    pub fn timeout_seconds(&self) -> u64 {
        self.timeout.as_ref().map(Span::seconds).unwrap_or(1800)
    }
}

/// Select exactly the deployment shown on the source card, not a moving
/// branch name. The daemon freezes its release and the target recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Promote {
    pub environment: String,
    pub deployment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slo {
    pub availability: Percent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Span>,
}

impl Slo {
    pub fn window_days(&self) -> i64 {
        self.window
            .as_ref()
            .map(|w| (w.seconds() / 86_400) as i64)
            .unwrap_or(DEFAULT_WINDOW_DAYS)
            .max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// A GET: the status it answers with, and optionally a string its body
    /// contains.
    Http,
    /// A TCP connect.
    Tcp,
    /// A shell command, run in the scope's directory: exit 0 is healthy.
    Command,
}

/// One health check. Which fields apply depends on `kind`; `validate` says
/// which one is missing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckDecl {
    /// What the check is called on the page and in its samples. Defaults to
    /// its target: the path, the URL, `host:port` or the command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: CheckKind,
    /// `http`: appended to the environment's `url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// `http`: a whole URL, instead of `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// `http`: the status a healthy answer has. 200 unless said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<u16>,
    /// `http`: a string a healthy answer's body contains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// `tcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// `command`: run with `sh -c`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// How often. 60s unless said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<Span>,
    /// How long one run may take before it is killed and counted a failure.
    /// 10s unless said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Span>,
    /// A successful answer slower than this is degraded, not unavailable.
    /// Captured on the sample so changing this threshold does not rewrite history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_after_ms: Option<u64>,
}

impl CheckDecl {
    pub fn every_seconds(&self) -> u64 {
        self.every.as_ref().map(Span::seconds).unwrap_or(60)
    }

    pub fn timeout_seconds(&self) -> u64 {
        self.timeout.as_ref().map(Span::seconds).unwrap_or(10)
    }

    /// The URL an `http` check asks, given its environment's `url`.
    pub fn http_url(&self, env_url: Option<&str>) -> Option<String> {
        if let Some(url) = &self.url {
            return Some(url.clone());
        }
        let base = env_url?.trim_end_matches('/');
        Some(format!("{base}{}", self.path.as_deref().unwrap_or("/")))
    }

    /// What it is aimed at, for a person: the URL, `host:port` or command.
    pub fn target(&self, env_url: Option<&str>) -> String {
        match self.kind {
            CheckKind::Http => self.http_url(env_url).unwrap_or_default(),
            CheckKind::Tcp => format!(
                "{}:{}",
                self.host.as_deref().unwrap_or_default(),
                self.port.map(|p| p.to_string()).unwrap_or_default()
            ),
            CheckKind::Command => self.command.clone().unwrap_or_default(),
        }
    }

    /// Its name: declared, else its path or target.
    pub fn display_name(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        match (self.kind, &self.path) {
            (CheckKind::Http, Some(path)) if self.url.is_none() => path.clone(),
            _ => self.target(None).chars().take(80).collect(),
        }
    }
}

/// Whether `name` can be an environment's name: a metric id segment
/// (`[a-z0-9][a-z0-9_-]*`), since `availability.<env>` is one.
pub fn is_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// Check every scope's environments together: names are unique across the
/// instance, `promotes_to` names one of them, and every check has what its
/// kind needs. `scopes` is `(scope name, its declarations)`.
pub fn validate<'a>(scopes: impl IntoIterator<Item = (&'a str, &'a [EnvironmentDecl])>) -> Result<()> {
    let bad = |m: String| Err(FactoryError::BadRequest(m));
    let scopes: Vec<(&str, &[EnvironmentDecl])> = scopes.into_iter().collect();
    let mut owner: std::collections::BTreeMap<&str, &str> = Default::default();
    for (scope, envs) in &scopes {
        for env in envs.iter() {
            if !is_env_name(&env.name) {
                return bad(format!(
                    "scope {scope:?} declares environment {:?}: an environment name is lowercase letters, digits, \
                     `-` and `_`, starting with a letter or digit, since it names metrics like availability.<env>",
                    env.name
                ));
            }
            if let Some(other) = owner.insert(&env.name, scope) {
                return bad(format!(
                    "environment {:?} is declared by scope {other:?} and by scope {scope:?}; an environment name \
                     is unique across the instance",
                    env.name
                ));
            }
        }
    }
    for (scope, envs) in &scopes {
        for env in envs.iter() {
            let here = format!("scope {scope:?}, environment {:?}", env.name);
            if let Some(deploy) = &env.deploy {
                if deploy.agent.trim().is_empty() || deploy.command.trim().is_empty()
                    || deploy.prepare.as_ref().is_some_and(|command| command.trim().is_empty())
                    || deploy.timeout_seconds() > 3600 {
                    return bad(format!("{here}: deploy needs agent and command, a nonempty prepare if declared, and timeout at most 1h"));
                }
            }
            if let Some(next) = &env.promotes_to {
                if next == &env.name || !owner.contains_key(next.as_str()) {
                    return bad(format!(
                        "{here}: promotes_to {next:?} names no other declared environment"
                    ));
                }
            }
            if let Some(url) = &env.url {
                if !(url.starts_with("http://") || url.starts_with("https://")) {
                    return bad(format!("{here}: url {url:?} is not an http:// or https:// URL"));
                }
            }
            if let Some(slo) = &env.slo {
                if slo.window_days() > SAMPLE_RETENTION_DAYS {
                    return bad(format!(
                        "{here}: an SLO window is at most {SAMPLE_RETENTION_DAYS}d, which is how long samples are kept"
                    ));
                }
            }
            let mut names = std::collections::BTreeSet::new();
            for check in &env.checks {
                let name = check.display_name();
                if !names.insert(name.clone()) {
                    return bad(format!("{here}: two checks are called {name:?}; give one a `name:`"));
                }
                let every = check.every_seconds();
                let timeout = check.timeout_seconds();
                if every < MIN_EVERY_SECONDS {
                    return bad(format!("{here}, check {name:?}: every is at least {MIN_EVERY_SECONDS}s"));
                }
                if timeout > MAX_TIMEOUT_SECONDS || timeout > every {
                    return bad(format!(
                        "{here}, check {name:?}: timeout is at most {MAX_TIMEOUT_SECONDS}s and no longer than every"
                    ));
                }
                if check.slow_after_ms.is_some_and(|ms| ms == 0 || ms >= timeout.saturating_mul(1000)) {
                    return bad(format!(
                        "{here}, check {name:?}: slow_after_ms is positive and below timeout in milliseconds"
                    ));
                }
                match check.kind {
                    CheckKind::Http => {
                        if let Some(path) = &check.path {
                            if !path.starts_with('/') {
                                return bad(format!("{here}, check {name:?}: path {path:?} starts with /"));
                            }
                        }
                        match (&check.url, &env.url) {
                            (Some(url), _) if !(url.starts_with("http://") || url.starts_with("https://")) => {
                                return bad(format!("{here}, check {name:?}: url {url:?} is not an http(s) URL"));
                            }
                            (None, None) => {
                                return bad(format!(
                                    "{here}, check {name:?}: an http check needs the environment's url or its own"
                                ));
                            }
                            _ => {}
                        }
                    }
                    CheckKind::Tcp => {
                        if check.host.as_deref().unwrap_or("").is_empty() || check.port.is_none() {
                            return bad(format!("{here}, check {name:?}: a tcp check needs host and port"));
                        }
                    }
                    CheckKind::Command => {
                        if check.command.as_deref().unwrap_or("").trim().is_empty() {
                            return bad(format!("{here}, check {name:?}: a command check needs command"));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

// ================================================================= records

/// One check's answer at one moment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub environment: String,
    pub check: String,
    pub at: DateTime<Utc>,
    pub ok: bool,
    pub latency_ms: u64,
    /// Successful but slower than the threshold declared when checked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub slow: bool,
    /// What it answered, or why it failed: `200`, `expected 200, got 502`,
    /// `timed out after 5s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Sample {
    pub fn status(&self) -> EnvStatus {
        if !self.ok {
            EnvStatus::Down
        } else if self.slow {
            EnvStatus::Degraded
        } else {
            EnvStatus::Up
        }
    }
}

/// The post-deploy verification: the environment's own checks, run once
/// right after the deployment said it finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployVerification {
    pub at: DateTime<Utc>,
    pub ok: bool,
    pub checks: Vec<Sample>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeployStatus {
    Running,
    Succeeded,
    Failed,
    RolledBack,
}

impl DeployStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    /// A task run, speaking with its own token.
    Run,
    /// A standing agent.
    Agent,
    /// The owner: a person, or a script they started.
    Person,
}

/// Who put a release on an environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub kind: ActorKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

/// What `factory deploy start` says about the release going out.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ReleaseFacts {
    pub commit: String,
    /// `git describe`'s answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub describe: Option<String>,
    /// A version or tag, when the release has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The build profile: `release`, `debug`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    /// Built from a working tree with uncommitted changes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dirty: bool,
    /// Where it was built from: a ref, a branch or a worktree path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// The commit's committer time: where lead time for changes starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_at: Option<DateTime<Utc>>,
}

/// One attempt to put a release on an environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deployment {
    pub id: String,
    pub scope: String,
    pub environment: String,
    pub release: ReleaseFacts,
    pub actor: Actor,
    /// What recorded it, when it was not the actor's own hand: `release.sh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Done by a person with nothing in between: the owner, with no `via`.
    pub manual: bool,
    /// Frozen on start: skipped, paused or removed checks cannot count as success.
    #[serde(default)]
    pub strict_verification: bool,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    pub status: DeployStatus,
    /// Why it failed or was rolled back, in the recorder's words, or which
    /// check failed its verification.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The commit that was running before: what a rollback targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<DeployVerification>,
}

impl Deployment {
    pub fn duration_seconds(&self) -> Option<i64> {
        self.finished_at.map(|f| (f - self.started_at).num_seconds().max(0))
    }

    pub fn finished(&self) -> bool {
        self.status != DeployStatus::Running
    }
}

/// One release in the catalogue: a commit of a scope, and what is known
/// about it. Assembled from deployments and `factory release add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    pub scope: String,
    #[serde(flatten)]
    pub facts: ReleaseFacts,
    pub first_seen: DateTime<Utc>,
    /// Where it is the current release now.
    #[serde(default)]
    pub running_on: Vec<String>,
    /// Deployments of it, by outcome.
    pub deployments: u32,
    pub failed_deployments: u32,
}

// ============================================================== decisions

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvStatus {
    /// Every known check's latest answer was healthy and fast.
    Up,
    /// Some checks failing, some not, or a successful check was slow.
    Degraded,
    /// Every check failing.
    Down,
    /// Nothing checked yet, no checks declared, or checks paused.
    Unknown,
}

/// The status a set of latest answers adds up to. `latest` has one entry
/// per declared check: its newest sample's `ok`, or `None` if it has none.
pub fn status_of(latest: &[Option<bool>]) -> EnvStatus {
    status_of_checks(
        &latest.iter().map(|ok| ok.map(|ok| if ok { EnvStatus::Up } else { EnvStatus::Down })).collect::<Vec<_>>(),
    )
}

/// Aggregate individual check answers, including slow successful responses.
pub fn status_of_checks(latest: &[Option<EnvStatus>]) -> EnvStatus {
    let known: Vec<EnvStatus> = latest.iter().flatten().copied().filter(|s| *s != EnvStatus::Unknown).collect();
    if known.is_empty() {
        EnvStatus::Unknown
    } else if known.iter().all(|s| *s == EnvStatus::Up) {
        EnvStatus::Up
    } else if known.iter().all(|s| *s == EnvStatus::Down) {
        EnvStatus::Down
    } else {
        EnvStatus::Degraded
    }
}

/// The status now and since when, by replaying `samples` (any order) over
/// `checks`, the declared check names. A sample for a check no longer
/// declared is ignored.
pub fn status_timeline(checks: &[String], samples: &[Sample]) -> (EnvStatus, Option<DateTime<Utc>>) {
    let mut ordered: Vec<&Sample> = samples.iter().filter(|s| checks.contains(&s.check)).collect();
    ordered.sort_by_key(|s| s.at);
    let mut latest: Vec<Option<EnvStatus>> = vec![None; checks.len()];
    let mut status = EnvStatus::Unknown;
    let mut since = None;
    for s in ordered {
        let i = checks.iter().position(|c| c == &s.check).expect("filtered");
        latest[i] = Some(s.status());
        let now = status_of_checks(&latest);
        if now != status || since.is_none() {
            status = now;
            since = Some(s.at);
        }
    }
    (status, since)
}

/// A stretch during which an environment was not healthy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Incident {
    pub environment: String,
    pub started_at: DateTime<Utc>,
    /// `None` while it is still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<DateTime<Utc>>,
    /// The checks that failed during it.
    pub checks: Vec<String>,
}

impl Incident {
    pub fn duration_seconds(&self, now: DateTime<Utc>) -> i64 {
        (self.ended_at.unwrap_or(now) - self.started_at).num_seconds().max(0)
    }
}

/// Every incident in `samples` of one environment, oldest first. Per check,
/// [`INCIDENT_THRESHOLD`] consecutive failures open a span at the first of
/// them, and the next success closes it; spans of different checks that
/// overlap or touch are one incident, naming every check in it.
pub fn incidents(environment: &str, samples: &[Sample]) -> Vec<Incident> {
    let mut by_check: std::collections::BTreeMap<&str, Vec<&Sample>> = Default::default();
    for s in samples {
        by_check.entry(s.check.as_str()).or_default().push(s);
    }
    let mut spans: Vec<(DateTime<Utc>, Option<DateTime<Utc>>, String)> = Vec::new();
    for (check, mut list) in by_check {
        list.sort_by_key(|s| s.at);
        let mut run_start: Option<DateTime<Utc>> = None;
        let mut run_len = 0usize;
        for s in list {
            if s.ok {
                if run_len >= INCIDENT_THRESHOLD {
                    spans.push((run_start.expect("a run has a start"), Some(s.at), check.to_string()));
                }
                run_start = None;
                run_len = 0;
            } else {
                run_start.get_or_insert(s.at);
                run_len += 1;
            }
        }
        if run_len >= INCIDENT_THRESHOLD {
            spans.push((run_start.expect("a run has a start"), None, check.to_string()));
        }
    }
    spans.sort_by_key(|(start, _, _)| *start);
    let mut out: Vec<Incident> = Vec::new();
    for (start, end, check) in spans {
        if let Some(last) = out.last_mut() {
            let overlaps = match last.ended_at {
                None => true,
                Some(e) => start <= e,
            };
            if overlaps {
                last.ended_at = match (last.ended_at, end) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    _ => None,
                };
                if !last.checks.contains(&check) {
                    last.checks.push(check);
                }
                continue;
            }
        }
        out.push(Incident { environment: environment.to_string(), started_at: start, ended_at: end, checks: vec![check] });
    }
    out
}

/// The share of healthy samples at or after `since`. `None` with no
/// samples: an environment nobody checked has no availability, not 100%.
pub fn availability(samples: &[Sample], since: DateTime<Utc>) -> Option<f64> {
    let (ok, total) = samples
        .iter()
        .filter(|s| s.at >= since)
        .fold((0u64, 0u64), |(ok, total), s| (ok + s.ok as u64, total + 1));
    (total > 0).then(|| ok as f64 / total as f64)
}

/// What is left of the error budget: `1.0` untouched, `0.0` spent exactly,
/// below zero overspent (kept, not clamped: how far over is the news).
pub fn error_budget(availability: f64, target: f64) -> f64 {
    let budget = 1.0 - target;
    if budget <= 0.0 {
        return 0.0;
    }
    (budget - (1.0 - availability)) / budget
}

/// The nearest-rank percentile of `values`, which it sorts. `None` if empty.
pub fn percentile(values: &mut [f64], p: f64) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let rank = ((p / 100.0) * values.len() as f64).ceil().max(1.0) as usize;
    Some(values[rank.min(values.len()) - 1])
}

/// DORA's four keys for the promotion path ending at one environment, over
/// the window before `now`. Every figure is `None` when there is nothing to
/// compute it from -- never a zero standing in for "no data".
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Dora {
    pub window_days: i64,
    /// Successful deployments per week.
    pub deploy_frequency: Option<f64>,
    /// Median seconds from a released commit's committer time to its
    /// deployment finishing, over successful deployments that say when
    /// their commit was made.
    pub lead_time_p50: Option<f64>,
    /// The share of finished deployments that failed, were rolled back, or
    /// were followed within [`CHANGE_FAILURE_HORIZON_HOURS`] -- before the
    /// next deployment -- by an incident.
    pub change_failure_rate: Option<f64>,
    /// Median seconds an incident lasted, over incidents that ended in the
    /// window.
    pub time_to_restore_p50: Option<f64>,
    /// Mean of the same.
    pub mttr: Option<f64>,
    pub incidents: u32,
}

pub fn dora(deployments: &[Deployment], incidents: &[Incident], window_days: i64, now: DateTime<Utc>) -> Dora {
    let since = now - Duration::days(window_days);
    let mut finished: Vec<&Deployment> = deployments
        .iter()
        .filter(|d| d.finished() && d.finished_at.is_some_and(|f| f >= since && f <= now))
        .collect();
    finished.sort_by_key(|d| d.finished_at);
    let succeeded: Vec<&&Deployment> = finished.iter().filter(|d| d.status == DeployStatus::Succeeded).collect();

    let deploy_frequency = (!succeeded.is_empty()).then(|| succeeded.len() as f64 / (window_days as f64 / 7.0));
    let mut lead: Vec<f64> = succeeded
        .iter()
        .filter_map(|d| {
            let c = d.release.committed_at?;
            Some((d.finished_at? - c).num_seconds().max(0) as f64)
        })
        .collect();
    let lead_time_p50 = percentile(&mut lead, 50.0);

    let horizon = Duration::hours(CHANGE_FAILURE_HORIZON_HOURS);
    let failed_changes = finished
        .iter()
        .enumerate()
        .filter(|(i, d)| match d.status {
            DeployStatus::Failed | DeployStatus::RolledBack => true,
            DeployStatus::Succeeded => {
                let from = d.finished_at.expect("finished");
                let next = finished.get(i + 1).and_then(|n| n.started_at.into());
                let until = next.map(|n: DateTime<Utc>| n.min(from + horizon)).unwrap_or(from + horizon);
                incidents.iter().any(|inc| inc.started_at >= from && inc.started_at < until)
            }
            DeployStatus::Running => false,
        })
        .count();
    let change_failure_rate = (!finished.is_empty()).then(|| failed_changes as f64 / finished.len() as f64);

    let in_window: Vec<&Incident> = incidents
        .iter()
        .filter(|i| i.started_at >= since || i.ended_at.is_none_or(|e| e >= since))
        .collect();
    let mut restores: Vec<f64> = in_window
        .iter()
        .filter(|i| i.ended_at.is_some_and(|e| e >= since))
        .map(|i| i.duration_seconds(now) as f64)
        .collect();
    let mttr = (!restores.is_empty()).then(|| restores.iter().sum::<f64>() / restores.len() as f64);
    let time_to_restore_p50 = percentile(&mut restores, 50.0);

    Dora {
        window_days,
        deploy_frequency,
        lead_time_p50,
        change_failure_rate,
        time_to_restore_p50,
        mttr,
        incidents: in_window.len() as u32,
    }
}

/// One slot of a check's history strip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StripBucket {
    pub start: DateTime<Utc>,
    pub ok: u32,
    pub failed: u32,
    /// Subset of `ok`: successful samples which exceeded their threshold.
    #[serde(default)]
    pub slow: u32,
}

/// `samples` of one check, counted into `buckets` equal slots ending at
/// `now`, oldest first. An empty slot is `0/0`: not checked, not healthy.
pub fn strip(samples: &[Sample], now: DateTime<Utc>, span: Duration, buckets: usize) -> Vec<StripBucket> {
    let buckets = buckets.max(1);
    let start = now - span;
    let width = span.num_seconds().max(1) as f64 / buckets as f64;
    let mut out: Vec<StripBucket> = (0..buckets)
        .map(|i| StripBucket {
            start: start + Duration::milliseconds((i as f64 * width * 1000.0) as i64),
            ok: 0,
            failed: 0,
            slow: 0,
        })
        .collect();
    for s in samples {
        if s.at < start || s.at > now {
            continue;
        }
        let i = (((s.at - start).num_milliseconds() as f64 / 1000.0) / width) as usize;
        let b = &mut out[i.min(buckets - 1)];
        if s.ok {
            b.ok += 1;
            b.slow += u32::from(s.slow);
        } else {
            b.failed += 1;
        }
    }
    out
}

// ================================================================== report

/// One declared check, as the page shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckView {
    pub name: String,
    pub kind: CheckKind,
    pub target: String,
    pub every_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_after_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<Sample>,
    /// The last 24 hours in 48 half-hour slots.
    pub strip: Vec<StripBucket>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SloView {
    /// A ratio: `0.995`.
    pub target: f64,
    pub window_days: i64,
}

/// One environment on the page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentCard {
    pub name: String,
    pub scope: String,
    pub tier: Tier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotes_to: Option<String>,
    /// The owner may start the frozen promotion workflow. Rechecked on write.
    #[serde(default)]
    pub promotion_ready: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promotion_reason: Option<String>,
    /// `false` for an environment only deployments name -- an ad-hoc one
    /// nobody declared. It has no checks and no SLO.
    pub declared: bool,
    pub paused: bool,
    pub status: EnvStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_since: Option<DateTime<Utc>>,
    /// The newest deployment that succeeded: what is running there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<Deployment>,
    /// A deployment in progress, if one is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<Deployment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slo: Option<SloView>,
    pub uptime_24h: Option<f64>,
    pub uptime_7d: Option<f64>,
    /// Over the SLO window (28 days without one).
    pub uptime_window: Option<f64>,
    /// `None` without an SLO or without samples.
    pub error_budget: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<DateTime<Utc>>,
    pub checks: Vec<CheckView>,
    /// Over the SLO window, newest first.
    pub incidents: Vec<Incident>,
    pub dora: Dora,
}

/// `GET /api/environments`: the Operations tab and `factory env`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvironmentsReport {
    pub generated_at: DateTime<Utc>,
    /// In promotion order: ephemeral, staging, production; by name within.
    pub environments: Vec<EnvironmentCard>,
    /// Newest first.
    pub releases: Vec<Release>,
    /// Running ones first, then newest first; at most [`REPORT_DEPLOYMENTS`].
    pub deployments: Vec<Deployment>,
}

/// How many deployments the report carries.
pub const REPORT_DEPLOYMENTS: usize = 200;

/// Promotion order: ephemeral before staging before production, by name
/// within a tier.
pub fn promotion_order(cards: &mut [EnvironmentCard]) {
    cards.sort_by(|a, b| a.tier.cmp(&b.tier).then_with(|| a.name.cmp(&b.name)));
}

/// Build the report from the records, all pure. `declared` is `(scope,
/// declaration)`; `samples` every sample within the longest window;
/// `deployments` every deployment, any order; `added` releases recorded
/// without a deployment.
pub fn report(
    declared: &[(String, EnvironmentDecl)],
    samples: &[Sample],
    deployments: &[Deployment],
    added: &[(String, ReleaseFacts, DateTime<Utc>)],
    now: DateTime<Utc>,
) -> EnvironmentsReport {
    let mut deployments: Vec<Deployment> = deployments.to_vec();
    deployments.sort_by(|a, b| b.started_at.cmp(&a.started_at));

    // Declared environments, then any a deployment names that nobody
    // declared -- an ad-hoc review environment is still somewhere a release
    // ran.
    let mut names: Vec<(String, String, Option<&EnvironmentDecl>)> =
        declared.iter().map(|(scope, d)| (scope.clone(), d.name.clone(), Some(d))).collect();
    for d in &deployments {
        if !names.iter().any(|(_, name, _)| name == &d.environment) {
            names.push((d.scope.clone(), d.environment.clone(), None));
        }
    }

    let mut cards = Vec::new();
    for (scope, name, decl) in names {
        let env_samples: Vec<Sample> = samples.iter().filter(|s| s.environment == name).cloned().collect();
        let env_deploys: Vec<Deployment> = deployments.iter().filter(|d| d.environment == name).cloned().collect();
        let check_names: Vec<String> =
            decl.map(|d| d.checks.iter().map(CheckDecl::display_name).collect()).unwrap_or_default();
        let paused = decl.is_some_and(|d| d.paused);
        let (status, status_since) =
            if paused { (EnvStatus::Unknown, None) } else { status_timeline(&check_names, &env_samples) };
        let window_days = decl.and_then(|d| d.slo.as_ref()).map(Slo::window_days).unwrap_or(DEFAULT_WINDOW_DAYS);
        let uptime_window = availability(&env_samples, now - Duration::days(window_days));
        let slo = decl.and_then(|d| d.slo.as_ref()).map(|s| SloView { target: s.availability.ratio(), window_days });
        let error_budget = match (&slo, uptime_window) {
            (Some(slo), Some(a)) => Some(error_budget(a, slo.target)),
            _ => None,
        };
        let all_incidents = incidents(&name, &env_samples);
        let since = now - Duration::days(window_days);
        let mut window_incidents: Vec<Incident> = all_incidents
            .iter()
            .filter(|i| i.started_at >= since || i.ended_at.is_none_or(|e| e >= since))
            .cloned()
            .collect();
        window_incidents.reverse();
        let checks = decl
            .map(|d| {
                d.checks
                    .iter()
                    .map(|c| {
                        let name = c.display_name();
                        let mine: Vec<Sample> = env_samples.iter().filter(|s| s.check == name).cloned().collect();
                        CheckView {
                            last: mine.iter().max_by_key(|s| s.at).cloned(),
                            strip: strip(&mine, now, Duration::hours(24), 48),
                            target: c.target(d.url.as_deref()),
                            kind: c.kind,
                            every_seconds: c.every_seconds(),
                            slow_after_ms: c.slow_after_ms,
                            name,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        cards.push(EnvironmentCard {
            scope,
            tier: decl.map(|d| d.tier).unwrap_or_default(),
            url: decl.and_then(|d| d.url.clone()),
            promotes_to: decl.and_then(|d| d.promotes_to.clone()),
            promotion_ready: false,
            promotion_reason: None,
            declared: decl.is_some(),
            paused,
            status,
            status_since,
            current: env_deploys.iter().find(|d| d.status == DeployStatus::Succeeded).cloned(),
            running: env_deploys.iter().find(|d| d.status == DeployStatus::Running).cloned(),
            slo,
            uptime_24h: availability(&env_samples, now - Duration::hours(24)),
            uptime_7d: availability(&env_samples, now - Duration::days(7)),
            uptime_window,
            error_budget,
            last_check: env_samples.iter().map(|s| s.at).max(),
            checks,
            incidents: window_incidents,
            dora: dora(&env_deploys, &all_incidents, window_days, now),
            name,
        });
    }
    promotion_order(&mut cards);

    let releases = releases(&deployments, added, &cards);
    let mut listed: Vec<Deployment> = deployments.iter().filter(|d| d.status == DeployStatus::Running).cloned().collect();
    listed.extend(deployments.iter().filter(|d| d.status != DeployStatus::Running).cloned());
    listed.truncate(REPORT_DEPLOYMENTS);
    EnvironmentsReport { generated_at: now, environments: cards, releases, deployments: listed }
}

/// The catalogue: one row per `(scope, commit)`, from deployments (newest
/// first) and releases added by hand, with where each is current.
fn releases(
    deployments: &[Deployment],
    added: &[(String, ReleaseFacts, DateTime<Utc>)],
    cards: &[EnvironmentCard],
) -> Vec<Release> {
    fn touch(out: &mut Vec<Release>, scope: &str, facts: &ReleaseFacts, at: DateTime<Utc>) -> usize {
        if let Some(i) = out.iter().position(|r| r.scope == scope && r.facts.commit == facts.commit) {
            let r = &mut out[i];
            r.first_seen = r.first_seen.min(at);
            // Fill in what an earlier record left out; never overwrite.
            let f = &mut r.facts;
            f.describe = f.describe.take().or_else(|| facts.describe.clone());
            f.version = f.version.take().or_else(|| facts.version.clone());
            f.profile = f.profile.take().or_else(|| facts.profile.clone());
            f.source = f.source.take().or_else(|| facts.source.clone());
            f.committed_at = f.committed_at.or(facts.committed_at);
            f.dirty |= facts.dirty;
            return i;
        }
        out.push(Release {
            scope: scope.to_string(),
            facts: facts.clone(),
            first_seen: at,
            running_on: Vec::new(),
            deployments: 0,
            failed_deployments: 0,
        });
        out.len() - 1
    }
    let mut out: Vec<Release> = Vec::new();
    for d in deployments {
        let i = touch(&mut out, &d.scope, &d.release, d.started_at);
        out[i].deployments += 1;
        if matches!(d.status, DeployStatus::Failed | DeployStatus::RolledBack) {
            out[i].failed_deployments += 1;
        }
    }
    for (scope, facts, at) in added {
        touch(&mut out, scope, facts, *at);
    }
    for card in cards {
        if let Some(cur) = &card.current {
            if let Some(r) = out.iter_mut().find(|r| r.scope == cur.scope && r.facts.commit == cur.release.commit) {
                r.running_on.push(card.name.clone());
            }
        }
    }
    out.sort_by(|a, b| b.first_seen.cmp(&a.first_seen));
    out
}

// ================================================================ requests

/// `factory deploy start`: a deployment has begun.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployStart {
    pub environment: String,
    #[serde(default)]
    pub strict_verification: bool,
    /// The scope it belongs to. A declared environment's own scope is used
    /// and this must agree; an undeclared one takes this, else the caller's
    /// scope, else the root scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(flatten)]
    pub release: ReleaseFacts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// When it began, for recording one after the fact. Now unless said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
}

/// `factory deploy finish`: how it ended.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeployFinish {
    pub id: String,
    pub status: DeployStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Run the environment's checks before calling a success one. On unless
    /// said; a success that skipped it says so.
    #[serde(default = "yes")]
    pub verify: bool,
}

fn yes() -> bool {
    true
}

/// `factory release add`: a release that exists, deployed or not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseAdd {
    pub scope: String,
    #[serde(flatten)]
    pub release: ReleaseFacts,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(min: i64) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap() + Duration::minutes(min)
    }

    fn s(check: &str, min: i64, ok: bool) -> Sample {
        Sample { environment: "prod".into(), check: check.into(), at: t(min), ok, latency_ms: 1, slow: false, detail: None }
    }

    fn decls(yaml: &str) -> Vec<EnvironmentDecl> {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    fn deploy(min: i64, status: DeployStatus, committed: Option<i64>) -> Deployment {
        Deployment {
            id: format!("d{min}"),
            scope: "factory".into(),
            environment: "prod".into(),
            release: ReleaseFacts { commit: format!("c{min}"), committed_at: committed.map(t), ..Default::default() },
            actor: Actor { kind: ActorKind::Person, name: "owner".into(), run_id: None, task_id: None },
            via: None,
            manual: true,
            strict_verification: false,
            started_at: t(min),
            finished_at: (status != DeployStatus::Running).then(|| t(min + 2)),
            status,
            reason: None,
            previous_commit: None,
            verification: None,
        }
    }

    #[test]
    fn deploy_recipe_is_optional_bounded_and_round_trips() {
        assert!(decls("- name: production\n")[0].deploy.is_none());
        let envs = decls("- name: production\n  deploy: { agent: releaser, command: './deploy', prepare: './build', timeout: 20m }\n");
        validate([("factory", envs.as_slice())]).unwrap();
        assert_eq!(envs[0].deploy.as_ref().unwrap().timeout_seconds(), 1200);
        assert_eq!(serde_yaml_ng::from_str::<Vec<EnvironmentDecl>>(&serde_yaml_ng::to_string(&envs).unwrap()).unwrap(), envs);
        for recipe in ["{ agent: '', command: 'true' }", "{ agent: releaser, command: ' ' }", "{ agent: releaser, command: 'true', prepare: '' }", "{ agent: releaser, command: 'true', timeout: 61m }"] {
            let invalid = decls(&format!("- name: production\n  deploy: {recipe}\n"));
            assert!(validate([("factory", invalid.as_slice())]).is_err(), "{recipe}");
        }
        assert!(serde_json::from_value::<Promote>(serde_json::json!({"environment":"staging", "deployment":"selected", "commit":"moving"})).is_err());
        let legacy: DeployStart = serde_json::from_value(serde_json::json!({"environment":"staging", "commit":"abc"})).unwrap();
        assert!(!legacy.strict_verification);
    }

    #[test]
    fn spans_and_percentages_parse_and_refuse() {
        assert_eq!(parse_span("60s").unwrap(), 60);
        assert_eq!(parse_span("5m").unwrap(), 300);
        assert_eq!(parse_span("28d").unwrap(), 28 * 86_400);
        assert!(parse_span("0s").is_err());
        assert!(parse_span("10").is_err());
        assert!(parse_span("1w").is_err());
        assert!(parse_span("18446744073709551615d").is_err());
        let slo: Slo = serde_yaml_ng::from_str("{ availability: 99.5%, window: 7d }").unwrap();
        assert!((slo.availability.ratio() - 0.995).abs() < 1e-9);
        assert_eq!(slo.window_days(), 7);
        let bare: Slo = serde_yaml_ng::from_str("{ availability: 99 }").unwrap();
        assert!((bare.availability.ratio() - 0.99).abs() < 1e-9);
        assert_eq!(bare.window_days(), DEFAULT_WINDOW_DAYS);
        assert!(serde_yaml_ng::from_str::<Slo>("{ availability: 100% }").is_err());
    }

    #[test]
    fn the_issue_example_parses_validates_and_round_trips() {
        let envs = decls(
            "- name: production\n  tier: production\n  url: https://factory.example.ts.net:8790\n  checks:\n    \
             - { kind: http, path: /api/status, expect: 200, every: 60s, timeout: 5s }\n  \
             slo: { availability: 99.5%, window: 28d }\n\
             - name: staging\n  tier: staging\n  promotes_to: production\n  url: https://factory.example.ts.net:8791\n  \
             checks:\n    - { kind: http, path: /api/status, expect: 200, every: 60s }\n  slo: { availability: 99%, window: 28d }\n",
        );
        validate([("factory", envs.as_slice())]).unwrap();
        assert_eq!(envs[0].checks[0].http_url(envs[0].url.as_deref()).unwrap(), "https://factory.example.ts.net:8790/api/status");
        assert_eq!(envs[0].checks[0].display_name(), "/api/status");
        let back: Vec<EnvironmentDecl> = serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&envs).unwrap()).unwrap();
        assert_eq!(back, envs);
    }

    #[test]
    fn validation_names_what_is_wrong() {
        let err = |yaml: &str| validate([("a", decls(yaml).as_slice())]).unwrap_err().to_string();
        assert!(err("- name: Prod\n").contains("lowercase"));
        assert!(err("- name: s\n  promotes_to: p\n").contains("promotes_to"));
        assert!(err("- name: s\n  checks: [{ kind: http, path: /x }]\n").contains("url"));
        assert!(err("- name: s\n  checks: [{ kind: tcp, host: h }]\n").contains("host and port"));
        assert!(err("- name: s\n  checks: [{ kind: command }]\n").contains("command"));
        assert!(err("- name: s\n  checks: [{ kind: command, command: 'true', every: 1s, timeout: 1s }]\n").contains("every"));
        assert!(err("- name: s\n  checks: [{ kind: command, command: 'true', every: 10s, timeout: 20s }]\n").contains("timeout"));
        assert!(err("- name: s\n  checks: [{ kind: command, command: x }, { kind: command, command: x }]\n").contains("two checks"));
        assert!(err("- name: s\n  slo: { availability: 99%, window: 120d }\n").contains("90d"));
        assert!(serde_yaml_ng::from_str::<Vec<EnvironmentDecl>>("- name: s\n  typo: 1\n").is_err());

        let one = decls("- name: prod\n");
        let two = decls("- name: prod\n");
        let e = validate([("a", one.as_slice()), ("b", two.as_slice())]).unwrap_err().to_string();
        assert!(e.contains("unique across the instance"), "{e}");
        // promotes_to may cross scopes.
        let s = decls("- name: staging\n  promotes_to: prod\n");
        validate([("a", one.as_slice()), ("b", s.as_slice())]).unwrap();
    }

    #[test]
    fn slow_threshold_is_optional_positive_and_below_timeout() {
        for ms in [1, 9999] {
            let envs = decls(&format!("- name: prod\n  checks: [{{ kind: command, command: 'true', slow_after_ms: {ms} }}]"));
            validate([("factory", envs.as_slice())]).unwrap();
            assert_eq!(envs[0].checks[0].slow_after_ms, Some(ms));
            let back = decls(&serde_yaml_ng::to_string(&envs).unwrap());
            assert_eq!(back, envs);
        }
        for ms in [0, 10000, u64::MAX] {
            let envs = decls(&format!("- name: prod\n  checks: [{{ kind: command, command: 'true', slow_after_ms: {ms} }}]"));
            assert!(validate([("factory", envs.as_slice())]).unwrap_err().to_string().contains("slow_after_ms"));
        }
    }

    #[test]
    fn slow_samples_degrade_status_without_fabricating_outages() {
        let fast = s("api", 0, true);
        let slow = Sample { slow: true, latency_ms: 1001, ..s("api", 1, true) };
        let still_slow = Sample { at: t(2), ..slow.clone() };
        let recovered = s("api", 3, true);
        let samples = vec![fast.clone(), slow.clone(), still_slow];
        let checks = vec!["api".into()];
        assert_eq!(status_timeline(&checks, &samples), (EnvStatus::Degraded, Some(t(1))));
        assert_eq!(availability(&samples, t(0)), Some(1.0));
        assert!(incidents("prod", &samples).is_empty());
        assert_eq!(status_of_checks(&[Some(EnvStatus::Down), Some(slow.status())]), EnvStatus::Degraded);
        let buckets = strip(&samples, t(2), Duration::minutes(2), 1);
        assert_eq!((buckets[0].ok, buckets[0].failed, buckets[0].slow), (3, 0, 2));
        let mut reversed = vec![recovered, slow, fast];
        assert_eq!(status_timeline(&checks, &reversed), (EnvStatus::Up, Some(t(3))));
        reversed.push(Sample { ok: false, slow: true, ..s("api", 4, false) });
        assert_eq!(status_timeline(&checks, &reversed), (EnvStatus::Down, Some(t(4))), "failure takes precedence");
    }

    #[test]
    fn old_wire_samples_and_buckets_are_not_reclassified() {
        let old = serde_json::json!({"environment":"prod", "check":"api", "at":t(0), "ok":true, "latency_ms":9000});
        let sample: Sample = serde_json::from_value(old).unwrap();
        assert!(!sample.slow);
        assert_eq!(sample.status(), EnvStatus::Up);
        let bucket: StripBucket = serde_json::from_value(serde_json::json!({"start":t(0), "ok":1, "failed":0})).unwrap();
        assert_eq!(bucket.slow, 0);
    }

    #[test]
    fn status_is_up_degraded_down_or_unknown_and_says_since_when() {
        assert_eq!(status_of(&[]), EnvStatus::Unknown);
        assert_eq!(status_of(&[None, None]), EnvStatus::Unknown);
        assert_eq!(status_of(&[Some(true), None]), EnvStatus::Up);
        assert_eq!(status_of(&[Some(true), Some(false)]), EnvStatus::Degraded);
        assert_eq!(status_of(&[Some(false), Some(false)]), EnvStatus::Down);

        let checks = vec!["a".to_string(), "b".to_string()];
        let samples = vec![s("a", 0, true), s("b", 1, true), s("a", 2, false), s("a", 3, false), s("b", 4, false), s("gone", 5, true)];
        assert_eq!(status_timeline(&checks, &samples), (EnvStatus::Down, Some(t(4))));
        let samples = vec![s("a", 0, true), s("a", 1, true), s("a", 2, true)];
        assert_eq!(status_timeline(&checks, &samples), (EnvStatus::Up, Some(t(0))));
    }

    #[test]
    fn two_failures_in_a_row_open_an_incident_and_a_success_closes_it() {
        // One blip is not an incident.
        assert!(incidents("prod", &[s("a", 0, true), s("a", 1, false), s("a", 2, true)]).is_empty());
        let found = incidents("prod", &[s("a", 0, true), s("a", 1, false), s("a", 2, false), s("a", 3, true)]);
        assert_eq!(found, vec![Incident { environment: "prod".into(), started_at: t(1), ended_at: Some(t(3)), checks: vec!["a".into()] }]);
        // Still failing: open.
        let open = incidents("prod", &[s("a", 1, false), s("a", 2, false)]);
        assert_eq!(open[0].ended_at, None);
        // Two checks failing over overlapping stretches are one incident.
        let both = incidents(
            "prod",
            &[s("a", 1, false), s("a", 2, false), s("b", 2, false), s("b", 3, false), s("a", 4, true), s("b", 6, true)],
        );
        assert_eq!(both.len(), 1);
        assert_eq!((both[0].started_at, both[0].ended_at), (t(1), Some(t(6))));
        assert_eq!(both[0].checks, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn availability_and_error_budget_come_from_samples() {
        let samples: Vec<Sample> = (0..100).map(|i| s("a", i, i % 50 != 0)).collect();
        assert_eq!(availability(&samples, t(0)), Some(0.98));
        assert_eq!(availability(&samples, t(1000)), None, "nothing checked is not 100%");
        assert!((error_budget(0.995, 0.99) - 0.5).abs() < 1e-9);
        assert!((error_budget(1.0, 0.99) - 1.0).abs() < 1e-9);
        assert!(error_budget(0.98, 0.99) < 0.0, "overspent stays negative");
    }

    #[test]
    fn dora_reads_deployments_and_incidents() {
        let now = t(60 * 24 * 7);
        let deployments = vec![
            deploy(0, DeployStatus::Succeeded, Some(-60)),
            deploy(100, DeployStatus::Failed, Some(90)),
            deploy(200, DeployStatus::Succeeded, Some(170)),
            deploy(300, DeployStatus::Running, None),
        ];
        // An incident 10 minutes after the third deployment finished counts
        // against it.
        let incs = vec![Incident { environment: "prod".into(), started_at: t(212), ended_at: Some(t(222)), checks: vec![] }];
        let d = dora(&deployments, &incs, 28, now);
        assert!((d.deploy_frequency.unwrap() - 2.0 / 4.0).abs() < 1e-9);
        // Lead times: 62 min and 32 min; nearest-rank median is the lower.
        assert_eq!(d.lead_time_p50, Some(32.0 * 60.0));
        assert!((d.change_failure_rate.unwrap() - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(d.time_to_restore_p50, Some(600.0));
        assert_eq!(d.mttr, Some(600.0));
        assert_eq!(d.incidents, 1);

        let empty = dora(&[], &[], 28, now);
        assert_eq!((empty.deploy_frequency, empty.change_failure_rate, empty.lead_time_p50), (None, None, None));
    }

    #[test]
    fn an_incident_after_the_next_deployment_is_not_the_previous_ones_fault() {
        let deployments = vec![deploy(0, DeployStatus::Succeeded, None), deploy(10, DeployStatus::Succeeded, None)];
        // Starts after the second deployment began: only the second is blamed.
        let incs = vec![Incident { environment: "prod".into(), started_at: t(13), ended_at: Some(t(14)), checks: vec![] }];
        let d = dora(&deployments, &incs, 28, t(1000));
        assert_eq!(d.change_failure_rate, Some(0.5));
    }

    #[test]
    fn the_strip_counts_samples_into_slots() {
        let now = t(60);
        let strip = strip(&[s("a", 1, true), s("a", 2, false), s("a", 59, true), s("a", -10, true)], now, Duration::hours(1), 6);
        assert_eq!(strip.len(), 6);
        assert_eq!((strip[0].ok, strip[0].failed), (1, 1));
        assert_eq!((strip[5].ok, strip[5].failed), (1, 0));
        assert_eq!(strip[3].ok + strip[3].failed, 0);
    }

    #[test]
    fn the_report_puts_environments_in_promotion_order_with_their_current_release() {
        let envs = decls(
            "- name: prod\n  tier: production\n  checks: [{ kind: command, command: 'true' }]\n  slo: { availability: 99% }\n\
             - name: staging\n  tier: staging\n  promotes_to: prod\n",
        );
        let declared: Vec<(String, EnvironmentDecl)> = envs.into_iter().map(|e| ("factory".to_string(), e)).collect();
        let samples: Vec<Sample> = (0..10).map(|i| s("true", i, true)).collect();
        let mut review = deploy(5, DeployStatus::Succeeded, None);
        review.environment = "review13".into();
        let deployments = vec![deploy(0, DeployStatus::Succeeded, None), deploy(3, DeployStatus::Failed, None), review];
        let added = vec![("factory".to_string(), ReleaseFacts { commit: "c99".into(), ..Default::default() }, t(9))];
        let r = report(&declared, &samples, &deployments, &added, t(20));
        let names: Vec<&str> = r.environments.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["review13", "staging", "prod"]);
        let prod = &r.environments[2];
        assert_eq!(prod.status, EnvStatus::Up);
        assert_eq!(prod.current.as_ref().unwrap().release.commit, "c0", "a failed deployment is not what runs");
        assert_eq!(prod.uptime_24h, Some(1.0));
        assert_eq!(prod.error_budget, Some(1.0));
        assert!(!r.environments[0].declared);
        assert_eq!(r.environments[1].status, EnvStatus::Unknown, "no checks, no status");
        let c0 = r.releases.iter().find(|x| x.facts.commit == "c0").unwrap();
        assert_eq!(c0.running_on, vec!["prod".to_string()]);
        let c3 = r.releases.iter().find(|x| x.facts.commit == "c3").unwrap();
        assert_eq!(c3.failed_deployments, 1);
        assert!(r.releases.iter().any(|x| x.facts.commit == "c99" && x.deployments == 0), "an added release is listed undeployed");
    }
}
