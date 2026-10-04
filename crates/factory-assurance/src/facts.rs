//! L5-owned live benchmark and knowledge fact reads.
//! The provider holds only its BenchStore and instance root, never a router.
use crate::bench_store::BenchStore;
use async_trait::async_trait;
use factory_kernel::{FactoryError, Result};
use factory_kernel::{GateCase, GateFact, KnowledgeTags, Provide};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};
pub struct Provider<'a> {
    bench: &'a BenchStore,
    root: PathBuf,
}

impl<'a> Provider<'a> {
    pub fn new(bench: &'a BenchStore, root: PathBuf) -> Self {
        Self { bench, root }
    }
}

#[async_trait]
impl Provide<factory_kernel::BenchResolutionFact> for Provider<'_> {
    type Query = String;
    type Value = Option<factory_kernel::BenchResolutionFact>;
    type Error = FactoryError;
    async fn get(&self, dataset: &String) -> Result<Self::Value> {
        let runs = self.bench.runs(Some(dataset), 200).await?;
        let Some(run) = runs.into_iter().find(|run| run.settled()) else {
            return Ok(None);
        };
        let results = crate::bench::aggregate(&run.attempts);
        let (passed, failed) = results.iter().fold((0, 0), |(passed, failed), result| {
            (passed + result.pass, failed + result.fail)
        });
        Ok(Some(factory_kernel::BenchResolutionFact {
            dataset: dataset.clone(),
            run_id: run.id,
            started_at: run.started_at,
            ended_at: run.ended_at,
            passed,
            failed,
        }))
    }
}

#[async_trait]
impl Provide<KnowledgeTags> for Provider<'_> {
    type Query = ();
    type Value = KnowledgeTags;
    type Error = FactoryError;
    async fn get(&self, _: &()) -> Result<Self::Value> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || KnowledgeTags {
            tags: crate::knowledge::index(&root)
                .tags
                .into_iter()
                .map(|t| t.name)
                .collect(),
        })
        .await
        .map_err(|e| FactoryError::Other(anyhow::anyhow!("knowledge index: {e}")))
    }
}
impl factory_kernel::FactProvider for Provider<'_> {
    type Level = factory_kernel::L5;
}
impl Provider<'_> {
    async fn gate_fact_for(&self, dataset: &str) -> Result<Option<GateFact>> {
        // `BenchStore::runs` loads every returned run's attempts eagerly, so
        // this is bounded rather than "every run this dataset ever had" --
        // the same 200 `Request::BenchRuns` already asks for, which a
        // dataset gated often enough to bury its newest settled run past
        // could still, in principle, outrun; the same edge case that bound
        // already accepts.
        let runs = self.bench.runs(Some(dataset), 200).await?;
        let Some(run) = runs.into_iter().find(|r| r.settled()) else {
            return Ok(None);
        };
        let cases = run
            .cases
            .iter()
            .map(|case| GateCase {
                id: case.id.clone(),
                gated: case.gate.is_some(),
                verdicts: run
                    .attempts
                    .iter()
                    .filter(|a| a.case_id == case.id)
                    .filter_map(|a| a.verdict)
                    .collect(),
            })
            .collect();
        Ok(Some(GateFact {
            run_id: run.id,
            ended_at: run.ended_at,
            cases,
        }))
    }
}
#[async_trait]
impl Provide<GateFact> for Provider<'_> {
    type Query = BTreeSet<String>;
    type Value = BTreeMap<String, GateFact>;
    type Error = FactoryError;
    async fn get(&self, names: &Self::Query) -> Result<Self::Value> {
        let mut gates = BTreeMap::new();
        for name in names {
            if let Some(fact) = self.gate_fact_for(name).await? {
                gates.insert(name.clone(), fact);
            }
        }
        Ok(gates)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::{BenchAttempt, BenchRun, BenchRunStatus, Verdict};
    use factory_kernel::{BenchResolutionFact, Facts, L6};

    fn run(id: &str, attempts: Vec<BenchAttempt>) -> BenchRun {
        BenchRun {
            id: id.into(),
            dataset: "demo".into(),
            dataset_revision: 1,
            cases: serde_json::from_value(serde_json::json!([
                {"id":"gated", "title":"gate", "scope":"demo", "gate":"true"},
                {"id":"ungated", "title":"no gate", "scope":"demo"}
            ]))
            .unwrap(),
            case_bases: BTreeMap::new(),
            agents: vec!["builder".into()],
            attempts_per_case: 1,
            concurrency: 1,
            status: BenchRunStatus::Running,
            attempts,
            started_at: "2026-09-25T12:00:00Z".parse().unwrap(),
            ended_at: None,
        }
    }

    fn attempt(id: &str, case: &str, verdict: Option<Verdict>) -> BenchAttempt {
        let mut attempt = BenchAttempt::pending(id.into(), case.into(), "builder".into(), 1);
        attempt.verdict = verdict;
        attempt
    }

    async fn seed(store: &BenchStore, run: &BenchRun) {
        store.put_run(run).await.unwrap();
        for attempt in &run.attempts {
            store.put_attempt(&run.id, attempt).await.unwrap();
        }
    }

    #[tokio::test]
    async fn live_benchmark_reads_keep_settlement_not_status_and_case_verdict_semantics() {
        let store = BenchStore::in_memory().unwrap();
        let provider = Provider::new(&store, std::env::temp_dir());
        let reader = Facts::<L6>::new();
        assert!(reader
            .get::<BenchResolutionFact, _>(&provider, &"missing".into())
            .await
            .unwrap()
            .is_none());
        let verdicts = [
            Verdict::Pass,
            Verdict::Fail,
            Verdict::Unverified,
            Verdict::Skipped,
            Verdict::Cancelled,
            Verdict::Error,
        ];
        let mut attempts: Vec<_> = verdicts
            .into_iter()
            .enumerate()
            .map(|(i, v)| attempt(&format!("a{i}"), "gated", Some(v)))
            .collect();
        attempts.push(attempt("ungated-pass", "ungated", Some(Verdict::Pass)));
        let older = run("settled-running-status", attempts);
        seed(&store, &older).await;
        let mut newest = run(
            "not-settled-done-status",
            vec![attempt("pending", "gated", None)],
        );
        newest.status = BenchRunStatus::Done;
        seed(&store, &newest).await;
        let names = ["demo".into(), "missing".into()].into_iter().collect();
        let gates = reader.get::<GateFact, _>(&provider, &names).await.unwrap();
        assert_eq!(gates.len(), 1);
        let gate = &gates["demo"];
        assert_eq!(gate.run_id, older.id);
        assert_eq!(gate.ended_at, None);
        assert!(gate.cases[0].gated);
        assert_eq!(gate.cases[0].verdicts, verdicts);
        assert!(!gate.cases[1].gated);
        assert_eq!(gate.cases[1].verdicts, [Verdict::Pass]);
        let counts = reader
            .get::<BenchResolutionFact, _>(&provider, &"demo".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!((counts.passed, counts.failed), (2, 1));
        assert_eq!(counts.started_at, older.started_at);
        assert_eq!(counts.ended_at, None);
        // One existing provider must see attempts written after its last read.
        store
            .put_attempt(
                &newest.id,
                &attempt("pending", "gated", Some(Verdict::Unverified)),
            )
            .await
            .unwrap();
        let counts = reader
            .get::<BenchResolutionFact, _>(&provider, &"demo".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(counts.run_id, newest.id);
        assert_eq!((counts.passed, counts.failed), (0, 0));
        assert_eq!(
            Provide::<GateFact>::get(&provider, &names).await.unwrap()["demo"].run_id,
            newest.id
        );
        assert!(Provide::<GateFact>::get(&provider, &BTreeSet::new())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn benchmark_providers_keep_the_existing_two_hundred_run_bound() {
        let store = BenchStore::in_memory().unwrap();
        let provider = Provider::new(&store, std::env::temp_dir());
        seed(
            &store,
            &run(
                "buried",
                vec![attempt("settled", "gated", Some(Verdict::Pass))],
            ),
        )
        .await;
        for i in 0..200 {
            seed(
                &store,
                &run(
                    &format!("pending-{i}"),
                    vec![attempt(&format!("a-{i}"), "gated", None)],
                ),
            )
            .await;
        }
        assert!(
            Provide::<BenchResolutionFact>::get(&provider, &"demo".into())
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            Provide::<GateFact>::get(&provider, &["demo".into()].into_iter().collect())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.runs(Some("demo"), 201).await.unwrap().len(), 201);
    }

    #[tokio::test]
    async fn knowledge_tags_are_read_live_from_the_instance_vault_not_a_cached_snapshot() {
        let root = std::env::temp_dir().join(format!("factory-l5-tags-{}", uuid::Uuid::new_v4()));
        let store = BenchStore::in_memory().unwrap();
        let provider = Provider::new(&store, root.clone());
        assert!(Provide::<KnowledgeTags>::get(&provider, &())
            .await
            .unwrap()
            .tags
            .is_empty());
        let vault = root.join(".factory/knowledge");
        std::fs::create_dir_all(&vault).unwrap();
        let page = vault.join("control.md");
        std::fs::write(
            &page,
            "---\ntitle: Evidence\ntags: [control/demo/one]\n---\nprivate body\n",
        )
        .unwrap();
        let first = Facts::<L6>::new()
            .get::<KnowledgeTags, _>(&provider, &())
            .await
            .unwrap();
        assert!(first.tags.contains("control/demo/one"));
        assert!(!serde_json::to_string(&first)
            .unwrap()
            .contains("private body"));
        std::fs::write(
            &page,
            "---\ntitle: Evidence\ntags: [control/demo/two]\n---\nprivate body\n",
        )
        .unwrap();
        let second = Provide::<KnowledgeTags>::get(&provider, &()).await.unwrap();
        assert!(!second.tags.contains("control/demo/one"));
        assert!(second.tags.contains("control/demo/two"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
