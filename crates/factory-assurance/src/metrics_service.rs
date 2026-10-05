//! L5's live metrics service: registry resolution, selective lower fact reads,
//! compliance/Quality evaluation and numeric projection. The outside router
//! supplies raw authored L6 subjects/limits, never reports or computed values.
use crate::{
    checks::{self, EvaluationSubject},
    evaluation_rollup::{self, StatusCounts},
    evidence::{self, BudgetIntent},
    metrics::{
        self, MetricDef, MetricDefView, MetricError, MetricId, MetricSeries, MetricValue,
        MetricsWindow,
    },
    quality::{self, ScenarioStatus},
    quality_inputs, reported,
};
use chrono::{DateTime, NaiveDate, Utc};
use factory_kernel::{
    Attestation, AttestedRun, BackupFact, CostReport, EnvironmentMetricFact, FactoryError, Facts,
    KnowledgeTags, ProcessMetricFact, ProductionBin, ProductionBucket, ProductionFact, Provide,
    Result, ScopeNode, ScopeTree, L5,
};
use factory_process::{
    measurements::{AttestedQuery, ProcessMetricsQuery, ProductionQuery},
    usage::SpendQuery,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Debug)]
pub struct Metrics {
    pub values: Vec<MetricValue>,
    pub series: Vec<MetricSeries>,
    pub registry: Vec<MetricDefView>,
    /// `#278`: every `scope.metrics` declaration this call's `Plan::prepare`
    /// found wrong -- a bad slug, a duplicate source id, a path that
    /// escapes the instance, a `secrets` component, or a symlink. Never a
    /// reason this call itself failed; always set, even when no asked id
    /// is a `reported.*` one, so a broken declaration surfaces without
    /// anyone having to ask for it by name.
    pub findings: Vec<reported::Finding>,
}

pub trait Ports: evidence::Ports {
    type Production: Provide<
        ProductionFact,
        Query = ProductionQuery,
        Value = ProductionFact,
        Error = FactoryError,
    >;
    type Process: Provide<
        ProcessMetricFact,
        Query = ProcessMetricsQuery,
        Value = BTreeMap<String, ProcessMetricFact>,
        Error = FactoryError,
    >;
    type Environments: Provide<
        EnvironmentMetricFact,
        Query = Option<String>,
        Value = BTreeMap<String, EnvironmentMetricFact>,
        Error = FactoryError,
    >;
    fn production(&self) -> &Self::Production;
    fn process(&self) -> &Self::Process;
    fn environments(&self) -> &Self::Environments;
}

/// Authored L6 counting instructions and receipts. There is no metric,
/// check verdict, rollup or task/workflow page decoration in this input.
pub struct PolicyScope {
    pub scope: ScopeNode,
    pub subjects: Vec<EvaluationSubject<bool>>,
    pub budget: Option<BudgetIntent>,
}
pub struct PolicyInputs {
    pub scopes: Vec<PolicyScope>,
    pub attestations: Vec<Attestation>,
}
pub type QualityBudgets = std::result::Result<BTreeMap<String, BudgetIntent>, String>;

pub struct Plan {
    resolved: Vec<(MetricId, std::result::Result<MetricDef, &'static str>)>,
    computing: Vec<(MetricId, std::result::Result<MetricDef, &'static str>)>,
    quality: Option<std::result::Result<quality_inputs::Inputs, String>>,
    /// `#278`: every scope's `scope.metrics` declaration, already validated
    /// -- computed fresh in [`Plan::prepare`] from the plain input handed
    /// in, never cached across calls.
    catalogue: reported::Catalogue,
    scope: Option<String>,
    members: BTreeSet<String>,
    /// `#278` phase 2: the plain inputs this plan was built from, kept so
    /// `gather_policy`'s own nested `Check::Metric` read
    /// (`Service::metric_values`) can share this exact computation rather
    /// than opening a second, different path to the same values.
    root: PathBuf,
    reported: reported::Configuration,
}
impl Plan {
    /// Scope and ids resolve before any IO. Quality's own authored read is
    /// used to expand its metric dependencies into this single lazy pass.
    /// `reported` is the instance's plain `scope.metrics` declarations --
    /// see `reported.rs`'s own doc comment -- validated here, unconditionally,
    /// since that costs no more than a handful of path checks; the file
    /// each source actually names is read later, only if some id in
    /// `computing` needs it (`gather_measurements`).
    pub async fn prepare(
        ids: &[MetricId],
        root: PathBuf,
        scopes: &ScopeTree,
        quality: &quality_inputs::Configuration,
        reported: &reported::Configuration,
        scope: Option<&str>,
    ) -> Result<Self> {
        let plan_root = root.clone();
        let plan_reported = reported.clone();
        let catalogue = reported::validate(&root, reported);
        let (asked, targets) = scopes.subtree_scopes(scope)?;
        let canonical_scope = asked.map(|s| s.name.clone());
        let members = targets.iter().map(|s| s.name.clone()).collect();
        let mut wanted = Vec::new();
        for id in ids {
            if !wanted.contains(id) {
                wanted.push(id.clone());
            }
        }
        let mut resolved = Vec::new();
        for id in wanted {
            match metrics::resolve(&id) {
                Ok(def) => resolved.push((id, Ok(def))),
                Err(MetricError::Unavailable { reason, .. }) => resolved.push((id, Err(reason))),
                Err(MetricError::Unknown(_)) => {
                    return Err(FactoryError::BadRequest(format!(
                        "{id} is not a known metric"
                    )))
                }
            }
        }
        let needs_quality = resolved
            .iter()
            .any(|(id, result)| result.is_ok() && is_quality_metric(id.as_str()));
        let quality = if needs_quality {
            Some(
                quality
                    .read(root, canonical_scope.as_deref(), false)
                    .await
                    .map_err(|e| e.to_string()),
            )
        } else {
            None
        };
        let mut computing: Vec<_> = resolved
            .iter()
            .filter(|(id, _)| !is_quality_metric(id.as_str()))
            .cloned()
            .collect();
        if let Some(Ok(inputs)) = &quality {
            for id in inputs.metric_ids() {
                if computing.iter().any(|(have, _)| have == &id) {
                    continue;
                }
                match metrics::resolve(&id) {
                    Ok(def) => computing.push((id, Ok(def))),
                    Err(MetricError::Unavailable { reason, .. }) => {
                        computing.push((id, Err(reason)))
                    }
                    Err(MetricError::Unknown(_)) => {}
                }
            }
        }
        Ok(Self {
            resolved,
            computing,
            quality,
            catalogue,
            scope: canonical_scope,
            members,
            root: plan_root,
            reported: plan_reported,
        })
    }
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }
    pub fn needs_policy(&self) -> bool {
        self.computing
            .iter()
            .any(|(id, result)| result.is_ok() && is_policy_metric(id.as_str()))
    }
    /// Whether `scope_name` (the scope that declared a `reported.*`
    /// source) is covered by this plan's own selected subtree -- always
    /// true for an unscoped (instance-wide) request, the same rule
    /// `compliance.<framework>`/`quality.<characteristic>` already follow.
    fn includes_scope(&self, scope_name: &str) -> bool {
        self.scope.is_none() || self.members.contains(scope_name)
    }
    pub fn quality_budget_ids(&self) -> Vec<&str> {
        match &self.quality {
            Some(Ok(inputs)) => inputs
                .trees
                .iter()
                .filter(|(_, t)| evidence::needs_budget_facts(&quality::check_subjects(t)))
                .map(|(s, _)| s.id.as_str())
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Opaque in-flight owner state, not a fact or command response. Splitting
/// gather/finish lets the legacy outside request perform its page-error
/// compatibility preflight without a callback entering the service.
pub struct Gathered {
    spend: Option<CostReport>,
    production: Option<ProductionFact>,
    process: BTreeMap<String, ProcessMetricFact>,
    policy: Option<PolicyMeasurements>,
    /// `#278`: every needed source's file, read at most once per call and
    /// shared across every `reported.*` id that names it -- keyed by
    /// source id, empty when no asked id needs one.
    reported: BTreeMap<String, std::result::Result<reported::Document, String>>,
}
struct PolicyMeasurements {
    rollup: Vec<PolicyFramework>,
}
struct PolicyFramework {
    framework: String,
    counts: StatusCounts,
}

pub struct Service<'a, P> {
    // `pub(crate)`, not private: `#278`'s `check_evaluation::Provider` holds
    // one of these as its sole lower-evidence seam and reaches through it
    // for `shared`/`for_scope`, rather than this crate's two L5 services
    // each carrying their own separate `evidence::Service`.
    pub(crate) evidence: evidence::Service<'a, P>,
}
impl<'a, P: Ports> Service<'a, P> {
    pub fn new(evidence: evidence::Service<'a, P>) -> Self {
        Self { evidence }
    }
    pub async fn gather(
        &self,
        plan: &Plan,
        policy: Option<&PolicyInputs>,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
    ) -> Result<Gathered> {
        let mut gathered = self.gather_measurements(plan, now, window).await?;
        if plan.needs_policy() {
            self.gather_policy(
                &mut gathered,
                policy.ok_or_else(|| {
                    FactoryError::BadRequest("policy metric declarations were not resolved".into())
                })?,
                plan,
            )
            .await?;
        }
        Ok(gathered)
    }

    /// The request may resolve L6's authored declarations after these lower
    /// reads, preserving the legacy ordering without a router callback.
    pub async fn gather_measurements(
        &self,
        plan: &Plan,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
    ) -> Result<Gathered> {
        let computing = &plan.computing;
        let canonical_scope = plan.scope();
        let facts = Facts::<L5>::new();
        let needs_spend = computing.iter().any(|(id, result)| {
            result.is_ok() && matches!(id.as_str(), "unit_cost" | "tokens_per_run")
        });
        let spend = if needs_spend {
            Some(
                facts
                    .get::<CostReport, _>(
                        self.evidence.ports.spend(),
                        &SpendQuery {
                            basis: factory_kernel::SpendBasis::Finished,
                            scope: canonical_scope.map(str::to_string),
                            from: Some(
                                now - chrono::Duration::days(
                                    window
                                        .map(MetricsWindow::days)
                                        .unwrap_or(OPERATIONS_WINDOW_DAYS),
                                ),
                            ),
                            to: Some(now),
                            group_by: factory_kernel::CostGroupBy::Scope,
                        },
                    )
                    .await?,
            )
        } else {
            None
        };
        let production = if computing
            .iter()
            .any(|(id, result)| result.is_ok() && is_production_metric(id.as_str()))
        {
            Some(
                facts
                    .get::<ProductionFact, _>(
                        self.evidence.ports.production(),
                        &ProductionQuery {
                            scope: canonical_scope.map(str::to_string),
                            now,
                            minutes: window.map(|w| (w.days() * 24 * 60) as u32).or(Some(5)),
                            bin: ProductionBin::Day,
                            subtree: true,
                        },
                    )
                    .await?,
            )
        } else {
            None
        };
        let process_names: BTreeSet<String> = computing
            .iter()
            .filter(|(id, result)| {
                result.is_ok()
                    && (is_operations_metric(id.as_str())
                        || is_usage_metric(id.as_str())
                        || is_hours_metric(id.as_str())
                        || is_intake_metric(id.as_str())
                        || id.as_str().starts_with("goal_tasks_done."))
            })
            .map(|(id, _)| id.to_string())
            .collect();
        let process = if process_names.is_empty() {
            BTreeMap::new()
        } else {
            facts
                .get::<ProcessMetricFact, _>(
                    self.evidence.ports.process(),
                    &ProcessMetricsQuery {
                        scope: canonical_scope.map(str::to_string),
                        now,
                        window_days: window.map(MetricsWindow::days),
                        names: process_names,
                    },
                )
                .await?
        };
        // `#278`: the distinct source ids some asked `reported.*` id
        // actually needs -- a source declared but never asked for, or
        // asked for but outside the selected subtree, is never read. Each
        // needed source's file is read exactly once here, however many of
        // its own ids `computing` names.
        let needed_sources: BTreeSet<String> = computing
            .iter()
            .filter(|(id, result)| result.is_ok() && is_reported_metric(id.as_str()))
            .filter_map(|(id, _)| reported_segments(id.as_str()))
            .map(|(source_id, _)| source_id.to_string())
            .filter(|source_id| {
                plan.catalogue
                    .sources
                    .get(source_id)
                    .is_some_and(|source| plan.includes_scope(&source.scope.name))
            })
            .collect();
        let reported = if needed_sources.is_empty() {
            BTreeMap::new()
        } else {
            let paths: Vec<(String, std::path::PathBuf)> = needed_sources
                .into_iter()
                .map(|source_id| {
                    let path = plan.catalogue.sources[&source_id].path.clone();
                    (source_id, path)
                })
                .collect();
            tokio::task::spawn_blocking(move || {
                paths
                    .into_iter()
                    .map(|(source_id, path)| (source_id, reported::read_source(&path)))
                    .collect::<BTreeMap<_, _>>()
            })
            .await
            .map_err(|e| FactoryError::Other(anyhow::anyhow!("reported metrics read: {e}")))?
        };

        Ok(Gathered {
            spend,
            production,
            process,
            policy: None,
            reported,
        })
    }
    pub async fn gather_policy(
        &self,
        gathered: &mut Gathered,
        inputs: &PolicyInputs,
        plan: &Plan,
    ) -> Result<()> {
        gathered.policy = Some(self.policy_measurements(inputs, plan).await?);
        Ok(())
    }

    /// `#278`: every metric id a `Check::Metric` among `per_scope`'s
    /// subjects asks for, resolved and computed once, unscoped, and shared
    /// across every scope and control in this evaluation -- the lazy,
    /// shared batching the issue asks for, and the same simplification
    /// `quality::evaluate`'s own shared `values` map already makes for a
    /// Quality `MetricMeasure`. Circular (`checks::is_circular_metric`) and
    /// unresolvable ids are dropped before any plan exists --
    /// `checks::evaluate` then reads them `open` with its own reason, and
    /// `checks::check_vocabulary` already caught the same mistake as a
    /// finding when the catalogue loaded. Empty does no work at all: no
    /// plan, no gather. `root`/`reported` are the caller's own plain
    /// config projection, the same raw input `Plan::prepare` always takes;
    /// validating it again here costs no more than the handful of path
    /// checks `Plan::prepare` already does unconditionally on every call.
    ///
    /// This is unscoped by design -- it does not itself enforce a
    /// `reported.*` id's scope coverage (the same simplification Quality's
    /// own shared `values` map already makes). The returned
    /// [`reported::Catalogue`] is how a caller applies that coverage rule
    /// afterward, per evaluated scope: see
    /// [`metrics_for_evaluated_scope`], which both `check_evaluation`'s
    /// provider and [`Service::policy_measurements`] call on this same
    /// computation -- never a second read or a second path to the value.
    pub async fn metric_values<S: checks::CheckSource>(
        &self,
        per_scope: &[(&str, &[S])],
        root: PathBuf,
        reported: &reported::Configuration,
        now: DateTime<Utc>,
    ) -> Result<(BTreeMap<MetricId, MetricValue>, reported::Catalogue)> {
        let mut ids = Vec::new();
        for (_, subjects) in per_scope {
            for id in evidence::metric_check_ids(subjects) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        if ids.is_empty() {
            return Ok((BTreeMap::new(), reported::Catalogue::default()));
        }
        let plan = Plan::prepare(
            &ids,
            root,
            &self.evidence.scopes,
            &quality_inputs::Configuration::default(),
            reported,
            None,
        )
        .await?;
        let gathered = self.gather_measurements(&plan, now, None).await?;
        let computed = self
            .finish(&plan, gathered, &Ok(BTreeMap::new()), now, None)
            .await?;
        let values = computed
            .values
            .into_iter()
            .map(|v| (v.id.clone(), v))
            .collect();
        Ok((values, plan.catalogue))
    }

    async fn policy_measurements(
        &self,
        inputs: &PolicyInputs,
        plan: &Plan,
    ) -> Result<PolicyMeasurements> {
        let tags = Provide::<KnowledgeTags>::get(&self.evidence.own, &())
            .await?
            .tags;
        let now = Utc::now();
        let targets: Vec<_> = inputs
            .scopes
            .iter()
            .map(|s| (s.scope.name.as_str(), s.subjects.as_slice()))
            .collect();
        let shared = self.evidence.shared(&targets).await?;
        // `#278` phase 2: the Policy tab (`check_evaluation::Provider`) and
        // this rollup (`compliance.<framework>`/`open_controls.<framework>`,
        // which feeds Goals, Quality measures and dashboard tiles) must
        // agree on a `check: metric` control's status -- one gatherer, per
        // AGENTS.md. Same shared, unscoped computation; narrowed per
        // evaluated scope below, exactly the way the Policy tab already
        // does it.
        let (metric_values, reported_catalogue) = self
            .metric_values(&targets, plan.root.clone(), &plan.reported, now)
            .await?;
        let mut per_scope = Vec::new();
        for scope in &inputs.scopes {
            let subtree = subtree_names(&self.evidence.scopes, &scope.scope.name)?;
            let scoped_metrics = metrics_for_evaluated_scope(
                &metric_values,
                &reported_catalogue,
                &subtree,
                now,
            );
            let evidence = self
                .evidence
                .for_scope(
                    &scope.scope,
                    &scope.subjects,
                    &tags,
                    &inputs.attestations,
                    &shared,
                    scope.budget.as_ref(),
                    &scoped_metrics,
                    now,
                )
                .await?;
            per_scope.push(checks::evaluate(&scope.subjects, &evidence, now));
        }
        let mut frameworks: BTreeMap<String, StatusCounts> = BTreeMap::new();
        for status in evaluation_rollup::worst_across_scopes(&per_scope) {
            let counts = frameworks
                .entry(status.control.framework.clone())
                .or_default();
            if status.kind {
                counts.add(status.status.kind());
            }
        }
        Ok(PolicyMeasurements {
            rollup: frameworks
                .into_iter()
                .map(|(framework, counts)| PolicyFramework { framework, counts })
                .collect(),
        })
    }

    pub async fn finish(
        &self,
        plan: &Plan,
        gathered: Gathered,
        budgets: &QualityBudgets,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
    ) -> Result<Metrics> {
        let canonical_scope = plan.scope();
        let computing = &plan.computing;
        let facts = Facts::<L5>::new();
        let Gathered {
            spend,
            production,
            process,
            policy: policy_report,
            reported: reported_docs,
        } = gathered;
        if plan.needs_policy() && policy_report.is_none() {
            return Err(FactoryError::BadRequest(
                "policy metric declarations were not resolved".into(),
            ));
        }
        let needs_backup = computing
            .iter()
            .any(|(id, result)| result.is_ok() && is_backup_metric(id.as_str()));
        let needs_attested = computing
            .iter()
            .any(|(id, result)| result.is_ok() && is_attestation_metric(id.as_str()));
        // `#154`: never spawns `git`/`tmutil` -- `backup_fact` shares
        // `capture` with `backup_report` but not its repository or Time
        // Machine probes.
        let backup_fact = if needs_backup {
            Some(
                facts
                    .get::<BackupFact, _>(self.evidence.ports.backup(), &now)
                    .await?,
            )
        } else {
            None
        };
        // `#158`: one read shared by `conformance_rate.<category>` (any
        // number of distinct categories a request asks for) and
        // `gate_fail_rate` (every category) -- `categories: None`, so the
        // pure figure functions do their own per-category filtering rather
        // than this fetching once per category asked for.
        let attested = if needs_attested {
            let days = window
                .map(MetricsWindow::days)
                .unwrap_or(OPERATIONS_WINDOW_DAYS);
            let attestation_window = factory_process::window::Window::trailing(now, days);
            let scopes = canonical_scope.map(|_| &plan.members);
            Some(
                facts
                    .get::<AttestedRun, _>(
                        self.evidence.ports.attested(),
                        &AttestedQuery {
                            scopes: scopes.cloned(),
                            categories: None,
                            window: attestation_window,
                        },
                    )
                    .await?,
            )
        } else {
            None
        };
        let needs_environments = computing
            .iter()
            .any(|(id, r)| r.is_ok() && environment_metric(id.as_str()).is_some());
        let environments = if needs_environments {
            Some(
                facts
                    .get::<EnvironmentMetricFact, _>(
                        self.evidence.ports.environments(),
                        &canonical_scope.map(str::to_string),
                    )
                    .await?,
            )
        } else {
            None
        };

        let reported_sources = ReportedSources {
            catalogue: &plan.catalogue,
            docs: &reported_docs,
            members: canonical_scope.map(|_| &plan.members),
        };
        let sources = ComputeSources {
            spend: spend.as_ref(),
            production: production.as_ref(),
            policy_report: policy_report.as_ref(),
            process: &process,
            attested: attested.as_deref(),
            backup: backup_fact.as_ref(),
            environments: environments.as_ref(),
            reported: reported_sources,
        };
        let mut computed: BTreeMap<MetricId, MetricValue> = BTreeMap::new();
        let mut computed_series: BTreeMap<MetricId, MetricSeries> = BTreeMap::new();
        for (id, def_result) in computing {
            if def_result.is_err() {
                continue;
            }
            let (value, series) = self
                .compute_one(id, &sources, now, window, canonical_scope)
                .await?;
            computed.insert(id.clone(), value);
            if let Some(s) = series {
                computed_series.insert(id.clone(), s);
            }
        }

        let quality_report: Option<std::result::Result<QualityRollup, String>> = match &plan.quality
        {
            None => None,
            Some(Err(e)) => Some(Err(e.clone())),
            Some(Ok(inputs)) => {
                let mut values = computed.clone();
                for (id, def) in computing {
                    if let Err(reason) = def {
                        values.insert(id.clone(), unavailable_value(id, reason, now));
                    }
                }
                Some(
                    self.judge_quality(inputs, &values, now, budgets)
                        .await
                        .map(|(reports, _)| QualityRollup(reports))
                        .map_err(|e| e.to_string()),
                )
            }
        };

        let mut values = Vec::new();
        let mut series = Vec::new();
        let mut registry = Vec::new();

        for (id, def_result) in plan.resolved.clone() {
            let def = match def_result {
                Ok(def) => def,
                Err(reason) => {
                    values.push(unavailable_value(&id, reason, now));
                    // `metrics::registry()` lists the fixed, unbound family
                    // for a metric this module can name but not compute --
                    // v1 has no *parameterised* unavailable family, so an
                    // exact-id lookup always finds it for today's two.
                    if let Some(found) = metrics::registry()
                        .into_iter()
                        .find(|d| d.id == id.as_str())
                    {
                        registry.push(found.into());
                    }
                    continue;
                }
            };
            let value = if let Some(characteristic) = id.as_str().strip_prefix("quality.") {
                match quality_report.as_ref().expect("needs_quality set") {
                    Ok(rollup) => quality_value(&id, rollup, characteristic, now),
                    Err(error) => MetricValue {
                        id: id.clone(),
                        value: None,
                        as_of: now,
                        reason: Some(format!("the quality evaluation failed: {error}")),
                    },
                }
            } else {
                if let Some(s) = computed_series.remove(&id) {
                    series.push(s);
                }
                computed
                    .remove(&id)
                    .expect("every available non-quality id was computed")
            };
            values.push(value);
            let mut def_view: MetricDefView = def.into();
            // `#278`: `resolve`'s own `reported_def` is a generic
            // placeholder (it has no access to the declaration); overlay
            // the scope's own declared title, unit and direction here, the
            // one place that declaration is actually in scope -- this is
            // also how the dashboard tile glyph (`ui/js/dashboard-tiles-
            // model.js`'s `betterGlyph`) ends up reading the real
            // direction, since it reads `better` off this same registry
            // entry, never `metrics::resolve` fresh.
            if let Some((source_id, metric_id)) = reported_segments(id.as_str()) {
                if let Some(declared) = plan
                    .catalogue
                    .sources
                    .get(source_id)
                    .and_then(|source| source.declared.get(metric_id))
                {
                    def_view.title = declared.title.clone();
                    def_view.unit = declared.unit;
                    def_view.better = declared.better;
                }
            }
            registry.push(def_view);
        }

        Ok(Metrics {
            values,
            series,
            registry,
            findings: plan.catalogue.findings.clone(),
        })
    }
    async fn judge_quality(
        &self,
        inputs: &quality_inputs::Inputs,
        values: &BTreeMap<MetricId, MetricValue>,
        now: DateTime<Utc>,
        budgets: &QualityBudgets,
    ) -> Result<(Vec<quality::ScopeReport>, Vec<quality::Finding>)> {
        let budgets = budgets
            .as_ref()
            .map_err(|error| FactoryError::Other(anyhow::anyhow!(error.clone())))?;
        let scopes: Vec<_> = inputs
            .trees
            .iter()
            .map(|(scope, tree)| evidence::QualityScope {
                scope: scope.scope.clone(),
                tree,
                budget: budgets.get(&scope.id).cloned(),
            })
            .collect();
        self.evidence.judge_quality(&scopes, values, now).await
    }
    async fn compute_one(
        &self,
        id: &MetricId,
        sources: &ComputeSources<'_>,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
        scope: Option<&str>,
    ) -> Result<(MetricValue, Option<MetricSeries>)> {
        let production = sources.production;
        let policy_report = sources.policy_report;
        if let Some(measurement) = sources.process.get(id.as_str()) {
            return Ok((
                MetricValue {
                    id: id.clone(),
                    value: measurement.value,
                    as_of: measurement.as_of,
                    reason: measurement.reason.clone(),
                },
                None,
            ));
        }
        let daily = || &production.expect("needs_production set").daily;
        Ok(if id.as_str() == "throughput_week" {
            let days = window.map(MetricsWindow::days).unwrap_or(7) as usize;
            let mut s = throughput_series(daily(), days);
            let value = match window {
                Some(_) => exact_production_value(
                    id,
                    &production.expect("needs_production set").buckets,
                    days,
                    now,
                ),
                None => value_from_series(&s, now, "no finished runs recorded yet"),
            };
            align_series_end(&mut s, &value, now);
            (value, Some(s))
        } else if id.as_str() == "first_pass_yield" {
            let days = window.map(MetricsWindow::days).unwrap_or(28) as usize;
            let mut s = first_pass_yield_series(daily(), days);
            let value = match window {
                Some(_) => exact_production_value(
                    id,
                    &production.expect("needs_production set").buckets,
                    days,
                    now,
                ),
                None => ratio_value(
                    value_from_series(&s, now, &no_recent_runs(days)),
                    daily(),
                    days,
                    now,
                ),
            };
            align_series_end(&mut s, &value, now);
            (value, Some(s))
        } else if id.as_str() == "scrap_rate" {
            let days = window.map(MetricsWindow::days).unwrap_or(28) as usize;
            let mut s = scrap_rate_series(daily(), days);
            let value = match window {
                Some(_) => exact_production_value(
                    id,
                    &production.expect("needs_production set").buckets,
                    days,
                    now,
                ),
                None => ratio_value(
                    value_from_series(&s, now, &no_recent_runs(days)),
                    daily(),
                    days,
                    now,
                ),
            };
            align_series_end(&mut s, &value, now);
            (value, Some(s))
        } else if matches!(id.as_str(), "unit_cost" | "tokens_per_run") {
            let cohort = sources
                .spend
                .expect("needs_spend set")
                .finished
                .as_ref()
                .expect("finished spend query");
            let figure = if id.as_str() == "unit_cost" {
                &cohort.unit_cost
            } else {
                &cohort.tokens_per_run
            };
            (
                MetricValue {
                    id: id.clone(),
                    value: figure.value,
                    as_of: figure.as_of.unwrap_or(now),
                    reason: figure.reason.clone(),
                },
                None,
            )
        } else if id.as_str() == "cost_week" {
            (self.cost_week_value(id, scope, now, window).await?, None)
        } else if is_backup_metric(id.as_str()) {
            (
                backup_metric_value(id, sources.backup.expect("needs_backup set")),
                None,
            )
        } else if let Some((name, env)) = environment_metric(id.as_str()) {
            (
                environment_value(
                    id,
                    sources.environments.expect("needs_environments set"),
                    name,
                    env,
                    now,
                ),
                None,
            )
        } else if let Some(framework) = id.as_str().strip_prefix("compliance.") {
            (
                compliance_value(id, policy_report.expect("needs_policy set"), framework, now),
                None,
            )
        } else if let Some(framework) = id.as_str().strip_prefix("open_controls.") {
            (
                open_controls_value(id, policy_report.expect("needs_policy set"), framework, now),
                None,
            )
        } else if let Some(category) = id.as_str().strip_prefix("conformance_rate.") {
            (
                attestation_metric_value(
                    id,
                    sources.attested.expect("needs_attested set"),
                    category,
                    now,
                ),
                None,
            )
        } else if id.as_str() == "gate_fail_rate" {
            let figure =
                crate::conformance::gate_fail_rate(sources.attested.expect("needs_attested set"));
            (figure_to_value(id, &figure, now), None)
        } else if id.as_str() == "review_reject_rate" {
            let figure = crate::conformance::review_reject_rate(
                sources.attested.expect("needs_attested set"),
            );
            (figure_to_value(id, &figure, now), None)
        } else if let Some(dataset) = id.as_str().strip_prefix("bench.resolve_rate.") {
            (self.bench_resolve_rate_value(id, dataset, now).await?, None)
        } else if let Some((source_id, metric_id)) = reported_segments(id.as_str()) {
            let in_subtree = match sources.reported.members {
                None => true,
                Some(members) => sources
                    .reported
                    .catalogue
                    .sources
                    .get(source_id)
                    .is_some_and(|source| members.contains(&source.scope.name)),
            };
            (
                reported::value_for(
                    id,
                    source_id,
                    metric_id,
                    sources.reported.catalogue,
                    sources.reported.docs,
                    in_subtree,
                    now,
                ),
                None,
            )
        } else {
            // Every family `metrics::resolve` returns `Ok` for today has
            // a branch above; a future metric added to the registry
            // without one here comes back honestly unresolved rather
            // than panicking a request that merely asked for it --
            // The registry has no exhaustive metric enum, so adding a
            // definition still requires adding its computation here.
            (
                MetricValue {
                    id: id.clone(),
                    value: None,
                    as_of: now,
                    reason: Some("no computation wired for this metric yet".to_string()),
                },
                None,
            )
        })
    }

    async fn bench_resolve_rate_value(
        &self,
        id: &MetricId,
        dataset: &str,
        now: DateTime<Utc>,
    ) -> Result<MetricValue> {
        let measured = Provide::<factory_kernel::BenchResolutionFact>::get(
            &self.evidence.own,
            &dataset.to_string(),
        )
        .await?;
        let Some(run) = measured else {
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!("no settled bench run for dataset {dataset:?}")),
            });
        };
        let (pass, fail) = (run.passed, run.failed);
        if pass + fail == 0 {
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!(
                    "nothing gated in the newest settled run of {dataset:?}"
                )),
            });
        }
        // As of when that run settled, not when this was asked: a resolve
        // rate is exactly as old as the run it came from, and a reader that
        // holds it to a freshness window (a quality scenario's `max_age`)
        // has to see that. A settled run with no end recorded falls back to
        // its start, the older of the two, never to `now`.
        Ok(MetricValue {
            id: id.clone(),
            value: Some(f64::from(pass) / f64::from(pass + fail)),
            as_of: run.ended_at.unwrap_or(run.started_at),
            reason: None,
        })
    }

    /// `cost_week` (#164): `spend`'s own known sum over runs
    /// started in the trailing window -- `window`'s own days when given, 7
    /// otherwise, an unnormalised sum either way (`throughput_week`'s own
    /// rule for an explicit window). `spend`, not the `needs_runs`
    /// prefetch: there is exactly one path to a spend figure, and this is
    /// it. `None`, with a reason naming the known sum and the counts,
    /// whenever the window holds a run whose usage is unknown, whose cost
    /// is unknown, or whose reading is a lower bound -- any one of those
    /// makes the sum something other than the whole truth.
    async fn cost_week_value(
        &self,
        id: &MetricId,
        scope: Option<&str>,
        now: DateTime<Utc>,
        window: Option<MetricsWindow>,
    ) -> Result<MetricValue> {
        let days = window.map(MetricsWindow::days).unwrap_or(7);
        let report = Facts::<L5>::new()
            .get::<factory_kernel::CostReport, _>(
                self.evidence.ports.spend(),
                &factory_process::usage::SpendQuery {
                    scope: scope.map(str::to_string),
                    from: Some(now - chrono::Duration::days(days)),
                    to: Some(now),
                    group_by: factory_kernel::CostGroupBy::Scope,
                    ..Default::default()
                },
            )
            .await?;
        let total = &report.total;
        let unmeasured = total.runs_unknown + total.runs_cost_unknown;
        let lower_bound = total.runs_partial;
        let unattributed = if scope.is_some() {
            report.unattributed_runs
        } else {
            0
        };
        if unmeasured > 0 || lower_bound > 0 || unattributed > 0 {
            let known_runs = total
                .runs
                .saturating_sub(unmeasured)
                .saturating_sub(lower_bound);
            return Ok(MetricValue {
                id: id.clone(),
                value: None,
                as_of: now,
                reason: Some(format!(
                    "${:.2} known over {known_runs} of {} runs started in the trailing {days} days; \
                     {unmeasured} unmeasured, {lower_bound} a lower bound, {unattributed} unattributed -- see factory cost --since {days}d",
                    total.cost_usd, total.runs,
                )),
            });
        }
        Ok(MetricValue {
            id: id.clone(),
            value: Some(total.cost_usd),
            as_of: now,
            reason: None,
        })
    }
}
fn is_production_metric(id: &str) -> bool {
    matches!(id, "throughput_week" | "first_pass_yield" | "scrap_rate")
}

fn is_operations_metric(id: &str) -> bool {
    matches!(
        id,
        "cycle_time_p50"
            | "cycle_time_p85"
            | "queue_wait_p95"
            | "fail_rate"
            | "rework_rate"
            | "time_to_recover_p50"
    )
}

/// The window the operations metrics are read over -- the trailing 28 days
/// the production ratios use.
const OPERATIONS_WINDOW_DAYS: i64 = 28;

fn is_usage_metric(id: &str) -> bool {
    id == "estimate_accuracy"
}

fn is_hours_metric(id: &str) -> bool {
    matches!(id, "agent_hours" | "blocked_hours")
}

/// `#154`'s two backup metrics -- both read off one `the backup provider`
/// call, at most once per `metrics_for` call.
fn is_backup_metric(id: &str) -> bool {
    matches!(id, "backup_age_hours" | "backup_verified_age_days")
}

fn is_intake_metric(id: &str) -> bool {
    matches!(
        id,
        "ready_rate" | "needs_info_rate" | "duplicate_rate" | "intake_lead_time"
    )
}

/// `pub(crate)`, not private: `#278`'s `checks::is_circular_metric` reuses
/// this exact prefix test to refuse a policy `check: metric` on
/// `compliance.*` as circular, rather than writing the same match a third
/// time (`quality::is_quality_metric` is the second, over a `MetricId`).
pub(crate) fn is_policy_metric(id: &str) -> bool {
    id.starts_with("compliance.") || id.starts_with("open_controls.")
}

pub(crate) fn is_quality_metric(id: &str) -> bool {
    id.starts_with("quality.")
}

/// `#278`: a scope-reported metric, `reported.<source>.<metric>`.
fn is_reported_metric(id: &str) -> bool {
    id.starts_with("reported.")
}

/// `reported.<source>.<metric>` split into its two bound segments --
/// `None` only if `id` does not have the shape `MetricId`/`resolve` both
/// already enforce, which cannot happen for an id this module itself put
/// into `computing`.
fn reported_segments(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix("reported.")?.split_once('.')
}

/// `#278` phase 2: `scope_name`'s own subtree -- itself and everything
/// below it, by name -- the selection phase 1's coverage rule
/// (`reported::value_for`'s `in_subtree`) already reads a `reported.*` id
/// against for a scoped metrics request. A `check: metric` applies the
/// exact same rule per *evaluated* scope instead: see
/// [`metrics_for_evaluated_scope`].
pub(crate) fn subtree_names(scopes: &ScopeTree, scope_name: &str) -> Result<BTreeSet<String>> {
    let (_, targets) = scopes.subtree_scopes(Some(scope_name))?;
    Ok(targets.iter().map(|s| s.name.clone()).collect())
}

/// `#278` phase 2: [`Service::metric_values`]'s shared, unscoped read,
/// narrowed to what a control evaluated *at* `subtree`'s own scope may
/// actually read as evidence -- a cheap post-filter on the one shared
/// computation, never a second read or a second path to the value. A
/// built-in id passes through untouched, keeping whatever scoping
/// `metric_values` already gave it (Quality stays instance-wide and
/// unchanged; this rule is for `check: metric` alone). A
/// `reported.<source>.<metric>` id whose declaring scope falls outside
/// `subtree` is replaced with [`reported::outside_subtree`]'s own `None`
/// and reason -- the identical wording a scoped `/api/metrics` read
/// already gives for the same gap (`reported::value_for`), so the two
/// never explain one absence two different ways. An id whose source does
/// not resolve at all keeps whatever reason the unscoped read already
/// gave it (an unknown source, say) -- there is no declaring scope to
/// compare `subtree` against, so this filter has nothing to add.
pub(crate) fn metrics_for_evaluated_scope(
    values: &BTreeMap<MetricId, MetricValue>,
    catalogue: &reported::Catalogue,
    subtree: &BTreeSet<String>,
    now: DateTime<Utc>,
) -> BTreeMap<MetricId, MetricValue> {
    values
        .iter()
        .map(|(id, value)| {
            let Some((source_id, _metric_id)) = reported_segments(id.as_str()) else {
                return (id.clone(), value.clone());
            };
            match catalogue.sources.get(source_id) {
                Some(source) if !subtree.contains(&source.scope.name) => (
                    id.clone(),
                    reported::outside_subtree(id, source_id, &source.scope.name, now),
                ),
                _ => (id.clone(), value.clone()),
            }
        })
        .collect()
}

/// `availability.<env>` and its siblings (`#185`): the metric's name and
/// the environment it names.
fn environment_metric(id: &str) -> Option<(&str, &str)> {
    let (name, env) = id.split_once('.')?;
    (metrics::ENVIRONMENT_METRICS.contains(&name) && !env.contains('.')).then_some((name, env))
}

/// `#158`: conformance, gate failure and independent review rejection, the
/// registry families `the attestation provider` backs -- one read shared
/// by all, whatever mix of categories and however many a
/// request actually asks for.
pub fn is_attestation_metric(id: &str) -> bool {
    id.starts_with("conformance_rate.") || id == "gate_fail_rate" || id == "review_reject_rate"
}

/// Sum `finished`/`scrapped`/`reworked`/`first_pass` over a `window`-day
/// trailing window ending at each day in `daily` (clipped at the start of
/// the grid, so the earliest few points are over a shorter window than
/// `window`), and hand each sum to `calc` -- `None` skips that day's point
/// entirely (used for a ratio with nothing finished yet to divide by)
/// rather than fabricating a number.
fn rolling_series<F>(
    daily: &[factory_kernel::ProductionBucket],
    window: usize,
    calc: F,
) -> Vec<(NaiveDate, f64)>
where
    F: Fn(u32, u32, u32, u32) -> Option<f64>,
{
    let mut points = Vec::new();
    for i in 0..daily.len() {
        let start = i.saturating_sub(window.saturating_sub(1));
        let (mut finished, mut scrapped, mut reworked, mut first_pass) = (0u32, 0u32, 0u32, 0u32);
        for bucket in &daily[start..=i] {
            finished += bucket.finished;
            scrapped += bucket.scrapped;
            reworked += bucket.reworked;
            first_pass += bucket.first_pass;
        }
        if let Some(value) = calc(finished, scrapped, reworked, first_pass) {
            points.push((daily[i].from.date_naive(), value));
        }
    }
    points
}

fn throughput_series(daily: &[factory_kernel::ProductionBucket], window: usize) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("throughput_week").expect("fixed id"),
        points: rolling_series(daily, window, |finished, _, _, _| Some(f64::from(finished))),
    }
}

/// `first_pass / finished`, never `1 - reworked / finished` -- a bucket
/// where nothing was ever retried but nothing ever succeeded either
/// (`reworked: 0`, `first_pass: 0`) is a real `0.0`, not a manufactured
/// `1.0`. See `production.rs`'s module doc comment for `first_pass`'s own
/// definition.
fn first_pass_yield_series(
    daily: &[factory_kernel::ProductionBucket],
    window: usize,
) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("first_pass_yield").expect("fixed id"),
        points: rolling_series(daily, window, |finished, _, _, first_pass| {
            if finished == 0 {
                None
            } else {
                Some(f64::from(first_pass) / f64::from(finished))
            }
        }),
    }
}

fn scrap_rate_series(daily: &[factory_kernel::ProductionBucket], window: usize) -> MetricSeries {
    MetricSeries {
        id: MetricId::new("scrap_rate").expect("fixed id"),
        points: rolling_series(daily, window, |finished, scrapped, _, _| {
            if finished == 0 {
                None
            } else {
                Some(f64::from(scrapped) / f64::from(finished))
            }
        }),
    }
}

/// A series' own last point is always the metric's current value -- this is
/// the one place that invariant is enforced, so `throughput_week`/
/// `first_pass_yield`/`scrap_rate` can never disagree with their own
/// sparkline.
fn value_from_series(series: &MetricSeries, now: DateTime<Utc>, empty_reason: &str) -> MetricValue {
    let value = series.points.last().map(|(_, v)| *v);
    MetricValue {
        id: series.id.clone(),
        value,
        as_of: now,
        reason: if value.is_none() {
            Some(empty_reason.to_string())
        } else {
            None
        },
    }
}

/// The explicit request window is an exact timestamp interval. Production's
/// `buckets` cover that interval (including its partial first day), whereas
/// `daily` is the calendar-aligned 53-week history used for sparklines.
fn exact_production_value(
    id: &MetricId,
    buckets: &[ProductionBucket],
    window_days: usize,
    now: DateTime<Utc>,
) -> MetricValue {
    let (finished, scrapped, first_pass) =
        buckets
            .iter()
            .fold((0u32, 0u32, 0u32), |(f, s, p), bucket| {
                (
                    f + bucket.finished,
                    s + bucket.scrapped,
                    p + bucket.first_pass,
                )
            });
    if id.as_str() == "throughput_week" {
        return MetricValue {
            id: id.clone(),
            value: Some(f64::from(finished)),
            as_of: now,
            reason: None,
        };
    }
    if finished == 0 {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(no_recent_runs(window_days)),
        };
    }
    let numerator = if id.as_str() == "first_pass_yield" {
        first_pass
    } else {
        scrapped
    };
    let as_of = buckets
        .iter()
        .rev()
        .find(|bucket| bucket.finished > 0)
        .map(|bucket| bucket.to)
        .unwrap_or(now);
    MetricValue {
        id: id.clone(),
        value: Some(f64::from(numerator) / f64::from(finished)),
        as_of,
        reason: None,
    }
}

fn align_series_end(series: &mut MetricSeries, value: &MetricValue, now: DateTime<Utc>) {
    let Some(value) = value.value else { return };
    match series.points.last_mut() {
        Some((day, current)) if *day == now.date_naive() => *current = value,
        _ => series.points.push((now.date_naive(), value)),
    }
}

fn no_recent_runs(window: usize) -> String {
    format!("no finished runs in the trailing {window} days")
}

/// A production ratio as its registry entry defines it -- over the trailing
/// `window` days -- with an honest `as_of`.
///
/// The rolling series skips a day whose own window finished nothing, so
/// its last point can be a *weeks-old* day's ratio; taken as-is, a team
/// that stopped running things 35 days ago would still read a value.
/// When nothing in the trailing `window` finished, there is no value for
/// "the trailing 28 days" at all: `None`, with the reason. Otherwise the
/// value is `as_of` the end of the newest daily bucket in that window that
/// finished anything -- when the data behind it stopped, to the day,
/// rather than the moment it was asked for -- so a freshness window (a
/// quality scenario's `max_age`) can read it as stale. The last bucket's
/// `to` is already clipped to the query's own moment
/// (`ProductionBucket::to`), so a run finished today reads as of now.
///
/// `throughput_week` deliberately keeps `now`: it is a count over a window
/// that ends now, so even a zero is a current fact, not an old one.
fn ratio_value(
    mut value: MetricValue,
    daily: &[factory_kernel::ProductionBucket],
    window: usize,
    now: DateTime<Utc>,
) -> MetricValue {
    let start = daily.len().saturating_sub(window);
    match daily[start..].iter().rev().find(|b| b.finished > 0) {
        Some(newest) if value.value.is_some() => value.as_of = newest.to,
        _ => {
            value.value = None;
            value.as_of = now;
            value.reason = Some(no_recent_runs(window));
        }
    }
    value
}

/// Every backing read a `metrics_for` call may have gathered, bundled so
/// `compute_one` takes one reference instead of one parameter per family --
/// each field `Some` exactly when some asked id needed it.
struct ComputeSources<'a> {
    spend: Option<&'a factory_kernel::CostReport>,
    production: Option<&'a factory_kernel::ProductionFact>,
    policy_report: Option<&'a PolicyMeasurements>,
    process: &'a BTreeMap<String, factory_kernel::ProcessMetricFact>,
    backup: Option<&'a factory_kernel::BackupFact>,
    environments: Option<&'a BTreeMap<String, EnvironmentMetricFact>>,
    /// `#158`: `attested_runs`'s finished runs, shared by
    /// `conformance_rate.<category>` and `gate_fail_rate`.
    attested: Option<&'a [crate::conformance::AttestedRun]>,
    /// `#278`: the validated declarations and whatever sources were
    /// actually read for this call, plus the selected subtree's own
    /// members (`None` for an unscoped request, which includes every
    /// scope).
    reported: ReportedSources<'a>,
}

#[derive(Clone, Copy)]
struct ReportedSources<'a> {
    catalogue: &'a reported::Catalogue,
    docs: &'a BTreeMap<String, std::result::Result<reported::Document, String>>,
    members: Option<&'a BTreeSet<String>>,
}

/// One environment metric off the card the Operations tab draws, so the
/// two can never disagree. A figure with nothing to compute it from is
/// `None` with the reason.
fn environment_value(
    id: &MetricId,
    cards: &BTreeMap<String, EnvironmentMetricFact>,
    name: &str,
    env: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    let answer = |value: Option<f64>, reason: &str| MetricValue {
        id: id.clone(),
        value,
        as_of: now,
        reason: value.is_none().then(|| reason.to_string()),
    };
    let Some(card) = cards.get(env) else {
        return answer(
            None,
            &format!("no environment named {env:?} is declared or deployed to"),
        );
    };
    let no_samples = "no health samples in the window yet";
    match name {
        "availability" => answer(card.availability, no_samples),
        "error_budget" if !card.has_slo => {
            answer(None, &format!("environment {env:?} declares no SLO"))
        }
        "error_budget" => answer(card.error_budget, no_samples),
        "incidents" => answer(card.availability.map(|_| card.incidents as f64), no_samples),
        "mttr" => answer(card.mttr, "no incident ended in the window"),
        "time_to_restore_p50" => {
            answer(card.time_to_restore_p50, "no incident ended in the window")
        }
        "deploy_frequency" => answer(
            card.deploy_frequency,
            "no successful deployment in the window",
        ),
        "lead_time_p50" => answer(
            card.lead_time_p50,
            "no successful deployment in the window says when its commit was made",
        ),
        "change_failure_rate" => answer(
            card.change_failure_rate,
            "no deployment finished in the window",
        ),
        _ => answer(None, "no computation wired for this metric yet"),
    }
}

fn unavailable_value(id: &MetricId, reason: &str, now: DateTime<Utc>) -> MetricValue {
    MetricValue {
        id: id.clone(),
        value: None,
        as_of: now,
        reason: Some(reason.to_string()),
    }
}

/// `backup_age_hours`/`backup_verified_age_days` (#154), off one
/// `backup_fact` call -- `as_of` is always `fact.at`, the instant it
/// was derived, never the moment the metric was asked for. `fact.recent`/
/// `fact.verified` being `None` is `resolve_backup_fact`'s own signal that
/// the destination could not be reached at all -- the only way either field
/// reads indeterminate rather than a plain `false`.
fn backup_metric_value(id: &MetricId, fact: &factory_kernel::BackupFact) -> MetricValue {
    if !fact.configured {
        return unavailable_value(id, "no backup is configured", fact.at);
    }
    match id.as_str() {
        "backup_age_hours" => match (fact.recent, fact.newest) {
            (None, _) => unavailable_value(id, "the destination is not reachable", fact.at),
            (Some(_), None) => unavailable_value(id, "no snapshot yet", fact.at),
            (Some(_), Some(newest)) => MetricValue {
                id: id.clone(),
                value: Some(fact.at.signed_duration_since(newest).num_seconds() as f64 / 3600.0),
                as_of: fact.at,
                reason: None,
            },
        },
        "backup_verified_age_days" => {
            if fact.verified.is_none() {
                return unavailable_value(id, "the destination is not reachable", fact.at);
            }
            if fact.newest.is_none() {
                return unavailable_value(id, "no snapshot yet", fact.at);
            }
            match &fact.last_verified {
                None => unavailable_value(
                    id,
                    "no snapshot in the destination has been verified",
                    fact.at,
                ),
                Some(v) if !v.ok => unavailable_value(
                    id,
                    &format!(
                        "the newest verification failed at {}",
                        v.at.format("%Y-%m-%d %H:%M UTC")
                    ),
                    fact.at,
                ),
                Some(v) => MetricValue {
                    id: id.clone(),
                    value: Some(fact.at.signed_duration_since(v.at).num_seconds() as f64 / 86400.0),
                    as_of: fact.at,
                    reason: None,
                },
            }
        }
        // `metrics::resolve` names only the two ids above for this family;
        // an id it never returns cannot reach here (`compute_one`'s own
        // catch-all doc comment).
        _ => unavailable_value(id, "no computation wired for this metric yet", fact.at),
    }
}

/// The per-scope reports a `quality.<characteristic>` value is summed
/// from -- `QualityReport::scopes`' trees without the report around them,
/// since `metrics` judges them itself (`judge_quality`) rather than
/// asking for a whole report.
struct QualityRollup(Vec<crate::quality::ScopeReport>);

fn compliance_value(
    id: &MetricId,
    report: &PolicyMeasurements,
    framework: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    let Some(rollup) = report.rollup.iter().find(|r| r.framework == framework) else {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("no catalogue loaded for framework {framework:?}")),
        };
    };
    let c = &rollup.counts;
    let counted = c.satisfied + c.attested + c.stale + c.open + c.not_applicable;
    if counted == 0 {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!(
                "framework {framework:?} has no regulation/standard controls counted"
            )),
        };
    }
    let value = (c.satisfied + c.attested + c.not_applicable) as f64 / counted as f64;
    MetricValue {
        id: id.clone(),
        value: Some(value),
        as_of: now,
        reason: None,
    }
}

/// `quality.<characteristic>`: of every declared scenario under
/// `characteristic`, counted once per scope it applies in within the selected
/// subtree, the share that is `met`. A draft or `no_data` scenario counts in
/// the denominator -- declared but not shown to be met is not met, the same
/// "never green without evidence" rule the tab itself keeps. `None`, with the
/// reason, for a characteristic ISO 25010 does not name or one no selected
/// scope declares anything under: nothing declared is not "all met".
fn quality_value(
    id: &MetricId,
    rollup: &QualityRollup,
    characteristic: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    let unknown = crate::quality::CATALOGUE
        .iter()
        .all(|c| c.id != characteristic);
    if unknown {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!(
                "{characteristic:?} is not an ISO 25010 characteristic"
            )),
        };
    }
    let (met, declared) = rollup
        .0
        .iter()
        .flat_map(|s| &s.attributes)
        .filter(|a| a.characteristic == characteristic)
        .flat_map(|a| &a.scenarios)
        .fold((0u32, 0u32), |(met, all), s| {
            (met + u32::from(s.status == ScenarioStatus::Met), all + 1)
        });
    if declared == 0 {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!(
                "no scope declares a quality scenario under {characteristic}"
            )),
        };
    }
    MetricValue {
        id: id.clone(),
        value: Some(f64::from(met) / f64::from(declared)),
        as_of: now,
        reason: None,
    }
}

fn open_controls_value(
    id: &MetricId,
    report: &PolicyMeasurements,
    framework: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    let Some(rollup) = report.rollup.iter().find(|r| r.framework == framework) else {
        return MetricValue {
            id: id.clone(),
            value: None,
            as_of: now,
            reason: Some(format!("no catalogue loaded for framework {framework:?}")),
        };
    };
    let c = &rollup.counts;
    MetricValue {
        id: id.clone(),
        value: Some((c.open + c.stale) as f64),
        as_of: now,
        reason: None,
    }
}

/// `#158`: a `crate::conformance::ConformanceFigure` as a
/// `MetricValue` -- `as_of` falls back to `now` only when the figure itself
/// has none, the same rule `intake_value` and the operations metrics keep.
fn figure_to_value(
    id: &MetricId,
    figure: &crate::conformance::ConformanceFigure,
    now: DateTime<Utc>,
) -> MetricValue {
    MetricValue {
        id: id.clone(),
        value: figure.value,
        as_of: figure.as_of.unwrap_or(now),
        reason: figure.reason.clone(),
    }
}

fn attestation_metric_value(
    id: &MetricId,
    runs: &[crate::conformance::AttestedRun],
    category: &str,
    now: DateTime<Utc>,
) -> MetricValue {
    figure_to_value(
        id,
        &crate::conformance::conformance_rate(runs, category),
        now,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scopes() -> ScopeTree {
        ScopeTree {
            scopes: [
                ("parent", "projects/work"),
                ("kid", "projects/work/child"),
                ("parent/fake", "projects/work-other"),
            ]
            .into_iter()
            .map(|(name, path)| ScopeNode {
                name: name.into(),
                path: path.into(),
            })
            .collect(),
        }
    }
    #[tokio::test]
    async fn scope_validation_precedes_unknown_and_instance_wide_ids() {
        for name in ["not_a_metric", "bench.resolve_rate.x", "goal_tasks_done.x"] {
            let ids = [MetricId::new(name).unwrap()];
            let outcome = Plan::prepare(
                &ids,
                PathBuf::from("/unneeded-no-io"),
                &scopes(),
                &quality_inputs::Configuration::default(),
                &reported::Configuration::default(),
                Some("missing"),
            )
            .await;
            assert!(matches!(outcome, Err(FactoryError::NoSuchScope(name)) if name == "missing"));
        }
        let outcome = Plan::prepare(
            &[MetricId::new("not_a_metric").unwrap()],
            PathBuf::from("/unneeded-no-io"),
            &scopes(),
            &quality_inputs::Configuration::default(),
            &reported::Configuration::default(),
            None,
        )
        .await;
        assert!(
            matches!(outcome, Err(FactoryError::BadRequest(message)) if message == "not_a_metric is not a known metric")
        );
    }
    #[tokio::test]
    async fn plan_deduplicates_first_seen_and_selects_by_path_not_name_prefix() {
        let ids = ["fail_rate", "cost_week", "fail_rate"].map(|name| MetricId::new(name).unwrap());
        let plan = Plan::prepare(
            &ids,
            PathBuf::from("/unneeded-no-io"),
            &scopes(),
            &quality_inputs::Configuration::default(),
            &reported::Configuration::default(),
            Some("projects/work"),
        )
        .await
        .unwrap();
        assert_eq!(plan.scope(), Some("parent"));
        assert_eq!(
            plan.members,
            BTreeSet::from(["parent".into(), "kid".into()])
        );
        assert_eq!(
            plan.resolved
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            ["fail_rate", "cost_week"]
        );
        assert!(
            plan.quality.is_none() && !plan.needs_policy() && plan.quality_budget_ids().is_empty()
        );
    }
    #[test]
    fn compliance_keeps_not_applicable_as_compliant_and_unknown_is_not_zero() {
        let now = Utc::now();
        let id = MetricId::new("compliance.test").unwrap();
        let report = PolicyMeasurements {
            rollup: vec![PolicyFramework {
                framework: "test".into(),
                counts: StatusCounts {
                    satisfied: 2,
                    attested: 1,
                    stale: 1,
                    open: 2,
                    not_applicable: 500,
                },
            }],
        };
        assert_eq!(
            compliance_value(&id, &report, "test", now).value,
            Some(503.0 / 506.0)
        );
        assert!(compliance_value(&id, &report, "absent", now)
            .value
            .is_none());
        assert_eq!(
            open_controls_value(&id, &report, "test", now).value,
            Some(3.0)
        );
        let empty = PolicyMeasurements {
            rollup: vec![PolicyFramework {
                framework: "test".into(),
                counts: StatusCounts::default(),
            }],
        };
        assert!(compliance_value(&id, &empty, "test", now).value.is_none());
        assert_eq!(
            open_controls_value(&id, &empty, "test", now).value,
            Some(0.0)
        );
    }
}
