//! Where quality requests are served (`#107`, the L6 Quality attributes
//! tab). Like `policies/mod.rs`, `goals/mod.rs` and `scenarios/mod.rs`: the
//! authored profiles under `<root>/.factory/quality/` are the source of
//! truth for *what* matters where (`factory_core::quality`, pure and tested
//! on its own), re-read on every request; this module resolves each scope's
//! chain off the live config snapshot, gathers the evidence and metric
//! values that already live elsewhere in the daemon, and hands them to
//! `quality::evaluate`.
//!
//! ## Reuse, not reimplementation
//!
//! A check measure is judged by `policy::evaluate` as a one-control
//! catalogue (`quality::applied_checks` builds those controls), so its
//! evidence is gathered by `policies::Engine::dataset_level_facts`/
//! `evidence_for_scope` -- the same two functions `policy_report` and
//! `scenarios_report` call, lazy in the same way: a scope none of whose
//! scenarios asks a `sandbox` question never pays for the agent roster, a
//! report with no `gate` check never opens the bench store. A metric
//! measure reads `Engine::metrics`, the one computation Goals' key results
//! and Scenarios' drivers read too. There is no third registry and no
//! second evidence path.
//!
//! ## `quality_changed`
//!
//! Goals and Policy publish their `*_changed` events on their own writes (a
//! check-in, an attestation). Quality has no write of its own -- profiles
//! are authored by hand, and remediation creates an ordinary task, which
//! already publishes `TaskCreated` -- and nothing in Factory watches files.
//! So this is the issue's other option, "on the next read": every report
//! fingerprints what it loaded (every profile and every scope's chain, not
//! just the asked subtree, so a scoped and an unscoped read agree) and
//! publishes `Event::QualityChanged` when that differs from the last read.
//! A reader that reloads on the event reads the same fingerprint again, so
//! the event never feeds itself.
//!
//! ## Attestations
//!
//! An `attestation` check measure reads `no_data` with
//! `quality::ATTESTATION_UNSUPPORTED` as its reason: the attestation store
//! keys a row by a `ControlRef` it parses back on every read, and a quality
//! scenario's synthetic ref (`quality/<attribute>/<scenario>`) does not
//! parse. So this module never loads attestations at all -- `evidence_for_scope`
//! is handed none.
//!
//! ## Enforces nothing
//!
//! Nothing here starts, stops or gates any work (design §8).
//! [`Engine::quality_remediate`] is the one write, an explicit action that
//! creates an ordinary task through `Engine::create`, the same door
//! `policy_remediate` and `scenario_promote` use.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use factory_core::adapter::agent::{QualityAttributeContext, QualityScenarioContext};
use factory_core::config::{Factory, Scope};
use factory_core::error::{FactoryError, Result};
use factory_core::event::Event;
use factory_core::metrics::{MetricId, MetricSeries, MetricValue};
use factory_core::policy::{self, Check};
use factory_core::protocol::{CharacteristicView, QualityRemediation, QualityReport, ScopeQuality};
use factory_core::quality::{self, Level, Measure, QualityCatalogue, QualityTree, ScenarioStatus, ScopeReport};
use factory_core::task::{NewTask, TaskFilter};

use crate::engine::Engine;
use factory_core::config::subtree_scopes;

/// The label a remediation task carries, and the key `ScopeQuality::open_tasks`
/// and [`Engine::quality_remediate`] look it up by. The scope is part of it
/// (unlike `policy=<framework>/<id>`) because one profile's scenario applies
/// in every scope that inherits it, and each scope's gap is its own.
pub(crate) fn remediation_label(scope: &str, attribute: &str, scenario: &str) -> String {
    format!("{scope}/{attribute}/{scenario}")
}

/// A fingerprint of everything a report's *shape* depends on that a person
/// authors: every loaded profile, every load finding, and every scope's
/// quality chain. Deliberately not the evidence or metric values -- those
/// move on their own events -- and not the asked scope, so a narrowed and a
/// whole-instance read of the same files agree. `DefaultHasher` is fine
/// here: the value never leaves this process or outlives a restart.
fn fingerprint(snapshot: &Factory, catalogue: &QualityCatalogue) -> u64 {
    let chains: Vec<(String, Vec<quality::QualityLayer>)> = snapshot
        .config
        .scopes
        .iter()
        .map(|s| (s.name.clone(), snapshot.config.quality_chain_for_scope(s)))
        .collect();
    let text = serde_json::to_string(&(&catalogue.profiles, &catalogue.findings, &chains)).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Every metric id `trees` measures by, once each, in first-seen order --
/// minus any `quality.*` metric (the recursion guard: `quality.<c>` is
/// itself computed from this module's report, and `quality::evaluate`
/// already reads such a measure as `no_data`) and any id the registry has
/// never heard of (`push_if_known`, so one typo in one profile cannot make
/// `Engine::metrics` refuse the whole call; the profile's own
/// `unknown_metric` finding already names it).
fn metric_ids<'a>(trees: impl IntoIterator<Item = &'a QualityTree>) -> Vec<MetricId> {
    let mut ids = Vec::new();
    for tree in trees {
        for attr in &tree.attributes {
            for s in &attr.scenarios {
                if let Some(Measure::Metric(m)) = &s.scenario.measure {
                    if !quality::is_quality_metric(&m.metric) && !ids.contains(&m.metric) {
                        crate::metrics::push_if_known(&mut ids, &m.metric);
                    }
                }
            }
        }
    }
    ids
}

/// The status word the guide shows beside a scenario, or `None` when there
/// is nothing to say: met, or a draft (whose missing measure already says
/// it).
fn guide_status(status: ScenarioStatus) -> Option<String> {
    match status {
        ScenarioStatus::Met | ScenarioStatus::Draft => None,
        ScenarioStatus::NotMet => Some("not met".to_string()),
        ScenarioStatus::Stale => Some("stale".to_string()),
        ScenarioStatus::NoData => Some("no data".to_string()),
    }
}

/// The guide's quality block, out of one scope's evaluated tree: its
/// H-importance attributes in authored order, each scenario's measure in
/// words and a status word when it is not met. See
/// `QualityScenarioContext` for why nothing here is a number.
pub(crate) fn guide_context(report: &ScopeReport) -> Vec<QualityAttributeContext> {
    report
        .attributes
        .iter()
        .filter(|a| a.importance == Level::High)
        .map(|a| QualityAttributeContext {
            attribute: a.id.clone(),
            scenarios: a
                .scenarios
                .iter()
                .map(|s| QualityScenarioContext {
                    scenario: s.scenario.scenario.id.clone(),
                    measure: s.scenario.scenario.measure.as_ref().map(quality::describe_measure),
                    status: guide_status(s.status),
                })
                .collect(),
        })
        .collect()
}

/// The instructions a remediation task carries: the scenario's six parts
/// as a sentence, what it is measured by, and why it is not met -- the
/// reasons exactly as the report shows them, so the task never says
/// something the evaluation did not. `policy::remediation_instructions`'
/// counterpart; there is no `remediation:` text to lead with, since a
/// quality scenario has none.
fn remediation_instructions(scope: &str, attribute: &str, result: &quality::ScenarioResult) -> String {
    let s = &result.scenario.scenario;
    let mut out = format!(
        "Quality scenario {attribute}/{} in scope {scope} is {}.\n",
        s.id,
        result.status.as_str().replace('_', " ")
    );
    let parts = [
        ("Source", &s.source),
        ("Stimulus", &s.stimulus),
        ("Artifact", &s.artifact),
        ("Environment", &s.environment),
        ("Response", &s.response),
    ];
    let written: Vec<String> = parts
        .iter()
        .filter_map(|(name, value)| value.as_ref().map(|v| format!("- {name}: {v}")))
        .collect();
    if !written.is_empty() {
        out.push_str("\nThe scenario:\n");
        out.push_str(&written.join("\n"));
        out.push('\n');
    }
    if let Some(measure) = &s.measure {
        out.push_str(&format!("\nResponse measure: {}\n", quality::describe_measure(measure)));
    }
    if !result.reasons.is_empty() {
        out.push_str("\nWhy it is not met:\n");
        for reason in &result.reasons {
            out.push_str(&format!("- {reason}\n"));
        }
    }
    out.push_str(
        "\nMake the response measure hold, then report done. A quality attribute gates nothing, \
         so say in your result what changed and how the measure reads now.",
    );
    out
}

/// Everything a quality evaluation reads before any evidence or metric:
/// the snapshot, the loaded profiles and their fingerprint, and each scope's
/// merged tree. Built by [`Engine::quality_inputs`]; the metric values its
/// trees need are then computed by whoever holds it -- `quality_report`
/// through `Engine::metrics`, or `Engine::metrics` itself in the same pass
/// as the rest of a request, so `production`/`policy_report` are read once.
pub(crate) struct QualityInputs {
    snapshot: Factory,
    catalogue: QualityCatalogue,
    fingerprint: u64,
    /// When the profiles were read -- orders two concurrent reports'
    /// fingerprints, so an older read can never overwrite a newer one.
    loaded_at: Instant,
    asked: Option<Scope>,
    pub(crate) trees: Vec<(Scope, QualityTree)>,
    findings: Vec<quality::Finding>,
}

impl QualityInputs {
    /// Every metric id the trees measure by -- see [`metric_ids`].
    pub(crate) fn metric_ids(&self) -> Vec<MetricId> {
        metric_ids(self.trees.iter().map(|(_, t)| t))
    }
}

/// Why a `no_data` scenario's measure can never produce data, whatever any
/// task does -- an `attestation` check (nothing can record one), or a metric
/// measure on a `quality.*`, unknown or unavailable metric. `None` when the
/// missing data is the kind work can supply: a task not yet run, a metric
/// with no finished runs behind it yet.
fn unfixable_by_a_task(measure: Option<&Measure>) -> Option<String> {
    match measure? {
        Measure::Check(Check::Attestation) => Some(quality::ATTESTATION_UNSUPPORTED.to_string()),
        Measure::Check(_) => None,
        Measure::Metric(m) if quality::is_quality_metric(&m.metric) => {
            Some(format!("{} is computed from quality scenarios themselves, so it cannot measure one", m.metric))
        }
        Measure::Metric(m) => match factory_core::metrics::resolve(&m.metric) {
            Ok(_) => None,
            Err(e) => Some(e.to_string()),
        },
    }
}

/// How long a scope's guide block is reused before it is judged again. Short
/// enough that a status word is never more than a minute behind; long
/// enough that a burst of dispatches into one scope (a bench run, a
/// workflow fanning out) judges it once rather than once per task.
const GUIDE_TTL: Duration = Duration::from_secs(60);

/// `Engine::quality_guide_cache`: canonical scope name to when its block was
/// judged, the profiles' fingerprint it was judged under, and the block.
pub(crate) type GuideCache = std::collections::HashMap<String, (Instant, u64, Vec<QualityAttributeContext>)>;

impl Engine {
    /// `.factory/quality/`, loaded off the async runtime.
    async fn load_quality(&self, snapshot: &Factory) -> Result<QualityCatalogue> {
        let dir = snapshot.quality_dir();
        tokio::task::spawn_blocking(move || quality::load(&dir))
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("quality profile walk: {e}")))
    }

    /// Load the profiles and fold every scope in `scope`'s subtree (the
    /// whole instance for `None`) that binds at least one -- or, with
    /// `only`, just that one scope, the guide's and remediation's case.
    pub(crate) async fn quality_inputs(&self, scope: Option<&str>, only: bool) -> Result<QualityInputs> {
        let loaded_at = Instant::now();
        let snapshot = self.factory_snapshot();
        let catalogue = self.load_quality(&snapshot).await?;
        let (asked, mut target_scopes) = subtree_scopes(&snapshot, scope)?;
        if only {
            target_scopes.retain(|s| Some(&s.name) == asked.as_ref().map(|a| &a.name));
        }
        let mut findings = catalogue.findings.clone();
        let mut trees = Vec::new();
        for t in target_scopes {
            let chain = snapshot.config.quality_chain_for_scope(&t);
            if chain.is_empty() {
                continue;
            }
            let (tree, chain_findings) = quality::applicable(&catalogue, &t.name, &chain);
            findings.extend(chain_findings);
            trees.push((t, tree));
        }
        Ok(QualityInputs {
            fingerprint: fingerprint(&snapshot, &catalogue),
            snapshot,
            catalogue,
            loaded_at,
            asked,
            trees,
            findings,
        })
    }

    /// Record what a *successful* report read, and publish
    /// `Event::QualityChanged` when it differs from what was recorded
    /// before. Compare, store and publish happen under one lock, so two
    /// reports finishing together cannot both publish, or lose the change;
    /// and a read that loaded its profiles before the recorded one never
    /// overwrites it, so a slow report over the old files cannot flip the
    /// fingerprint back. The first read after a start has nothing to
    /// compare against and publishes nothing.
    fn record_quality_fingerprint(&self, inputs: &QualityInputs) {
        let mut seen = self.quality_seen.lock().unwrap_or_else(|p| p.into_inner());
        match *seen {
            Some((at, _)) if at > inputs.loaded_at => return,
            Some((_, previous)) if previous != inputs.fingerprint => self.bus.publish(Event::QualityChanged {
                profiles: inputs.catalogue.profiles.keys().cloned().collect(),
            }),
            _ => {}
        }
        *seen = Some((inputs.loaded_at, inputs.fingerprint));
    }

    /// Judge every tree in `inputs` against `values`, already computed:
    /// the evidence every scope shares gathered once (`dataset_level_facts`)
    /// and each scope's own through `evidence_for_scope`. Returns each
    /// scope's report, in `inputs.trees` order, and every ambiguous-check
    /// finding the evidence turned up. Never computes a metric itself --
    /// which is what lets `Engine::metrics` call this without a cycle.
    pub(crate) async fn judge_quality(
        &self,
        inputs: &QualityInputs,
        values: &BTreeMap<MetricId, MetricValue>,
        now: DateTime<Utc>,
    ) -> Result<(Vec<ScopeReport>, Vec<quality::Finding>)> {
        let per_scope_applied: Vec<(&Scope, Vec<policy::Applied>)> =
            inputs.trees.iter().map(|(t, tree)| (t, quality::applied_checks(tree))).collect();

        // The knowledge vault is walked only when some scenario asks a
        // `knowledge` question -- through `load_catalogues_and_tags`, the
        // same walk a policy request makes, rather than a second one.
        let needs_tags = per_scope_applied
            .iter()
            .flat_map(|(_, applied)| applied)
            .flat_map(|a| &a.evidence)
            .any(|c| matches!(c, Check::Knowledge { .. }));
        let tags: BTreeSet<String> = if needs_tags {
            self.load_catalogues_and_tags().await?.2
        } else {
            BTreeSet::new()
        };
        let (gates, daemon_fact, credential_rows, backup_fact, budget_config) = self.dataset_level_facts(&per_scope_applied).await?;

        let mut reports = Vec::new();
        let mut findings = Vec::new();
        for ((t, applied), (_, tree)) in per_scope_applied.iter().zip(&inputs.trees) {
            let evidence = self
                .evidence_for_scope(
                    &inputs.snapshot,
                    t,
                    applied,
                    &tags,
                    &[],
                    &gates,
                    daemon_fact,
                    &credential_rows,
                    backup_fact.clone(),
                    budget_config.as_ref(),
                    now,
                )
                .await?;
            findings.extend(policy::evidence_findings(&evidence, &t.name).into_iter().map(|f| quality::Finding {
                kind: quality::FindingKind::AmbiguousCheckTarget,
                subject: f.subject,
                detail: f.detail,
            }));
            reports.push(quality::evaluate(tree, values, &evidence, now));
        }
        Ok((reports, findings))
    }

    /// Load, compute the metrics the trees read, and judge -- the whole
    /// evaluation for a caller that has no metric values of its own.
    async fn evaluate_quality(
        self: &Arc<Self>,
        inputs: &QualityInputs,
        now: DateTime<Utc>,
    ) -> Result<(Vec<ScopeReport>, Vec<quality::Finding>, Vec<MetricSeries>)> {
        let ids = inputs.metric_ids();
        let (values, series) = if ids.is_empty() {
            (BTreeMap::new(), Vec::new())
        } else {
            let computed = self.metrics(&ids, now).await?;
            let values: BTreeMap<MetricId, MetricValue> = computed.values.into_iter().map(|v| (v.id.clone(), v)).collect();
            (values, computed.series)
        };
        let (reports, findings) = self.judge_quality(inputs, &values, now).await?;
        Ok((reports, findings, series))
    }

    /// The L6 Quality attributes tab: `Request::Quality`. The one read
    /// that records the profiles' fingerprint (and so may publish
    /// `Event::QualityChanged`) -- a metrics or goals read that happens to
    /// compute `quality.*` does not.
    pub(crate) async fn quality_report(self: &Arc<Self>, scope: Option<&str>) -> Result<QualityReport> {
        let now = Utc::now();
        let inputs = self.quality_inputs(scope, false).await?;
        let (reports, evidence_findings, series) = self.evaluate_quality(&inputs, now).await?;

        let mut findings = inputs.findings.clone();
        findings.extend(evidence_findings);
        findings.sort_by(|a, b| a.subject.cmp(&b.subject).then(a.kind.cmp(&b.kind)).then(a.detail.cmp(&b.detail)));
        findings.dedup();

        // One unscoped read for every open remediation task, rather than one
        // per scope: `ScopedStores::list` with no scope fans out over every
        // store and merges, the same read `goal_tasks_done` makes.
        let open: BTreeMap<String, String> = if reports.is_empty() {
            BTreeMap::new()
        } else {
            self.store
                .list(&TaskFilter::default())
                .await?
                .into_iter()
                .filter(|t| !t.status.is_terminal())
                .filter_map(|t| t.labels.get("quality").cloned().map(|label| (label, t.id)))
                .collect()
        };

        let scopes = reports
            .into_iter()
            .map(|report| {
                let mut open_tasks = BTreeMap::new();
                for attr in &report.attributes {
                    for s in &attr.scenarios {
                        let id = &s.scenario.scenario.id;
                        if let Some(task) = open.get(&remediation_label(&report.scope, &attr.id, id)) {
                            open_tasks.insert(format!("{}/{id}", attr.id), task.clone());
                        }
                    }
                }
                ScopeQuality { report, open_tasks }
            })
            .collect();

        self.record_quality_fingerprint(&inputs);
        Ok(QualityReport {
            scope: inputs.asked.as_ref().map(|s| s.name.clone()),
            scopes,
            findings,
            catalogue: quality::CATALOGUE.iter().map(CharacteristicView::from).collect(),
            series,
        })
    }

    /// One scope's own evaluated tree, or `None` when its chain binds no
    /// profile. `h_only` narrows the tree to H-importance attributes before
    /// anything is gathered -- all the guide shows, so a dispatch never
    /// pays for evidence behind an M or L attribute.
    async fn quality_for_scope(self: &Arc<Self>, mut inputs: QualityInputs, h_only: bool) -> Result<Option<ScopeReport>> {
        if h_only {
            for (_, tree) in &mut inputs.trees {
                tree.attributes.retain(|a| a.importance == Level::High);
            }
            inputs.trees.retain(|(_, tree)| !tree.attributes.is_empty());
        }
        if inputs.trees.is_empty() {
            return Ok(None);
        }
        let (mut reports, _, _) = self.evaluate_quality(&inputs, Utc::now()).await?;
        Ok(reports.pop())
    }

    /// The guide's quality block for a task in `scope` -- resolved once per
    /// dispatch, the same as `policy_frameworks` and `goal_context`, and
    /// reused for [`GUIDE_TTL`] while the profiles' fingerprint holds.
    /// Judging is single-flight: the cache's lock is held across it, so a
    /// burst of dispatches waits for the first one's answer instead of each
    /// reading a year of runs. Empty, never an error, when nothing applies
    /// or something cannot be read -- a quality profile must never stop a
    /// task from dispatching -- and a failure is never cached.
    pub(crate) async fn quality_context(self: &Arc<Self>, scope: &str) -> Vec<QualityAttributeContext> {
        let mut cache = self.quality_guide_cache.lock().await;
        let inputs = match self.quality_inputs(Some(scope), true).await {
            Ok(inputs) => inputs,
            Err(error) => {
                tracing::warn!(scope, "could not load quality profiles for the agent guide: {error}");
                return Vec::new();
            }
        };
        let key = inputs.asked.as_ref().map(|s| s.name.clone()).unwrap_or_else(|| scope.to_string());
        if let Some((at, fp, block)) = cache.get(&key) {
            if *fp == inputs.fingerprint && at.elapsed() < GUIDE_TTL {
                return block.clone();
            }
        }
        let fp = inputs.fingerprint;
        match self.quality_for_scope(inputs, true).await {
            Ok(report) => {
                let block = report.as_ref().map(guide_context).unwrap_or_default();
                cache.insert(key, (Instant::now(), fp, block.clone()));
                block
            }
            Err(error) => {
                tracing::warn!(scope, "could not judge quality for the agent guide: {error}");
                Vec::new()
            }
        }
    }

    /// Close one scenario's gap: `Request::QualityRemediate`. Creates the task
    /// through `Engine::create`, the exact path `Request::TaskCreate`,
    /// `policy_remediate` and `scenario_promote` use, so every validation it
    /// does and its `Event::TaskCreated` apply here too.
    ///
    /// Refused when the scenario is `met` (no gap), a `draft` (no measure,
    /// so only a measure to write), or `no_data` for a reason no task can
    /// change (an `attestation` check, a `quality.*`, unknown or unavailable
    /// metric -- [`unfixable_by_a_task`]); each of the last two is an edit
    /// to a profile, and the refusal says so. When a non-terminal task
    /// labelled `quality=<scope>/<attribute>/<scenario>` is already open in
    /// `scope`, that task is answered with `created: false` and nothing is
    /// created (`#98`) -- unlike `policy_remediate`, which refuses and names
    /// it, because here the caller is asking for *the* task, and one exists.
    /// The check and the create are not atomic, the same as
    /// `policy_remediate`: two calls racing can both create.
    pub(crate) async fn quality_remediate(
        self: &Arc<Self>,
        scope: String,
        attribute: String,
        scenario: String,
        agent: Option<String>,
    ) -> Result<QualityRemediation> {
        let inputs = self.quality_inputs(Some(&scope), true).await?;
        let scope = inputs.asked.as_ref().map(|s| s.name.clone()).unwrap_or(scope);
        let report = self
            .quality_for_scope(inputs, false)
            .await?
            .ok_or_else(|| FactoryError::BadRequest(format!("no quality profile applies at {scope:?}")))?;
        let result = report
            .attributes
            .iter()
            .find(|a| a.id == attribute)
            .and_then(|a| a.scenarios.iter().find(|s| s.scenario.scenario.id == scenario))
            .ok_or_else(|| {
                FactoryError::BadRequest(format!("{attribute}/{scenario} is not a quality scenario that applies at {scope:?}"))
            })?;

        match result.status {
            ScenarioStatus::Met => {
                return Err(FactoryError::BadRequest(format!(
                    "{attribute}/{scenario} is already met at {scope:?}; nothing to remediate"
                )));
            }
            ScenarioStatus::Draft => {
                return Err(FactoryError::BadRequest(format!(
                    "{attribute}/{scenario} at {scope:?} is a draft with no response measure; give it a \
                     measure in its profile first -- there is no gap to close until there is"
                )));
            }
            ScenarioStatus::NoData => {
                if let Some(why) = unfixable_by_a_task(result.scenario.scenario.measure.as_ref()) {
                    return Err(FactoryError::BadRequest(format!(
                        "{attribute}/{scenario} at {scope:?} has no data, and no task can change that: {why}. \
                         The fix is an edit to its measure in the profile, not a task"
                    )));
                }
            }
            ScenarioStatus::NotMet | ScenarioStatus::Stale => {}
        }

        let label = remediation_label(&scope, &attribute, &scenario);
        let existing = self
            .store
            .list(&TaskFilter {
                scope: Some(scope.clone()),
                ..Default::default()
            })
            .await?
            .into_iter()
            .find(|t| !t.status.is_terminal() && t.labels.get("quality").map(String::as_str) == Some(label.as_str()));
        if let Some(task) = existing {
            return Ok(QualityRemediation { task, created: false });
        }

        let mut labels = BTreeMap::new();
        labels.insert("quality".to_string(), label);
        let new_task = NewTask {
            title: format!("Meet quality scenario {attribute}/{scenario}"),
            instructions: remediation_instructions(&scope, &attribute, result),
            scope: Some(scope),
            agent,
            labels,
            ..Default::default()
        };
        Ok(QualityRemediation {
            task: self.create(new_task).await?,
            created: true,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Engine-level report, metric, guide and remediation paths, on a
    //! temporary instance -- not `factory_core::quality` itself (covered on
    //! its own), but this module glued to a real scope tree, real profiles
    //! on disk and a real task store, the way a request sees them.

    use super::*;
    use factory_core::adapter::store::task_from_new;
    use factory_core::adapter::TaskStore;
    use factory_core::config::{Config, DaemonConfig, Instance, PolicyDeclaration};
    use factory_core::quality::FindingKind;
    use factory_core::run::{NewRun, RunPatch, RunStatus, Trigger};
    use factory_core::task::Task;
    use factory_plugins::{Registry, SqliteStore};
    use std::path::PathBuf;

    fn scope_at(id: &str, name: &str, path: &str, quality: &[&str]) -> Scope {
        let mut scope: Scope = serde_yaml_ng::from_str(&format!("id: {id}\nname: {name}\n")).unwrap();
        scope.path = PathBuf::from(path);
        scope.quality = quality.iter().map(|p| p.to_string()).collect();
        scope
    }

    fn write_profile(engine: &Engine, name: &str, body: &str) {
        let dir = engine.factory_snapshot().quality_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.yaml")), body).unwrap();
    }

    /// `company` (`.`) is root and binds `baseline`; `projects`
    /// (`projects`) adds `service`; `demo` (`projects/demo`) inherits both;
    /// `sibling` (`other`) sits outside `projects` -- the same shape the
    /// policy and goals daemon tests use, so a scope query is proven never
    /// to reach sideways.
    fn test_engine() -> Arc<Engine> {
        let root = std::env::temp_dir().join(format!("factory-quality-daemon-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let company = scope_at("company-id", "company", ".", &[]);
        let config = Config {
            version: 1,
            instance: Instance { id: "test".into(), name: "test".into() },
            daemon: DaemonConfig::default(),
            scope: Some(company.clone()),
            scopes: vec![
                company,
                scope_at("projects-id", "projects", "projects", &["service"]),
                scope_at("demo-id", "demo", "projects/demo", &[]),
                scope_at("sibling-id", "sibling", "other", &[]),
            ],
            roles: Default::default(),
            dashboard: None,
            policies: PolicyDeclaration::default(),
            quality: vec!["baseline".into()],
            infrastructure: Default::default(),
            secrets: Vec::new(),
            plugins_dir: None,
            renewals: Vec::new(),
            renewals_notify: None,
        };
        let registry = Registry::with_builtins();
        let store: Arc<dyn TaskStore> = Arc::new(SqliteStore::in_memory().unwrap());
        let engine = Arc::new(Engine::new(Factory { root, config }, registry, store, PathBuf::from("factory"), Vec::new()));
        write_profile(&engine, "baseline", BASELINE);
        write_profile(&engine, "service", SERVICE);
        engine
    }

    /// `security.confidentiality` (H) is met in any scope with no agents;
    /// `reliability` (M) is a draft.
    const BASELINE: &str = "attributes:\n\
        \x20 - id: security.confidentiality\n    importance: H\n    difficulty: M\n    scenarios:\n\
        \x20     - { id: sandboxed, measure: { check: sandbox } }\n\
        \x20 - id: reliability\n    importance: M\n    difficulty: L\n    scenarios:\n\
        \x20     - { id: someday }\n";

    /// Two H scenarios on a fitness-function task, and a metric measure no
    /// run has fed yet.
    const SERVICE: &str = "attributes:\n\
        \x20 - id: maintainability.modifiability\n    importance: H\n    difficulty: H\n    scenarios:\n\
        \x20     - { id: gate, measure: { check: task, task: quality-gate } }\n\
        \x20     - { id: right-first-time, measure: { metric: first_pass_yield, above: 0.8 } }\n";

    async fn task_in(engine: &Engine, scope: &str, title: &str) -> Task {
        let new = task_from_new(
            NewTask { title: title.to_string(), ..Default::default() },
            scope.to_string(),
            "assistant".to_string(),
            "shell".to_string(),
        );
        engine.store.create(&new).await.unwrap()
    }

    async fn finish(engine: &Engine, task: &Task, status: RunStatus) {
        let run = engine
            .store
            .create_run(&NewRun {
                task_id: task.id.clone(),
                trigger: Trigger::Manual,
                agent: "assistant".to_string(),
                adapter: "shell".to_string(),
                runtime: "shell".to_string(),
                token: "tok".to_string(),
                queued_at: None,
                scheduled_for: None,
            })
            .await
            .unwrap();
        engine
            .store
            .update_run(
                &run.id,
                &RunPatch { status: Some(status), ended_at: Some(Utc::now()), ..Default::default() },
                )
            .await
            .unwrap();
    }

    fn scope_names(report: &QualityReport) -> Vec<&str> {
        report.scopes.iter().map(|s| s.report.scope.as_str()).collect()
    }

    fn status(report: &QualityReport, scope: &str, attribute: &str, scenario: &str) -> ScenarioStatus {
        report
            .scopes
            .iter()
            .find(|s| s.report.scope == scope)
            .and_then(|s| s.report.attributes.iter().find(|a| a.id == attribute))
            .and_then(|a| a.scenarios.iter().find(|x| x.scenario.scenario.id == scenario))
            .map(|x| x.status)
            .unwrap_or_else(|| panic!("{scope}: {attribute}/{scenario} not in the report"))
    }

    #[tokio::test]
    async fn every_scope_gets_its_own_chain_and_a_scope_query_covers_only_its_subtree() {
        let engine = test_engine();
        let whole = engine.quality_report(None).await.unwrap();
        assert_eq!(scope_names(&whole), vec!["company", "projects", "demo", "sibling"]);
        assert_eq!(whole.catalogue.len(), 9);

        let attributes = |scope: &str| -> Vec<String> {
            whole.scopes.iter().find(|s| s.report.scope == scope).unwrap().report.attributes.iter().map(|a| a.id.clone()).collect()
        };
        assert_eq!(attributes("sibling"), vec!["security.confidentiality", "reliability"], "the root's baseline only");
        assert_eq!(
            attributes("demo"),
            vec!["security.confidentiality", "reliability", "maintainability.modifiability"],
            "demo inherits projects' service by path"
        );

        let narrowed = engine.quality_report(Some("projects")).await.unwrap();
        assert_eq!(scope_names(&narrowed), vec!["projects", "demo"]);
        assert_eq!(narrowed.scope.as_deref(), Some("projects"));
    }

    #[tokio::test]
    async fn a_check_measure_reads_the_same_evidence_policy_gathers() {
        let engine = test_engine();
        let report = engine.quality_report(Some("projects")).await.unwrap();
        assert_eq!(status(&report, "projects", "security.confidentiality", "sandboxed"), ScenarioStatus::Met);
        assert_eq!(
            status(&report, "projects", "maintainability.modifiability", "gate"),
            ScenarioStatus::NoData,
            "no task by that name yet"
        );
        assert_eq!(status(&report, "projects", "reliability", "someday"), ScenarioStatus::Draft);

        let gate = task_in(&engine, "projects", "quality-gate").await;
        finish(&engine, &gate, RunStatus::Failed).await;
        let report = engine.quality_report(Some("projects")).await.unwrap();
        assert_eq!(status(&report, "projects", "maintainability.modifiability", "gate"), ScenarioStatus::NotMet);
        assert_eq!(
            status(&report, "demo", "maintainability.modifiability", "gate"),
            ScenarioStatus::NoData,
            "a task check names a task in the evaluated scope, never a parent's"
        );

        finish(&engine, &gate, RunStatus::Done).await;
        let report = engine.quality_report(Some("projects")).await.unwrap();
        assert_eq!(status(&report, "projects", "maintainability.modifiability", "gate"), ScenarioStatus::Met);
        let series: Vec<&str> = report.series.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(series, vec!["first_pass_yield"], "the metric a scenario reads carries its sparkline");
    }

    #[tokio::test]
    async fn an_ambiguous_task_name_is_a_finding() {
        let engine = test_engine();
        task_in(&engine, "projects", "quality-gate").await;
        task_in(&engine, "projects", "quality-gate").await;
        let report = engine.quality_report(Some("projects")).await.unwrap();
        let ambiguous: Vec<&quality::Finding> =
            report.findings.iter().filter(|f| f.kind == FindingKind::AmbiguousCheckTarget).collect();
        assert_eq!(ambiguous.len(), 1, "{:#?}", report.findings);
        assert_eq!(ambiguous[0].subject, "projects");
    }

    #[tokio::test]
    async fn quality_metrics_are_the_share_of_declared_scenarios_met() {
        let engine = test_engine();
        let ids: Vec<MetricId> = ["quality.security", "quality.maintainability", "quality.safety", "quality.nonsense"]
            .into_iter()
            .map(|id| MetricId::new(id).unwrap())
            .collect();
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        let get = |id: &str| computed.values.iter().find(|v| v.id.as_str() == id).unwrap().clone();

        assert_eq!(get("quality.security").value, Some(1.0), "sandboxed is met in all four scopes");
        assert_eq!(get("quality.maintainability").value, Some(0.0), "four declared (two scopes x two), none met");
        assert_eq!(get("quality.safety").value, None, "nothing declared is not 'all met'");
        assert!(get("quality.safety").reason.unwrap().contains("no scope declares"));
        assert!(get("quality.nonsense").reason.unwrap().contains("not an ISO 25010 characteristic"));

        let defaults = engine.default_metric_ids().await;
        for id in ["quality.security", "quality.reliability", "quality.maintainability"] {
            assert!(defaults.iter().any(|d| d.as_str() == id), "{id} missing from {defaults:?}");
        }
    }

    #[tokio::test]
    async fn quality_changed_is_published_when_a_profile_moves_and_never_on_the_first_read() {
        let engine = test_engine();
        let mut bus = engine.bus.subscribe();
        let changed = |bus: &mut tokio::sync::broadcast::Receiver<Event>| {
            std::iter::from_fn(|| bus.try_recv().ok()).any(|e| matches!(e, Event::QualityChanged { .. }))
        };

        engine.quality_report(None).await.unwrap();
        assert!(!changed(&mut bus), "the first read has nothing to compare against");
        engine.quality_report(Some("demo")).await.unwrap();
        assert!(!changed(&mut bus), "a narrowed read of the same files is the same fingerprint");

        write_profile(&engine, "service", &SERVICE.replace("importance: H", "importance: M"));
        engine.quality_report(Some("demo")).await.unwrap();
        assert!(changed(&mut bus), "a profile edit is noticed on the next read");
        engine.quality_report(None).await.unwrap();
        assert!(!changed(&mut bus), "and only once");
    }

    #[tokio::test]
    async fn the_guide_names_h_attributes_with_a_status_word_only_when_not_met_and_is_stable() {
        let engine = test_engine();
        let context = engine.quality_context("demo").await;
        assert_eq!(
            context,
            vec![
                QualityAttributeContext {
                    attribute: "security.confidentiality".into(),
                    scenarios: vec![QualityScenarioContext {
                        scenario: "sandboxed".into(),
                        measure: Some("sandbox".into()),
                        status: None,
                    }],
                },
                QualityAttributeContext {
                    attribute: "maintainability.modifiability".into(),
                    scenarios: vec![
                        QualityScenarioContext {
                            scenario: "gate".into(),
                            measure: Some(quality::describe_measure(&Measure::Check(Check::Task {
                                task: "quality-gate".into(),
                                max_age: None,
                            }))),
                            status: Some("no data".into()),
                        },
                        QualityScenarioContext {
                            scenario: "right-first-time".into(),
                            measure: Some("first_pass_yield >= 0.8".into()),
                            status: Some("no data".into()),
                        },
                    ],
                },
            ],
            "reliability is M, so it is left out"
        );
        assert_eq!(engine.quality_context("demo").await, context, "nothing changed, so nothing differs");
        assert!(engine.quality_context("no-such-scope").await.is_empty(), "never an error at dispatch");
    }

    #[tokio::test]
    async fn remediate_creates_one_labelled_task_then_answers_the_open_one() {
        let engine = test_engine();
        let first = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "gate".into(), None)
            .await
            .unwrap();
        assert!(first.created);
        assert_eq!(first.task.scope, "demo");
        assert_eq!(
            first.task.labels.get("quality").map(String::as_str),
            Some("demo/maintainability.modifiability/gate")
        );
        assert!(first.task.instructions.contains("check task quality-gate") || first.task.instructions.contains("quality-gate"));

        let again = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "gate".into(), None)
            .await
            .unwrap();
        assert!(!again.created, "#98: the open task, not a second one");
        assert_eq!(again.task.id, first.task.id);

        let report = engine.quality_report(Some("demo")).await.unwrap();
        assert_eq!(
            report.scopes[0].open_tasks.get("maintainability.modifiability/gate"),
            Some(&first.task.id),
            "the report says which task is already open"
        );

        let projects = engine
            .quality_remediate("projects".into(), "maintainability.modifiability".into(), "gate".into(), None)
            .await
            .unwrap();
        assert!(projects.created, "another scope's gap is its own");
    }

    /// `#122`: a remediation task whose run failed is blocked, not closed,
    /// so it is still the open one -- the failure gets looked at instead of
    /// a second copy piling up behind it (both the report's `open_tasks`
    /// read and `quality_remediate`'s own lookup).
    #[tokio::test]
    async fn remediate_answers_a_failed_remediation_task_rather_than_spawning_a_duplicate() {
        let engine = test_engine();
        let first = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "gate".into(), None)
            .await
            .unwrap();
        engine.fail_task_for_test(&first.task.id, factory_core::run::FailKind::AgentFailed).await;

        let again = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "gate".into(), None)
            .await
            .unwrap();
        assert!(!again.created, "the failed task is still open");
        assert_eq!(again.task.id, first.task.id);
        let report = engine.quality_report(Some("demo")).await.unwrap();
        assert_eq!(report.scopes[0].open_tasks.get("maintainability.modifiability/gate"), Some(&first.task.id));

        // Closed on purpose, it no longer stands in the way.
        engine
            .handle_request(factory_core::protocol::Request::TaskClose {
                id: first.task.id.clone(),
                reason: factory_core::task::CloseReason::NotPlanned,
                duplicate_of: None,
                note: None,
            })
            .await;
        let fresh = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "gate".into(), None)
            .await
            .unwrap();
        assert!(fresh.created);
    }

    #[tokio::test]
    async fn remediate_refuses_a_met_scenario_a_draft_and_one_that_does_not_apply() {
        let engine = test_engine();
        let refuse = |attribute: &str, scenario: &str, scope: &str| {
            let engine = engine.clone();
            let (attribute, scenario, scope) = (attribute.to_string(), scenario.to_string(), scope.to_string());
            async move { engine.quality_remediate(scope, attribute, scenario, None).await.unwrap_err().to_string() }
        };
        assert!(refuse("security.confidentiality", "sandboxed", "demo").await.contains("already met"));
        assert!(refuse("reliability", "someday", "demo").await.contains("draft"));
        assert!(refuse("maintainability.modifiability", "gate", "sibling").await.contains("not a quality scenario"));
        assert!(engine.store.list(&TaskFilter::default()).await.unwrap().is_empty(), "nothing was created");
    }

    #[tokio::test]
    async fn remediate_refuses_no_data_that_only_a_profile_edit_can_fix() {
        let engine = test_engine();
        write_profile(
            &engine,
            "service",
            "attributes:\n  - id: maintainability.modifiability\n    importance: H\n    difficulty: H\n    scenarios:\n\
             \x20     - { id: signed-off, measure: { check: attestation } }\n\
             \x20     - { id: cheap, measure: { metric: unit_cost, below: 1 } }\n\
             \x20     - { id: gate, measure: { check: task, task: quality-gate } }\n",
        );
        let err = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "signed-off".into(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no task can change that") && err.contains("profile"), "{err}");
        // `unit_cost` was the unavailable-metric case here until #117 made it
        // computable: with no finished runs it is ordinary no_data now, the
        // kind a task can change, so it is remediated like `scrap_rate`.
        let cheap = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "cheap".into(), None)
            .await
            .unwrap();
        assert!(cheap.created);
        let ok = engine
            .quality_remediate("demo".into(), "maintainability.modifiability".into(), "gate".into(), None)
            .await
            .unwrap();
        assert!(ok.created, "no task yet is exactly the no_data a fitness-function task fixes");
    }

    #[tokio::test]
    async fn an_older_read_never_overwrites_a_newer_fingerprint_or_publishes() {
        let engine = test_engine();
        let mut bus = engine.bus.subscribe();
        let older = engine.quality_inputs(None, false).await.unwrap();
        write_profile(&engine, "service", &SERVICE.replace("importance: H", "importance: M"));
        let newer = engine.quality_inputs(None, false).await.unwrap();
        assert_ne!(older.fingerprint, newer.fingerprint);

        engine.record_quality_fingerprint(&newer);
        engine.record_quality_fingerprint(&older);
        assert!(bus.try_recv().is_err(), "first record has nothing to compare; the older one is ignored");
        let seen = engine.quality_seen.lock().unwrap().unwrap();
        assert_eq!(seen.1, newer.fingerprint, "the newer read stays recorded");
    }

    #[tokio::test]
    async fn a_metrics_read_of_quality_never_publishes_quality_changed() {
        let engine = test_engine();
        let mut bus = engine.bus.subscribe();
        engine.quality_report(None).await.unwrap();
        write_profile(&engine, "service", &SERVICE.replace("importance: H", "importance: M"));
        let ids = vec![MetricId::new("quality.security").unwrap(), MetricId::new("scrap_rate").unwrap()];
        let computed = engine.metrics(&ids, Utc::now()).await.unwrap();
        assert_eq!(computed.values.len(), 2, "only what was asked comes back, whatever quality read on the side");
        assert_eq!(computed.series.len(), 1);
        assert!(
            !std::iter::from_fn(|| bus.try_recv().ok()).any(|e| matches!(e, Event::QualityChanged { .. })),
            "only a quality report read records and publishes"
        );
    }

    #[tokio::test]
    async fn the_guide_block_is_reused_within_its_ttl_until_a_profile_changes() {
        let engine = test_engine();
        let first = engine.quality_context("projects").await;
        let gate = task_in(&engine, "projects", "quality-gate").await;
        finish(&engine, &gate, RunStatus::Done).await;
        assert_eq!(engine.quality_context("projects").await, first, "cached: the evidence moved, the block did not yet");

        write_profile(&engine, "service", &SERVICE.replace("right-first-time", "first-time"));
        let fresh = engine.quality_context("projects").await;
        assert_ne!(fresh, first, "a profile edit changes the fingerprint, so the block is judged again");
        let gate_line = fresh[1].scenarios.iter().find(|s| s.scenario == "gate").unwrap();
        assert_eq!(gate_line.status, None, "and now reads the finished task: met");
    }
}
