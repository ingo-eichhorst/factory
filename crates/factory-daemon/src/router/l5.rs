//! Requests served by L5 Improvement. The arms are moved verbatim from the single
//! `dispatch_request` match; `Request::level` decides which file a request lands in.
use crate::engine::*;
use super::misrouted;

impl Engine {
    pub(super) async fn route_l5(
        self: &Arc<Self>,
        caller: &crate::access::Caller,
        req: Request,
    ) -> Result<Payload> {
        match req {
            Request::Knowledge => {
                let root = self.factory_snapshot().root;
                let index = tokio::task::spawn_blocking(move || factory_core::knowledge::index(&root))
                    .await
                    .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge walk: {e}")))?;
                Ok(Payload::Knowledge {
                    root: index.root,
                    present: index.present,
                    legacy: index.legacy,
                    pages: index.pages,
                    tags: index.tags,
                    documents: index.documents,
                    gaps: index.gaps,
                    findings: index.findings,
                })
            }
            Request::KnowledgeSearch { text, tags, scope, limit } => {
                let (root, provider_name) = {
                    let factory = self.factory_snapshot();
                    (factory.root, factory.config.daemon.knowledge_provider)
                };
                let provider = self.shared.registry.knowledge(&provider_name)?;
                let query = factory_core::adapter::KnowledgeQuery {
                    text,
                    tags,
                    scope,
                    limit: limit
                        .unwrap_or(factory_core::adapter::knowledge::DEFAULT_SEARCH_LIMIT)
                        .min(factory_core::adapter::knowledge::MAX_SEARCH_LIMIT),
                };
                if query.text.trim().is_empty() && query.tags.is_empty() {
                    return Err(FactoryError::BadRequest(
                        "nothing to search for: give some text, a tag, or both".into(),
                    ));
                }
                let hits = provider.search(&root, &query).await?;
                Ok(Payload::KnowledgeHits {
                    provider: provider_name,
                    vault: factory_core::knowledge::vault_root(&root).display().to_string(),
                    hits,
                })
            }
            Request::KnowledgeImport { source, into, overwrite } => {
                let root = self.factory_snapshot().root;
                let source = PathBuf::from(source);
                let result = tokio::task::spawn_blocking(move || {
                    factory_core::knowledge::import(&root, &source, into.as_deref(), overwrite)
                })
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge import: {e}")))?
                .map_err(FactoryError::BadRequest)?;
                self.knowledge_changed(&result.copied).await;
                Ok(knowledge_write_payload(result))
            }
            Request::KnowledgeAdd { sources, into, overwrite } => {
                let root = self.factory_snapshot().root;
                let sources: Vec<PathBuf> = sources.into_iter().map(PathBuf::from).collect();
                let result = tokio::task::spawn_blocking(move || {
                    factory_core::knowledge::add(&root, &sources, into.as_deref(), overwrite)
                })
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge add: {e}")))?;
                self.knowledge_changed(&result.copied).await;
                Ok(knowledge_write_payload(result))
            }
            Request::KnowledgeWriteFile { path, overwrite, bytes } => {
                let root = self.factory_snapshot().root;
                let written = tokio::task::spawn_blocking(move || {
                    factory_core::knowledge::write_bytes(&root, &path, overwrite, &bytes)
                })
                .await
                .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge write: {e}")))?
                .map_err(FactoryError::BadRequest)?;
                self.knowledge_changed(std::slice::from_ref(&written)).await;
                Ok(knowledge_write_payload(factory_core::knowledge::WriteResult {
                    copied: vec![written],
                    ..Default::default()
                }))
            }
            Request::Benchmarks => {
                let factory = self.factory_snapshot();
                let configurations =
                    factory_core::benchmark::configurations(&factory.config.scopes, &factory.config.daemon.foreman);
                Ok(Payload::Benchmarks { configurations })
            }
            Request::Datasets => Ok(Payload::Datasets {
                root: self.factory_snapshot().datasets_dir().display().to_string(),
                datasets: self.l5_service().dataset_summaries()?,
            }),
            Request::Dataset { name } => {
                let (dataset, findings) = self.l5_service().dataset_view(&name)?;
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetCreate { name, description } => {
                let dataset = self.l5_service().dataset_create(&name, description).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.l5_service().known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetAddCases { name, cases } => {
                let dataset = self.l5_service().dataset_add_cases(&name, cases).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.l5_service().known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetImport { name, format, content, replace } => {
                let dataset = self.l5_service().dataset_import(&name, &format, &content, replace).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.l5_service().known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetFromTasks { name, task_ids } => {
                let dataset = self.l5_service().dataset_from_tasks(&name, task_ids).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.l5_service().known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetDeleteCase { name, id } => {
                let dataset = self.l5_service().dataset_delete_case(&name, &id).await?;
                let findings = factory_core::dataset::findings(&dataset, &self.l5_service().known_scope_names());
                Ok(Payload::Dataset { dataset, findings })
            }
            Request::DatasetDelete { name } => Ok(Payload::Deleted {
                deleted: self.l5_service().dataset_delete(&name).await?,
            }),
            Request::BenchRunStart {
                dataset,
                agents,
                attempts,
                concurrency,
                cases,
            } => {
                // Only the trailing name matters: the agent that actually
                // resolves in each case's own scope, not the scope a person
                // happened to find it under in the Configurations roster.
                let agents: Vec<String> = agents
                    .iter()
                    .map(|a| a.rsplit('/').next().unwrap_or(a).to_string())
                    .collect();
                let run = self
                    .start_bench_run(&dataset, agents, attempts.unwrap_or(1), concurrency.unwrap_or(1), cases)
                    .await?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::BenchRuns { dataset } => Ok(Payload::BenchRuns {
                runs: self.l5.bench.runs(dataset.as_deref(), 200).await?,
            }),
            Request::BenchRunGet { id } => {
                let run = self.l5.bench.get_run(&id).await?.ok_or_else(|| {
                    FactoryError::BadRequest(format!("no such bench run: {id:?}"))
                })?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::BenchRunCancel { id } => {
                let run = self.cancel_bench_run(&id).await?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::BenchRunClean { id } => {
                let run = self.clean_bench_run(&id).await?;
                let results = factory_core::bench::aggregate(&run.attempts);
                Ok(Payload::BenchRun { run, results })
            }
            Request::Metrics { ids, scope, window } => {
                let now = Utc::now();
                let ids = if ids.is_empty() { self.default_metric_ids().await } else { ids };
                let metrics = self.metrics_for(&ids, now, scope.as_deref(), window).await?;
                Ok(Payload::Metrics {
                    values: metrics.values,
                    series: metrics.series,
                    registry: metrics.registry,
                    findings: metrics.findings,
                })
            }
            Request::Signposts => Ok(Payload::Signposts {
                fact: self.signposts_fact(chrono::Utc::now(), true).await?,
            }),
            // `Event::QualityChanged`, when due, is published inside
            // `quality_report` itself -- it is a read that notices, not a write.
            Request::Quality { scope } => Ok(Payload::Quality {
                report: self.quality_report(scope.as_deref()).await?,
            }),
            // No event of its own: a created task already fired
            // `Event::TaskCreated` inside `Engine::create`, and answering an
            // already-open one changes nothing -- `PolicyRemediate`'s rule.
            Request::QualityRemediate { scope, attribute, scenario, agent } => Ok(Payload::QualityRemediate {
                result: self.quality_remediate(scope, attribute, scenario, agent).await?,
            }),
            // `#275`. No event of its own: filing publishes through
            // `Event::TaskEntry` (`Engine::entry`, inside `file_suggestion`)
            // like any other journaled report.
            Request::TaskSuggest { id, suggestion } => Ok(Payload::Suggestion {
                suggestion: self.l5_service().file_suggestion(&id, suggestion).await?,
            }),
            Request::Suggestions { scope, kind, target, state } => Ok(Payload::Suggestions {
                report: self.l5_service().suggestions_report(scope.as_deref(), kind, target, state).await?,
            }),
            Request::SuggestionGet { id } => Ok(Payload::Suggestion {
                suggestion: self.l5_service().suggestion_get(&id).await?,
            }),
            // No event of its own: the created task already fired
            // `Event::TaskCreated` inside `Engine::create`, the same rule
            // `PolicyRemediate`/`QualityRemediate` follow.
            Request::SuggestionTask { ids } => Ok(Payload::SuggestionTask {
                task: self.l5_service().suggestion_task(caller, ids).await?,
            }),
            Request::SuggestionDismiss { id, reason } => Ok(Payload::Suggestion {
                suggestion: self.l5_service().suggestion_dismiss(caller, &id, reason).await?,
            }),
            Request::SuggestionDone { id } => Ok(Payload::Suggestion {
                suggestion: self.l5_service().suggestion_done(caller, &id).await?,
            }),
            Request::SuggestionAsk { id, question } => Ok(Payload::Suggestion {
                suggestion: Box::pin(self.l5_service().suggestion_ask(caller, &id, question)).await?,
            }),
            other => Err(misrouted(other.level())),
        }
    }
}
